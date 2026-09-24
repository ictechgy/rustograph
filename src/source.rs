//! 수확 오케스트레이터 — cargo metadata와 syn 사이의 유일한 접점.
//!
//! 외부 의존(cargo, syn)은 이 파일과 modtree/harvest/cargo_meta에만 있다.
//! 수확은 판단하지 않는다: 해석 불가·조건부·외부 참조는 전부 실측 limitation이다.

use crate::cargo_meta::{self, Metadata};
use crate::graph::{self, Document, Edge, EdgeKind, Kind, Level, Vertex};
use crate::harvest::{self, BodyItem, Harvest};
use crate::modtree::{self, ModTree};
#[cfg(feature = "semantic")]
use crate::sem;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// 수확 옵션 — CLI 플래그의 도메인 표현.
#[derive(Debug, Default)]
pub struct Options {
    /// 심볼 레벨까지 수확할지(dead/signature 규칙은 항상 심볼이 필요하다).
    pub symbol_level: bool,
    /// 워크스페이스 외부 크레이트도 정점으로 포함할지(내부는 수확하지 않는다).
    pub include_deps: bool,
    /// 테스트 진입점(#[test]/#[bench])을 보존 루트로 잡을지.
    pub tests: bool,
    /// 모든 pub 아이템을 보존 루트로 잡을지(라이브러리용).
    pub retain_public: bool,
    /// 추가 보존 루트 ID.
    pub extra_roots: Vec<String>,
    /// ra_ap 의미 해석으로 본문 간선을 보강할지 — `semantic` feature 빌드 필요.
    /// 타입 해석 메서드 호출·매크로 확장·trait impl 행렬이 켜진다.
    pub semantic: bool,
    /// semantic 수확 결과를 디스크 캐시로 재사용할지 — syn 수확에는 무시된다.
    /// 캐시는 항상 부가적이다 — 손상·불일치는 조용히 새 수확으로 넘어간다.
    pub cache: bool,
}

/// 파싱된 파일 AST 아레나 — 수확이 끝날 때까지 아이템이 살아 있어야 해서
/// 'static으로 누수시킨다. CLI는 짧게 살다 종료하므로 해제 비용이 없다.
type Arena = BTreeMap<PathBuf, &'static [syn::Item]>;

/// `dir`의 cargo 워크스페이스를 수확해 문서를 만든다.
/// 실패는 문자열 오류 — 빈 그래프로 성공한 척하지 않는다.
/// semantic 모드에서 cache가 켜져 있으면 소스 지문이 같은 이전 수확
/// 문서를 재사용한다 — ra_ap 로드가 수십 초라 반복 질의의 실제 병목이다.
pub fn load(dir: &Path, opts: &Options) -> Result<Document, String> {
    // feature가 꺼진 빌드는 여기서 오류 — 조용히 syn으로 떨어지면
    // --semantic이 받은 결과가 의미 해석이 아니게 되어 거짓이 된다.
    if opts.semantic && !cfg!(feature = "semantic") {
        return Err(
            "--semantic requires a build with the `semantic` feature (cargo build --features semantic)"
                .to_string(),
        );
    }
    let meta = cargo_meta::load(dir)?;
    // 캐시 키는 모든 입력의 지문 — 지문을 못 재면 캐시를 끈다(실패는 부가적).
    let cache = if opts.semantic && opts.cache {
        fingerprint(dir, &meta, opts).map(|key| (key, cache_path(&meta)))
    } else {
        None
    };
    if let Some((key, path)) = &cache {
        if let Some(doc) = read_cache(path, *key) {
            return Ok(doc);
        }
    }
    let doc = harvest(dir, &meta, opts)?;
    if let Some((key, path)) = &cache {
        // `#[path]`가 워크스페이스 밖 파일을 로드하면 그 파일은 지문에
        // 없어 캐시가 stale해진다 — 그런 문서는 캐시에 쓰지 않는다.
        if !touches_outside(&doc, &meta.workspace_root) {
            write_cache(path, *key, &doc);
        }
    }
    Ok(doc)
}

/// `--target`의 cfg 팩트 — `rustc --print cfg` 실측이 권위다.
/// rustc를 못 쓰거나 트리플을 모르면 트리플 추정으로 폴백한다 —
/// 부분 팩트는 complete=false라 모르는 조건은 미지로 남는다.
pub fn target_facts(triple: &str) -> crate::cfgeval::Facts {
    if let Some(lines) = cargo_meta::rustc_cfg_lines(triple) {
        crate::cfgeval::Facts::from_cfg_lines(lines.iter().map(String::as_str))
    } else {
        crate::cfgeval::Facts::from_triple(triple)
    }
}

/// 문서 정점이 지문이 안 보는 파일을 가리키는가 — `#[path]`로
/// `../shared.rs`처럼 루트를 벗어난 파일이나, `include!`/`OUT_DIR`로
/// 로드된 `target/` 아래 생성 파일은 지문에 없어 캐시가 stale해진다.
/// 그런 문서는 캐시에 쓰지 않는다.
fn touches_outside(doc: &Document, root: &Path) -> bool {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let target = root.join("target");
    doc.vertices.iter().any(|v| {
        v.position
            .as_deref()
            .and_then(|p| p.rsplit_once(':').map(|(f, _)| PathBuf::from(f)))
            .is_some_and(|f| {
                let c = f.canonicalize().unwrap_or(f);
                !c.starts_with(&root) || c.starts_with(&target)
            })
    })
}

/// 캐시 파일 위치 — 워크스페이스 루트의 .rustograph/ 아래(gitignore됨).
fn cache_path(meta: &Metadata) -> PathBuf {
    meta.workspace_root.join(".rustograph/semantic-cache.json")
}

/// 수확 입력의 지문 — FNV-1a로 경로·크기·mtime을 접는다.
/// 소스 하나라도 바뀌면 키가 달라진다. 지문 재기에 실패하면 None —
/// 캐시 없이 수확하는 것이 캐시 때문에 실패하는 것보다 항상 낫다.
fn fingerprint(dir: &Path, meta: &Metadata, opts: &Options) -> Option<u64> {
    let mut h = 0xcbf29ce484222325u64;
    let mut feed = |bytes: &[u8]| {
        for &b in bytes {
            h = (h ^ u64::from(b)).wrapping_mul(0x100000001b3);
        }
    };
    // 스키마·도구 버전 — 출력 계약이 바뀌면 캐시도 무효다.
    feed(b"rustograph-cache-v1");
    feed(env!("CARGO_PKG_VERSION").as_bytes());
    feed(&graph::DOCUMENT_VERSION.to_le_bytes());
    feed(dir.canonicalize().ok()?.display().to_string().as_bytes());
    feed(&[
        opts.symbol_level as u8,
        opts.include_deps as u8,
        opts.tests as u8,
        opts.retain_public as u8,
    ]);
    let mut roots = opts.extra_roots.clone();
    roots.sort();
    for r in roots {
        feed(r.as_bytes());
        feed(&[0]);
    }
    // 워크스페이스 아래의 모든 .rs와 매니페스트 — 어느 파일이든 바뀌면
    // 지문이 달라진다. target/은 산출물이라 건너뛰고 숨김 디렉터리 중
    // .cargo는 config가 빌드 입력을 바꾸니 포함한다. 디렉터리 항목
    // 하나라도 못 읽으면 지문이 부분적이니 None — 부분 지문은
    // stale 캐시를 재사용하는 최악의 경로다.
    let mut stack = vec![meta.workspace_root.clone()];
    let mut files: BTreeMap<PathBuf, (u64, u64, u32)> = BTreeMap::new();
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).ok()? {
            let e = e.ok()?;
            let p = e.path();
            if p.is_dir() {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name != "target" && (!name.starts_with('.') || name == ".cargo") {
                    stack.push(p);
                }
                continue;
            }
            let interesting = p.extension().is_some_and(|x| x == "rs")
                || matches!(
                    p.file_name().and_then(|n| n.to_str()),
                    Some("Cargo.toml" | "Cargo.lock")
                );
            if !interesting {
                continue;
            }
            let md = e.metadata().ok()?;
            let mtime = md
                .modified()
                .ok()?
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?;
            files.insert(p, (md.len(), mtime.as_secs(), mtime.subsec_nanos()));
        }
    }
    for (p, (len, secs, nanos)) in files {
        feed(p.display().to_string().as_bytes());
        feed(&len.to_le_bytes());
        feed(&secs.to_le_bytes());
        feed(&nanos.to_le_bytes());
    }
    Some(h)
}

/// 캐시 파일을 읽는다 — 지문이 다르거나 손상됐으면 None(새 수확).
fn read_cache(path: &Path, key: u64) -> Option<Document> {
    #[derive(serde::Deserialize)]
    struct CacheFile {
        key: String,
        document: Document,
    }
    let src = std::fs::read_to_string(path).ok()?;
    let cf: CacheFile = serde_json::from_str(&src).ok()?;
    if cf.key != format!("{key:016x}") || cf.document.version > graph::DOCUMENT_VERSION {
        return None;
    }
    Some(cf.document)
}

/// 캐시를 쓴다 — 실패해도 수확 결과는 유효하니 조용히 넘긴다.
fn write_cache(path: &Path, key: u64, doc: &Document) {
    #[derive(serde::Serialize)]
    struct CacheFile<'a> {
        key: &'a str,
        document: &'a Document,
    }
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_err() {
        return;
    }
    if let Ok(text) = serde_json::to_string(&CacheFile {
        key: &format!("{key:016x}"),
        document: doc,
    }) {
        let _ = std::fs::write(path, text);
    }
}

/// 실제 수확 — 메타데이터 위에서 모듈 트리·간선을 조립한다.
fn harvest(dir: &Path, meta: &Metadata, opts: &Options) -> Result<Document, String> {
    let mut harvest = Harvest::default();
    let mut vertices: Vec<Vertex> = Vec::new();
    let mut edges: Vec<Edge> = Vec::new();
    let mut entry_roots: Vec<String> = Vec::new();
    let mut limitations = meta.limitations.clone();

    // 코드가 보는 lib 별칭 → 크레이트 정점. 스코프는 의존을 선언한
    // 크레이트다 — 같은 별칭을 멤버마다 다른 패키지에 물릴 수 있어
    // 전역 맵이면 last-wins로 오염된다. 멤버 의존은 붕괴하지 않고
    // 그 멤버의 루트 정점(타깃 이름)부터 걷는다 — 패키지 이름과
    // lib 타깃 이름이 다르면 패키지 이름 정점은 없다.
    // --deps일 때만 채운다 — 정점이 없는데 dep 경로가 해석되면
    // dangling 간선이 생긴다. 빈 표면 resolve는 옛 동작 그대로다.
    let dep_crates: modtree::DepCrates = if opts.include_deps {
        let mut dc = modtree::DepCrates::new();
        for d in &meta.dep_edges {
            let Some(&fi) = meta.by_id.get(&d.from) else {
                continue;
            };
            let from_pkg = &meta.packages[fi];
            if !from_pkg.workspace_member {
                continue;
            }
            let Some(&ti) = meta.by_id.get(&d.to) else {
                continue;
            };
            let to_pkg = &meta.packages[ti];
            // 멤버는 루트 모듈 정점 — lib 타깃 이름이 정점 ID다.
            // lib이 없는 멤버(proc-macro 타깃만 있거나 순수 bin 패키지)는
            // 걸을 모듈 트리가 없으니 패키지 정점으로 붕괴한다.
            let lib_root = to_pkg
                .targets
                .iter()
                .find(|t| t.kind == "lib")
                .map(|t| t.name.clone());
            let (vertex, member) = match lib_root {
                Some(root) if to_pkg.workspace_member => (root, true),
                _ => (to_pkg.name.clone(), false),
            };
            // 의존 선언은 패키지의 모든 타깃(lib·bin 전부)에 적용된다.
            for t in &from_pkg.targets {
                dc.insert(
                    t.name.clone(),
                    d.lib_name.clone(),
                    modtree::DepTarget {
                        vertex: vertex.clone(),
                        member,
                    },
                );
            }
        }
        dc
    } else {
        Default::default()
    };
    // 정점으로 존재하는 외부 크레이트 이름 — uses 간선 방출용.
    let dep_vertices: BTreeSet<String> = dep_crates.external.clone();

    emit_crate_level(
        meta,
        opts.include_deps,
        &mut vertices,
        &mut edges,
        &mut limitations,
    );

    // 전 워크스페이스가 한 트리 — 크레이트 간 `othercrate::x` 해석이 가능해야 한다.
    let mut tree = ModTree::default();
    let mut arena: Arena = BTreeMap::new();
    let mut conditional_mods = 0usize;
    let mut scan_roots: BTreeSet<PathBuf> = BTreeSet::new();
    for p in meta.packages.iter().filter(|p| p.workspace_member) {
        for t in &p.targets {
            if !matches!(t.kind.as_str(), "lib" | "bin") {
                continue; // example/test/bench 타깃은 수확 범위 밖(루트가 아님).
            }
            if let Ok(f) = t.src.canonicalize() {
                if let Some(d) = f.parent() {
                    scan_roots.insert(d.to_path_buf());
                }
            }
            grow_tree(
                &mut tree,
                &mut arena,
                &t.name,
                &t.src,
                &mut conditional_mods,
            )?;
            if t.kind == "bin" {
                entry_roots.push(format!("{}::main", t.name));
            }
        }
    }
    // orphan .rs — 전 타깃 처리 후에 스캔한다. lib의 스캔이 bin 병합 전에
    // 돌면 main.rs가 orphan으로 오인된다. known은 공유 루트의 extra_files까지.
    let known: BTreeSet<PathBuf> = tree
        .modules
        .values()
        .flat_map(|m| std::iter::once(m.file.clone()).chain(m.extra_files.iter().cloned()))
        .collect();
    for root in scan_roots {
        scan_orphans(&root, &known, &mut tree);
    }
    if conditional_mods > 0 {
        limitations.push(format!(
            "{conditional_mods} modules behind #[cfg] were included without feature evaluation"
        ));
    }
    if !tree.orphan_files.is_empty() {
        limitations.push(format!(
            "{} .rs files are not reachable from any mod declaration (orphans)",
            tree.orphan_files.len()
        ));
    }

    // 스코프 2단계: 전 모듈의 아이템 표를 먼저 채우고(1단계), 그 다음
    // `use`를 해석한다(2단계). 임포트 해석이 다른 모듈의 아이템 표를 필요로
    // 하기 때문 — 한 패스로 하면 처리 순서에 따라 크로스 크레이트 임포트가
    // 조용히 유실된다.
    for mp in tree.modules.keys().cloned().collect::<Vec<_>>() {
        if let Some(groups) = module_items(&tree, &arena, &mp) {
            modtree::fill_items(&mut tree, &mp, &flatten_items(&groups));
        }
    }
    for mp in tree.modules.keys().cloned().collect::<Vec<_>>() {
        if let Some(groups) = module_items(&tree, &arena, &mp) {
            modtree::fill_imports(&mut tree, &mp, &flatten_items(&groups), &dep_crates);
        }
    }

    // 1패스: 모듈·아이템 선언 — 정점과 contains/uses/implements 간선.
    let mut bodies: Vec<BodyItem<'_>> = Vec::new();
    let mut impls: Vec<harvest::ImplBlock> = Vec::new();
    let mut attr_refs: Vec<harvest::AttrRef> = Vec::new();
    let mut test_roots: Vec<String> = Vec::new();
    for mp in tree.modules.keys().cloned().collect::<Vec<_>>() {
        let mh = harvest_module(&mut tree, &arena, &mp, &mut harvest, &dep_vertices);
        vertices.push(mh.vertex);
        edges.extend(mh.edges);
        vertices.extend(mh.decls.vertices);
        entry_roots.extend(mh.decls.entry_roots);
        test_roots.extend(mh.decls.test_roots);
        bodies.extend(mh.decls.bodies);
        attr_refs.extend(mh.decls.attr_refs);
        impls.extend(mh.decls.impls);
    }
    for block in &impls {
        let krate = modtree::crate_of(&block.items_module);
        let (vs, es, ar, bs) =
            harvest::impls(std::slice::from_ref(block), &krate, &tree, &mut harvest);
        vertices.extend(vs);
        edges.extend(es);
        attr_refs.extend(ar);
        bodies.extend(bs);
    }

    // 메서드 이름 → ID 인덱스 — 이름 팬아웃 폴백에 쓴다(semantic에서는
    // ra가 해석하지 못한 호출에만 쓰인다).
    let mut method_index: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for v in &vertices {
        if v.kind == Kind::Method {
            if let Some(name) = v.id.rsplit("::").next() {
                method_index
                    .entry(name.to_string())
                    .or_default()
                    .push(v.id.clone());
            }
        }
    }

    // 2패스: 본문 — 모든 정점이 준비된 뒤에 해석한다.
    // semantic 엔진이 붙어 있으면 hir이 아는 본문은 타입 해석으로 정확히
    // 잡고, 모르는 본문(cfg 비활성·매크로 생성)만 syn 팬아웃으로 돌아간다.
    #[cfg(feature = "semantic")]
    let engine = if opts.semantic {
        // syn이 수확한 선언 위치 — 의미 해석 쪽에서 정규 ID 문자열이
        // 가리키는 정점의 provenance 검증에 쓴다(생성 정의 충돌 방지).
        let mut sites: BTreeMap<String, Vec<sem::Site>> = BTreeMap::new();
        for b in &bodies {
            sites.entry(b.id.clone()).or_default().push(sem::Site {
                file: b.file.clone(),
                range: b.range.clone(),
                module: b.module.clone(),
            });
        }
        Some(sem::Engine::load(&meta.workspace_root, &sites)?)
    } else {
        None
    };
    #[cfg(feature = "semantic")]
    if let Some(eng) = &engine {
        let ids: BTreeSet<&str> = vertices.iter().map(|v| v.id.as_str()).collect();
        let mut st = sem::Stats::default();
        for b in &bodies {
            let site = sem::OwnerSite {
                id: &b.id,
                cfg: &b.cfg,
                file: &b.file,
                range: &b.range,
                // 소유 크레이트·모듈 — 같은 파일을 둘이 넘는 문맥이
                // 공유해도(공유 include!·`#[path]`) 선언 문맥으로 정확한
                // 항목을 고른다.
                krate: b.module.split("::").next().unwrap_or_default(),
                module: &b.module,
            };
            match eng.body_edges(&site, &ids, &method_index, &mut st) {
                Some(es) => {
                    // 시그니처 간선은 엔진과 무관하게 syn이 권위다.
                    edges.extend(harvest::signature_edges(b, &tree, &dep_crates));
                    edges.extend(es);
                }
                None => {
                    st.unmapped += 1;
                    edges.extend(harvest::bodies(
                        std::slice::from_ref(b),
                        &tree,
                        &dep_crates,
                        &method_index,
                        &mut harvest,
                    ));
                }
            }
        }
        push_sem_stats(&st, eng.has_proc_macros(), &mut limitations, &mut harvest);
    } else {
        edges.extend(harvest::bodies(
            &bodies,
            &tree,
            &dep_crates,
            &method_index,
            &mut harvest,
        ));
    }
    #[cfg(not(feature = "semantic"))]
    edges.extend(harvest::bodies(
        &bodies,
        &tree,
        &dep_crates,
        &method_index,
        &mut harvest,
    ));

    // 속성 경로 참조 — 모드와 무관하게 syn이 권위다. `#[dep::attr]`나
    // `#[derive(dep::X)]`는 의미 해석이 더 잘 아는 것이 없다.
    edges.extend(harvest::attr_edges(
        &attr_refs,
        &tree,
        &dep_crates,
        &mut harvest,
    ));

    push_limitations(&harvest, &mut limitations);

    // 보존 루트 — 존재하는 정점만 루트가 된다.
    let ids: BTreeSet<&str> = vertices.iter().map(|v| v.id.as_str()).collect();
    let mut root_set: BTreeSet<String> = BTreeSet::new();
    for r in entry_roots {
        if ids.contains(r.as_str()) {
            root_set.insert(r);
        }
    }
    // #[test]/#[bench] 진입점은 --tests일 때만 보존 루트다.
    if opts.tests {
        for r in test_roots {
            if ids.contains(r.as_str()) {
                root_set.insert(r);
            }
        }
    }
    if opts.retain_public {
        for v in &vertices {
            if v.exported {
                root_set.insert(v.id.clone());
            }
        }
    }
    for r in &opts.extra_roots {
        if ids.contains(r.as_str()) {
            root_set.insert(r.clone());
        }
    }

    let level = if opts.symbol_level {
        Level::Symbol
    } else {
        Level::Module
    };
    Ok(graph::document(
        level,
        dir.display().to_string(),
        Some(meta.workspace_root.display().to_string()),
        root_set.into_iter().collect(),
        vertices,
        edges,
        limitations,
    ))
}

/// 크레이트 정점과 `depends` 간선을 만든다.
fn emit_crate_level(
    meta: &Metadata,
    include_deps: bool,
    vertices: &mut Vec<Vertex>,
    edges: &mut Vec<Edge>,
    limitations: &mut Vec<String>,
) {
    let mut skipped = 0usize;
    let mut present: BTreeSet<&str> = BTreeSet::new();
    for p in &meta.packages {
        // 워크스페이스 멤버는 타깃 루트 모듈이 크레이트 정점을 겸한다 —
        // rustc에서 타깃이 곧 크레이트이고, 별도 정점은 패키지명==타깃명일 때
        // 충돌한다.
        if p.workspace_member {
            present.insert(p.name.as_str());
            // lib/bin 타깃이 없는 멤버(proc-macro 크레이트 등)는 겸임할 루트
            // 모듈이 없다 — depends 간선이 dangling하지 않게 정점을 만든다.
            if p.targets
                .iter()
                .all(|t| !matches!(t.kind.as_str(), "lib" | "bin"))
            {
                vertices.push(Vertex {
                    id: p.name.clone(),
                    kind: Kind::Crate,
                    krate: p.name.clone(),
                    module: p.name.clone(),
                    position: None,
                    exported: false,
                    generated: false,
                    cfg: None,
                    unsafe_: false,
                });
            }
            continue;
        }
        if !include_deps {
            skipped += 1;
            continue;
        }
        present.insert(p.name.as_str());
        vertices.push(Vertex {
            id: p.name.clone(),
            kind: Kind::Crate,
            krate: p.name.clone(),
            module: p.name.clone(),
            position: None,
            exported: false,
            generated: false,
            cfg: None,
            unsafe_: false,
        });
    }
    if skipped > 0 {
        limitations.push(format!(
            "{skipped} external crates were omitted (use --deps to include)"
        ));
    }
    let mut dev_edges = 0usize;
    for d in &meta.dep_edges {
        let (Some(f), Some(t)) = (meta.by_id.get(&d.from), meta.by_id.get(&d.to)) else {
            continue;
        };
        let (fp, tp) = (
            meta.packages[*f].name.as_str(),
            meta.packages[*t].name.as_str(),
        );
        if !present.contains(fp) || !present.contains(tp) {
            continue;
        }
        if !d.kind.is_empty() {
            dev_edges += 1;
        }
        edges.push(Edge::new(fp.to_string(), tp.to_string(), EdgeKind::Depends));
    }
    if dev_edges > 0 {
        limitations.push(format!("{dev_edges} dev/build dependency edges included"));
    }
}

/// 타깃의 모듈 트리를 키운다 — mod 선언을 따라 파일을 발견한다.
fn grow_tree(
    tree: &mut ModTree,
    arena: &mut Arena,
    root_path: &str,
    root_file: &Path,
    conditional_count: &mut usize,
) -> Result<(), String> {
    let root_file = root_file
        .canonicalize()
        .map_err(|e| format!("cannot read {}: {e}", root_file.display()))?;
    match tree.modules.get_mut(root_path) {
        // lib와 같은 이름의 bin — 두 크레이트 인스턴스가 루트 경로를 공유한다.
        Some(m) => m.extra_files.push(root_file.clone()),
        None => {
            tree.modules.insert(
                root_path.to_string(),
                modtree::Module::new(root_file.clone(), true, true),
            );
        }
    }
    parse_into(arena, &root_file)?;
    let mut queue = vec![root_path.to_string()];
    let mut visited: BTreeSet<String> = BTreeSet::new();
    while let Some(mp) = queue.pop() {
        if !visited.insert(mp.clone()) {
            continue;
        }
        let Some(groups) = module_items(tree, arena, &mp) else {
            continue;
        };
        for sub in
            modtree::collect_submodules(&flatten_items(&groups), &mp, tree, conditional_count)
        {
            let file = tree.modules[&sub].file.clone();
            if tree.modules[&sub].file_module {
                parse_into(arena, &file)?;
            }
            queue.push(sub);
        }
    }
    Ok(())
}

/// 디렉터리 아래를 훑어 선언된 모듈 파일 집합에 없는 .rs를 orphan으로 잡는다.
fn scan_orphans(dir: &Path, known: &BTreeSet<PathBuf>, tree: &mut ModTree) {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                if let Ok(c) = p.canonicalize() {
                    if !known.contains(&c) && !tree.orphan_files.contains(&c) {
                        tree.orphan_files.push(c);
                    }
                }
            }
        }
    }
}

/// 파일을 파싱해 아레나에 넣는다. 파싱 실패는 limitation이 아니라 오류 —
/// 읽은 파일을 못 읽는 것과 해석 못 하는 것은 다르다…지만 실전에서는
/// 조건부 생성 파일이 깨진 문법을 가질 수 있어 빈 목록으로 둔다.
fn parse_into(arena: &mut Arena, file: &Path) -> Result<(), String> {
    if arena.contains_key(file) {
        return Ok(());
    }
    let src = std::fs::read_to_string(file)
        .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    match syn::parse_file(&src) {
        Ok(ast) => {
            arena.insert(file.to_path_buf(), Box::leak(ast.items.into_boxed_slice()));
            Ok(())
        }
        Err(e) => Err(format!("parse error in {}: {e}", file.display())),
    }
}

/// 모듈의 아이템 그룹 — (선언 파일, 그 파일의 아이템 목록) 쌍.
/// 파일 모듈은 파일 AST(루트는 extra_files까지 — 같은 이름의 lib/bin이
/// 루트를 공유할 때 각 아이템의 실제 파일을 보존해야 semantic 엔진이
/// 본문 소유자를 올바른 소스에 맞춘다), 인라인 모듈은 조상의 mod 본문.
fn module_items<'a>(
    tree: &ModTree,
    arena: &'a Arena,
    path: &str,
) -> Option<Vec<(PathBuf, &'a [syn::Item])>> {
    let module = tree.modules.get(path)?;
    if module.file_module {
        let mut out: Vec<(PathBuf, &'a [syn::Item])> = Vec::new();
        if let Some(items) = arena.get(&module.file).copied() {
            out.push((module.file.clone(), items));
        }
        for f in &module.extra_files {
            if let Some(items) = arena.get(f).copied() {
                out.push((f.clone(), items));
            }
        }
        return Some(out);
    }
    // 인라인 모듈: 같은 파일을 쓰는 파일-모듈 조상까지 올라간다.
    let mut top = path.to_string();
    loop {
        let m = tree.modules.get(&top)?;
        if m.file_module {
            break;
        }
        top = modtree::parent_of(&top)?;
    }
    let mut items: &'a [syn::Item] = arena.get(&tree.modules[&top].file).copied()?;
    // 파일 루트에서 목표까지 인라인 mod 본문을 따라 내려간다.
    let rel = path.strip_prefix(&format!("{top}::"))?;
    for seg in rel.split("::") {
        let mut next: Option<&'a [syn::Item]> = None;
        for it in items {
            if let syn::Item::Mod(m) = it {
                if m.ident == seg {
                    if let Some((_, content)) = &m.content {
                        next = Some(content);
                    }
                }
            }
        }
        items = next?;
    }
    Some(vec![(tree.modules[&top].file.clone(), items)])
}

/// 파일 그룹을 아이템 목록으로 펼친다 — 스코프 채우기·mod 수집처럼
/// 선언 파일이 필요 없는 호출자용.
fn flatten_items<'a>(groups: &[(PathBuf, &'a [syn::Item])]) -> Vec<&'a syn::Item> {
    groups.iter().flat_map(|(_, items)| items.iter()).collect()
}

/// 모듈 하나의 수확 산출물 — 호출자가 순서대로 합친다.
struct ModuleHarvest<'a> {
    vertex: Vertex,
    edges: Vec<Edge>,
    decls: harvest::ModuleDecls<'a>,
}

/// 한 모듈의 선언을 수확한다 — 모듈 정점, contains/uses 간선, 본문 보관.
fn harvest_module<'a>(
    tree: &mut ModTree,
    arena: &'a Arena,
    mp: &str,
    out: &mut Harvest,
    dep_vertices: &BTreeSet<String>,
) -> ModuleHarvest<'a> {
    let groups = module_items(tree, arena, mp).expect("module items must exist");
    let file = tree.modules[mp].file.clone();
    let krate = modtree::crate_of(mp);
    // 파라미터 이름이 fn harvest와 충돌하면 ident 경로가 함수 정점으로
    // 해석돼 거짓 간선·사이클이 생긴다 — 지역명은 `out`으로 둔다.
    let decls = harvest::decls(mp, &krate, &groups, out);

    // 모듈 정점 — 타깃 루트는 크레이트 정점을 겸한다(rustc 의미론).
    // 위치는 파일 시작, exported는 `pub mod` 여부를 따른다.
    let is_root = !mp.contains("::");
    let public = tree.modules[mp].public;
    let generated = file_has_marker(&file);
    let mod_cfg = tree.modules[mp].cfg.clone();
    let vertex = Vertex {
        id: mp.to_string(),
        kind: if is_root { Kind::Crate } else { Kind::Module },
        krate: krate.clone(),
        module: mp.to_string(),
        position: Some(format!("{}:1", file.display())),
        exported: public,
        generated,
        cfg: mod_cfg.clone(),
        unsafe_: false,
    };
    let mut edges = Vec::new();
    if let Some(parent) = modtree::parent_of(mp) {
        // 조건부 mod 선언의 contains는 그 조건 아래서만 성립한다.
        let mut e = Edge::new(parent, mp.to_string(), EdgeKind::Contains);
        e.cfg = mod_cfg.clone();
        edges.push(e);
    }
    // 모듈 직속 아이템에 contains 간선 — impl 메서드는 impls()가 타입 아래로 단다.
    // 아이템이 cfg면 그 아이템은 그 조건 아래서만 존재하니 contains도 같다.
    for v in &decls.vertices {
        if v.id
            .strip_prefix(&format!("{mp}::"))
            .is_some_and(|rest| !rest.contains("::"))
        {
            let mut e = Edge::new(mp.to_string(), v.id.clone(), EdgeKind::Contains);
            e.cfg = v.cfg.clone();
            edges.push(e);
        }
    }
    // use 임포트 → uses 간선. 해석된 정규 경로가 아이템이면 그 정점으로,
    // 아니면 소유 모듈로. 아이템 표는 1단계에서 전 모듈에 채워졌으므로
    // 모듈 처리 순서와 무관하게 같은 결과가 나온다. 임포트에 cfg가 있으면
    // 간선도 그 조건 아래서만 성립한다.
    for imp in tree.modules[mp].imports.values() {
        let to = if tree.item_exists(&imp.target) {
            imp.target.clone()
        } else if dep_vertices.contains(imp.target.as_str()) {
            // 외부 크레이트 정점 — `use dep::X`는 선언 의존의 실제 사용 증거다.
            imp.target.clone()
        } else {
            match owner_module(tree, &imp.target) {
                Some(m) if m != mp => m.clone(),
                _ => continue,
            }
        };
        if to != mp {
            let mut e = Edge::new(mp.to_string(), to, EdgeKind::Uses);
            e.cfg = imp.cfg.clone();
            edges.push(e);
        }
    }
    ModuleHarvest {
        vertex,
        edges,
        decls,
    }
}

/// 정규 ID의 소유 모듈 경로를 찾는다.
fn owner_module(tree: &ModTree, id: &str) -> Option<String> {
    let mut cur = id.to_string();
    loop {
        if tree.modules.contains_key(&cur) {
            return Some(cur);
        }
        cur = cur.rfind("::").map(|i| cur[..i].to_string())?;
    }
}

/// 파일의 생성 코드 마커 — harvest의 판정과 같은 규칙.
fn file_has_marker(file: &Path) -> bool {
    std::fs::read_to_string(file)
        .map(|s| {
            s.lines().take(10).any(|l| {
                let l = l.to_lowercase();
                l.contains("do not edit")
                    || l.contains("@generated")
                    || l.contains("auto-generated")
            })
        })
        .unwrap_or(false)
}

/// 수확 실측을 limitation 문장으로 옮긴다.
fn push_limitations(h: &Harvest, limitations: &mut Vec<String>) {
    if h.unresolved_paths > 0 {
        limitations.push(format!(
            "{} path references could not be resolved within the workspace (external crates, prelude names, or macro-generated names)",
            h.unresolved_paths
        ));
    }
    if h.fanned_method_calls > 0 {
        limitations.push(format!(
            "{} method calls resolved by name fan-out (no type information; errs toward alive)",
            h.fanned_method_calls
        ));
    }
    if h.external_macros > 0 {
        limitations.push(format!(
            "{} macro invocations resolve to std/external macros and were omitted; \
             expression-shaped arguments were still parsed, but macro-internal \
             syntax (patterns, key-value args) is invisible to syntactic analysis",
            h.external_macros
        ));
    }
    if h.cfg_items > 0 {
        limitations.push(format!(
            "{} items behind #[cfg] were included without feature evaluation",
            h.cfg_items
        ));
    }
}

/// 의미 해석 실측을 limitation 문장과 공유 카운터로 옮긴다.
/// fanned/unresolved는 syn과 같은 버킷 — 메시지가 두 개로 갈라지면
/// "총 몇 개인가"가 읽기 어려워진다. ext_macros는 별도 문장으로 둔다 —
/// syn 문구는 "인자를 구문으로 파싱했다"는 전제를 담는데 의미 해석은
/// 확장 트리를 걷기 때문에 그 주장이 거짓이 된다.
#[cfg(feature = "semantic")]
fn push_sem_stats(
    st: &sem::Stats,
    has_proc_macros: bool,
    limitations: &mut Vec<String>,
    out: &mut Harvest,
) {
    out.fanned_method_calls += st.fanned;
    out.unresolved_paths += st.unresolved;
    limitations.push(format!(
        "semantic analysis: {} call/reference edges resolved via types; \
         {} macro expansions walked; {} trait-dispatch sites expanded to candidate impls",
        st.resolved, st.expanded, st.trait_sites
    ));
    if st.ext_macros > 0 {
        limitations.push(format!(
            "{} macro invocations resolve to macros outside the graph (std/external defs or unresolved paths)",
            st.ext_macros
        ));
    }
    if st.external > 0 {
        limitations.push(format!(
            "{} call targets resolved to items outside the graph (dependencies, std, or macro/derive-generated defs)",
            st.external
        ));
    }
    if st.proc_macros > 0 && !has_proc_macros {
        limitations.push(format!(
            "{} proc-macro invocations could not be expanded (proc-macro server unavailable)",
            st.proc_macros
        ));
    }
    if st.unrepresentable > 0 {
        limitations.push(format!(
            "{} trait-dispatch candidates have no graph vertex (blanket, primitive, or generated impls); counted at dispatch sites",
            st.unrepresentable
        ));
    }
    if st.unexpanded > 0 {
        let cause = if has_proc_macros {
            "expansion failure or depth limit"
        } else {
            "proc-macro server unavailable or expansion failure"
        };
        limitations.push(format!(
            "{} macro calls could not be expanded ({cause})",
            st.unexpanded
        ));
    }
    if st.unmapped > 0 {
        limitations.push(format!(
            "{} bodies invisible to semantic analysis (cfg-disabled or macro-generated); syntactic fan-out used",
            st.unmapped
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cargo_meta::Metadata;
    use crate::graph::{document, Level};

    /// 캐시 지문의 왕복과 무효화 — 파일 하나라도 바뀌면 키가 달라져야 한다.
    #[test]
    fn cache_roundtrip_and_invalidation() {
        let tmp = std::env::temp_dir().join(format!("rustograph-cache-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        std::fs::create_dir_all(&tmp).unwrap();
        std::fs::write(tmp.join("a.rs"), "fn a() {}").unwrap();
        let meta = Metadata {
            packages: vec![],
            by_id: BTreeMap::new(),
            dep_edges: vec![],
            workspace_root: tmp.clone(),
            limitations: vec![],
        };
        let opts = Options {
            semantic: true,
            cache: true,
            ..Default::default()
        };
        let k1 = fingerprint(&tmp, &meta, &opts).expect("fingerprint");
        assert_eq!(k1, fingerprint(&tmp, &meta, &opts).unwrap());
        // 내용이 바뀌면 지문이 달라진다 — 길이도 달라 mtime 세분도와 무관.
        std::fs::write(tmp.join("a.rs"), "fn a() { let much_longer = 1; }").unwrap();
        let k2 = fingerprint(&tmp, &meta, &opts).unwrap();
        assert_ne!(k1, k2);
        // 수확 옵션도 키에 들어간다 — --deps 문서를 캐시로 속이면 안 된다.
        let opts_deps = Options {
            include_deps: true,
            ..Default::default()
        };
        assert_ne!(k2, fingerprint(&tmp, &meta, &opts_deps).unwrap());
        // 읽기·쓰기 왕복 — 같은 키면 문서가 돌아온다.
        let doc = document(
            Level::Symbol,
            ".".into(),
            None,
            vec![],
            vec![],
            vec![],
            vec![],
        );
        let path = cache_path(&meta);
        write_cache(&path, k2, &doc);
        let got = read_cache(&path, k2).expect("cache hit");
        assert_eq!(got.vertices.len(), doc.vertices.len());
        // 다른 키와 손상된 파일은 None — 새 수확으로 넘어간다.
        assert!(read_cache(&path, k1).is_none());
        std::fs::write(&path, "not json").unwrap();
        assert!(read_cache(&path, k2).is_none());
        // 캐시 파일이 워크스페이스 안에 있어도 지문을 바꾸지 않는다 —
        // .rustograph는 숨김 디렉터리라 지문 걷기에서 빠진다.
        assert_eq!(k2, fingerprint(&tmp, &meta, &opts).unwrap());
        std::fs::remove_dir_all(&tmp).ok();
    }
}
