//! 수확 오케스트레이터 — cargo metadata와 syn 사이의 유일한 접점.
//!
//! 외부 의존(cargo, syn)은 이 파일과 modtree/harvest/cargo_meta에만 있다.
//! 수확은 판단하지 않는다: 해석 불가·조건부·외부 참조는 전부 실측 limitation이다.

pub mod routes;
pub mod schema;

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

/// 분석 대상이 경로를 고르는 소스를 읽을 때의 크기 상한이다. 생성 코드(수 MB)는 넉넉히
/// 덮되, 상한 없는 읽기가 거대한 sparse 파일을 통째로 메모리에 올리지 않게 한다.
/// 판단값이다(rustc의 문서화된 한도가 아니다).
pub(crate) const MAX_SOURCE_BYTES: u64 = 64 * 1024 * 1024;

/// 분석 대상 소스를 `max_bytes`까지만 읽는다 — 정규 파일만 연다([`require_regular_file`]).
/// 정규 파일이 아니면 `InvalidInput`, 상한을 넘으면 `InvalidData` 오류다.
pub(crate) fn read_source_bounded(path: &Path, max_bytes: u64) -> std::io::Result<String> {
    use std::io::Read;
    require_regular_file(path)?;
    let mut text = String::new();
    std::fs::File::open(path)?
        .take(max_bytes.saturating_add(1))
        .read_to_string(&mut text)?;
    if text.len() as u64 > max_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("{} is larger than {max_bytes} bytes", path.display()),
        ));
    }
    Ok(text)
}

/// 분석 대상이 경로를 고른 소스가 정규 파일인지 연기 전에 본다 — `metadata`는 stat이라
/// FIFO에서도 막히지 않는다. FIFO는 여는 순간 쓰는 쪽을 영원히 기다리고 장치(`/dev/zero`,
/// 그것을 가리키는 링크)는 끝없이 읽혀, 보지 않고 읽으면 수확이 끝나지 않거나 메모리를 다
/// 쓴다. 정규 파일이 아니면 `InvalidInput`이고 문구에 해결 방향을 싣는다 — 호출자가 앞에
/// 경로를 붙인다.
fn require_regular_file(path: &Path) -> std::io::Result<()> {
    if std::fs::metadata(path)?.is_file() {
        return Ok(());
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        "not a regular file (devices, FIFOs and directories are not read); \
         point it at a regular .rs file",
    ))
}

/// `dir`의 cargo 워크스페이스를 수확해 문서를 만든다.
/// 실패는 문자열 오류 — 빈 그래프로 성공한 척하지 않는다.
/// semantic 모드에서 cache가 켜져 있으면 소스 지문이 같은 이전 수확
/// 문서를 재사용한다 — ra_ap 로드가 수십 초라 반복 질의의 실제 병목이다.
pub fn load(dir: &Path, opts: &Options) -> Result<Document, String> {
    require_semantic_build(opts)?;
    let meta = cargo_meta::load(dir)?;
    load_with(dir, &meta, opts)
}

/// feature가 꺼진 빌드는 여기서 오류 — 조용히 syn으로 떨어지면
/// --semantic이 받은 결과가 의미 해석이 아니게 되어 거짓이 된다.
fn require_semantic_build(opts: &Options) -> Result<(), String> {
    if opts.semantic && !cfg!(feature = "semantic") {
        return Err(
            "--semantic requires a build with the `semantic` feature (cargo build --features semantic)"
                .to_string(),
        );
    }
    Ok(())
}

/// 이미 읽은 메타데이터로 [`load`]와 같은 문서를 만든다 — deps 보고서처럼 메타데이터를
/// 따로 쓰는 호출자가 한 실행에서 `cargo metadata`를 두 번 돌리지 않게 한다.
pub fn load_with(dir: &Path, meta: &Metadata, opts: &Options) -> Result<Document, String> {
    load_inner(dir, meta, opts, false).map(|(doc, _)| doc)
}

/// [`load`]와 같은 문서와, 그 문서를 만든 수확이 읽은 입력을 돌려준다 — 문서를 파일로
/// 쓰는 명령이 입력 보호 목록을 위해 워크스페이스를 다시 수확하지 않게 한다.
/// semantic 캐시가 적중하면 수확이 없어 입력은 None이다(호출자가 [`input_files`]로 찾는다).
pub(crate) fn load_with_inputs(
    dir: &Path,
    opts: &Options,
) -> Result<(Document, Option<HarvestInputs>), String> {
    require_semantic_build(opts)?;
    let meta = cargo_meta::load(dir)?;
    load_inner(dir, &meta, opts, true)
}

/// 캐시를 거친 수확 — `want_inputs`면 수확이 읽은 입력도 함께 돌려준다.
fn load_inner(
    dir: &Path,
    meta: &Metadata,
    opts: &Options,
    want_inputs: bool,
) -> Result<(Document, Option<HarvestInputs>), String> {
    require_semantic_build(opts)?;
    // 캐시 키는 모든 입력의 지문 — 지문을 못 재면 캐시를 끈다(실패는 부가적).
    let cache = if opts.semantic && opts.cache {
        fingerprint(dir, meta, opts).map(|key| (key, cache_path(meta)))
    } else {
        None
    };
    if let Some((key, path)) = &cache {
        if let Some(doc) = read_cache(path, *key, &meta.workspace_root) {
            return Ok((doc, None));
        }
    }
    let mut sink = Sink {
        want_edges: true,
        want_inputs,
        ..Default::default()
    };
    let (doc, uses_include) = harvest(dir, meta, opts, Some(&mut sink))?;
    if let Some((key, path)) = &cache {
        // 지문이 못 보는 입력을 소비한 문서는 캐시에 쓰지 않는다 —
        // include!/OUT_DIR 생성 파일과 루트 밖 #[path] 파일, 지문이
        // 건너뛰는 숨김 디렉터리 아래 정점은 stale을 만든다.
        if !uses_include && !has_uncovered_input(&doc, &meta.workspace_root) {
            write_cache(path, *key, &doc);
        }
    }
    Ok((doc, sink.inputs))
}

/// syn 심볼 그래프와 정점별 소스 범위를 함께 돌려준다 — 소스 위치를 impact와
/// 같은 정점 ID로 귀속하려는 라이브러리 호출자용이다.
/// 기본 옵션(심볼 레벨, 외부 크레이트 정점 없음, semantic 없음)으로
/// 수확한다 — 정점 집합은 `impact`/`reach` 기본 실행과 같다. 간선까지 만든
/// 완전한 문서다. 정점 ID만 쓰는 `schema`는 간선 해석을 건너뛰는 같은 수확을 쓴다.
pub fn owner_spans(
    dir: &Path,
    meta: &Metadata,
) -> Result<(Document, Vec<harvest::OwnerSpan>), String> {
    let mut sink = Sink {
        spans: Some(Vec::new()),
        want_edges: true,
        ..Default::default()
    };
    let (doc, _) = harvest(dir, meta, &span_options(), Some(&mut sink))?;
    Ok((doc, sink.spans.unwrap_or_default()))
}

/// 정점 ID 집합과 정점별 소스 범위만 돌려준다 — `schema`가 relation-use 사실을
/// impact와 같은 정점 ID로 귀속하는 데 쓴다. 간선은 해석하지 않는다: 본문 이름
/// 팬아웃 간선이 대형 저장소에서 이 수확 시간·메모리의 대부분인데 귀속에는 정점과
/// 범위만 필요하다. 정점·범위는 1패스가 다 만들므로 [`owner_spans`]와 같다.
pub(crate) fn vertex_spans(
    dir: &Path,
    meta: &Metadata,
) -> Result<(BTreeSet<String>, Vec<harvest::OwnerSpan>), String> {
    let mut sink = Sink {
        spans: Some(Vec::new()),
        want_edges: false,
        ..Default::default()
    };
    let (doc, _) = harvest(dir, meta, &span_options(), Some(&mut sink))?;
    Ok((vertex_id_set(doc), sink.spans.unwrap_or_default()))
}

/// 정점 범위를 쓰는 수확의 옵션 — 심볼 레벨, 외부 크레이트 정점 없음, semantic 없음.
/// 정점 집합이 `impact`/`reach` 기본 실행과 같아야 사실의 usr가 그래프 정점이 된다.
fn span_options() -> Options {
    Options {
        symbol_level: true,
        ..Default::default()
    }
}

/// 간선 없이 만든 문서에서 정점 ID만 남긴다 — 간선이 빈 문서가 완전한 그래프처럼
/// 다른 질의로 흘러가지 않게 호출자에게는 ID 집합만 넘긴다.
fn vertex_id_set(doc: Document) -> BTreeSet<String> {
    doc.vertices.into_iter().map(|v| v.id).collect()
}

/// 수확 부산물을 받는 그릇 — 필요한 칸만 Some으로 채워 넘긴다.
///
/// `schema`는 정점 범위만, `routes`는 모듈 트리와 AST 아레나까지 받는다. 라우트
/// 추출이 핸들러 경로를 `impact`와 같은 정점 ID로 해석하려면 같은 수확의 이름
/// 표가 필요하다 — 따로 다시 만들면 ID 규칙이 갈라진다.
#[derive(Default)]
struct Sink {
    spans: Option<Vec<harvest::OwnerSpan>>,
    tree: Option<ModTree>,
    arena: Option<Arena>,
    want_tree: bool,
    /// 2패스(본문·속성 간선)를 돌릴지 — 정점 ID·범위·트리만 쓰는 호출자는 false다.
    /// sink 유무로 추정하지 않고 명시한다: 간선이 필요한 sink 호출자가 생겼을 때
    /// 조용히 간선 없는 문서를 받지 않게 하려는 것이다.
    want_edges: bool,
    /// 이 수확이 읽은 입력을 `inputs`로 넘길지 — 문서를 파일로 쓰는 호출자만 true다.
    want_inputs: bool,
    inputs: Option<HarvestInputs>,
}

/// `routes`가 쓰는 수확 산출물 — 정점 ID·정점 범위·모듈 트리·AST 아레나.
/// 간선은 만들지 않으므로 문서 대신 정점 ID 집합만 싣는다.
pub(crate) struct Parts {
    pub(crate) vertex_ids: BTreeSet<String>,
    pub(crate) spans: Vec<harvest::OwnerSpan>,
    pub(crate) tree: ModTree,
    arena: Arena,
}

impl Parts {
    /// 모듈 하나의 (선언 파일, 아이템 목록) 묶음 — 인라인 모듈과 lib/bin 합본
    /// 루트를 수확과 같은 규칙으로 푼다.
    pub(crate) fn module_items(&self, module: &str) -> Vec<(PathBuf, &'static [syn::Item])> {
        module_items(&self.tree, &self.arena, module).unwrap_or_default()
    }
}

/// 기본 옵션(심볼 레벨)으로 수확하고 트리·아레나까지 돌려준다. 라우트 추출은
/// 간선을 읽지 않으므로 간선 해석을 건너뛴다.
pub(crate) fn harvest_parts(dir: &Path, meta: &Metadata) -> Result<Parts, String> {
    let mut sink = Sink {
        spans: Some(Vec::new()),
        want_tree: true,
        want_edges: false,
        ..Default::default()
    };
    let (doc, _) = harvest(dir, meta, &span_options(), Some(&mut sink))?;
    let (Some(tree), Some(arena)) = (sink.tree, sink.arena) else {
        return Err("internal error: harvest did not return its module tree".to_string());
    };
    Ok(Parts {
        vertex_ids: vertex_id_set(doc),
        spans: sink.spans.unwrap_or_default(),
        tree,
        arena,
    })
}

/// 수확이 읽은 입력 — 산출물이 덮어쓰면 안 되는 파일 목록의 재료다. 문서를 만든 수확이
/// 이것을 넘기면 `--out`이 보호 목록을 위해 워크스페이스를 다시 수확하지 않는다
/// (cargo metadata·전체 파싱을 한 번 더 해 대형 저장소에서 `--out` 시간이 두 배였다).
pub(crate) struct HarvestInputs {
    arena: Arena,
    orphan_files: Vec<PathBuf>,
    /// 읽거나 파싱하지 못한 모듈 파일 — 끝까지 간 수확에는 없다(수확은 그런 파일에서
    /// 실패한다). 관대 모드로 트리만 다시 찾은 [`input_files`]만 채운다.
    unloaded: Vec<PathBuf>,
    workspace_root: PathBuf,
}

/// `--out` 입력 보호 목록 — 덮어쓰면 안 되는 파일과, 목록에 빠진 입력이 있을 수 있는
/// 이유다. 이유가 있으면 호출자가 기존 출력 교체를 보수적으로 거부한다.
pub(crate) struct ProtectedInputs {
    /// 산출물이 덮어쓰면 안 되는 파일.
    pub(crate) files: Vec<PathBuf>,
    /// 리터럴로 풀지 못한 include 경로가 있다 — 그 대상은 목록에 없다.
    pub(crate) unresolved_includes: bool,
    /// 읽거나 파싱하지 못한 모듈 파일(경로순) — 파일은 목록에 있지만 그 파일이 선언한
    /// 하위 모듈·include 입력은 없다.
    pub(crate) unloaded_modules: Vec<PathBuf>,
}

impl HarvestInputs {
    /// 산출물 경로가 실제 수확 입력을 덮어쓰지 않도록 보호 목록을 만든다. 모듈·사용자 지정
    /// 타깃은 수확 아레나를, include 계열은 별도 경로 검사를 쓴다.
    pub(crate) fn protected_files(&self) -> ProtectedInputs {
        let mut files: BTreeSet<PathBuf> = self.arena.keys().cloned().collect();
        let unresolved_includes = include_inputs(&self.arena, &mut files);
        files.extend(self.orphan_files.iter().cloned());
        files.extend(self.unloaded.iter().cloned());
        let manifests = manifests_above(
            files
                .iter()
                .chain(std::iter::once(&self.workspace_root.join("Cargo.toml"))),
        );
        files.extend(manifests);
        ProtectedInputs {
            files: files.into_iter().collect(),
            unresolved_includes,
            unloaded_modules: self.unloaded.clone(),
        }
    }
}

/// 파일들의 조상 디렉터리에 있는 매니페스트·설정 파일 — 수확 입력이다. 디렉터리마다 한
/// 번만 본다: 파일마다 조상 전부를 stat하면 깊은 트리에서 파일 수 × 깊이 × 4번이다.
/// 이미 본 디렉터리에서 멈춰도 된다 — 처음 볼 때 그 위 조상까지 이미 다 봤다.
fn manifests_above<'a>(paths: impl Iterator<Item = &'a PathBuf>) -> Vec<PathBuf> {
    let mut seen: BTreeSet<&Path> = BTreeSet::new();
    let mut found = Vec::new();
    for path in paths {
        for parent in path.ancestors().skip(1) {
            if !seen.insert(parent) {
                break;
            }
            #[cfg(test)]
            PROBED_DIRS.with(|c| c.set(c.get() + 1));
            for name in [
                "Cargo.toml",
                "Cargo.lock",
                ".rustograph.yml",
                ".rustograph.yaml",
            ] {
                let input = parent.join(name);
                if input.is_file() {
                    found.push(input);
                }
            }
        }
    }
    found
}

/// 문서를 만든 수확의 입력을 넘겨받지 못한 `--out`(schema·routes, semantic 캐시 적중)이
/// 보호 목록을 다시 만든다. 모듈 트리만 키운다 — 이름 해석·정점 수확은 보호 목록에 쓰이지
/// 않는다. 파일 목록은 [`HarvestInputs::protected_files`]와 같다. 읽거나 파싱하지 못한
/// 모듈 파일은 오류가 아니라 `unloaded_modules`다 — 그런 파일을 limitation으로 세고 문서를
/// 내는 명령(`schema`)이 출력 대상에 따라 성공·실패가 갈리지 않게 한다.
pub(crate) fn input_files(dir: &Path) -> Result<ProtectedInputs, String> {
    let metadata = cargo_meta::load(dir)?;
    let found = input_tree(&metadata, true)?;
    let inputs = HarvestInputs {
        arena: found.arena,
        orphan_files: found.tree.orphan_files,
        unloaded: found.unloaded.into_iter().flatten().collect(),
        workspace_root: metadata.workspace_root,
    };
    Ok(inputs.protected_files())
}

/// 매크로 확장을 추정하지 않고 리터럴 include 입력만 보호한다.
fn include_inputs(arena: &Arena, files: &mut BTreeSet<PathBuf>) -> bool {
    use syn::visit::Visit;
    #[derive(Default)]
    struct Includes {
        paths: Vec<(String, bool)>,
        unresolved: bool,
    }
    impl<'ast> Visit<'ast> for Includes {
        fn visit_macro(&mut self, mac: &'ast syn::Macro) {
            let Some(name) = mac
                .path
                .segments
                .last()
                .map(|segment| segment.ident.to_string())
            else {
                return;
            };
            if matches!(name.as_str(), "include" | "include_str" | "include_bytes") {
                match syn::parse2::<syn::LitStr>(mac.tokens.clone()) {
                    Ok(path) => self.paths.push((path.value(), name == "include")),
                    Err(_) => self.unresolved = true,
                }
            } else if nested_include(mac.tokens.clone()) {
                // macro_rules! 안의 경로는 확장 위치에 의존한다. 읽은 입력인
                // 척하지 않고 기존 산출물 교체를 보수적으로 막는다.
                self.unresolved = true;
            }
        }
    }
    let mut pending = Vec::new();
    let mut unresolved = false;
    for (file, items) in arena {
        let mut includes = Includes::default();
        for item in *items {
            includes.visit_item(item);
        }
        unresolved |= includes.unresolved;
        pending.extend(
            includes
                .paths
                .into_iter()
                .map(|(path, code)| (file.parent().unwrap_or(Path::new(".")).join(path), code)),
        );
    }
    let mut visited = BTreeSet::new();
    while let Some((file, code)) = pending.pop() {
        let file = file.canonicalize().unwrap_or(file);
        files.insert(file.clone());
        if !code || !visited.insert(file.clone()) {
            continue;
        }
        // 리터럴 경로는 분석 대상이 고른다 — 장치·FIFO·거대 파일도 될 수 있다. 못 읽으면
        // 읽은 입력인 척하지 않고 기존 산출물 교체를 보수적으로 막는다(보호 등록은 위에서 끝났다).
        let Ok(text) = read_source_bounded(&file, MAX_SOURCE_BYTES) else {
            unresolved = true;
            continue;
        };
        let mut includes = Includes::default();
        if let Ok(parsed) = syn::parse_file(&text) {
            includes.visit_file(&parsed);
        } else if let Ok(parsed) = syn::parse_str::<syn::Expr>(&text) {
            includes.visit_expr(&parsed);
        } else {
            unresolved = true;
        }
        unresolved |= includes.unresolved;
        pending.extend(
            includes
                .paths
                .into_iter()
                .map(|(path, code)| (file.parent().unwrap_or(Path::new(".")).join(path), code)),
        );
    }
    unresolved
}

/// 매크로 토큰 안의 include 호출은 syn 방문자가 확장하지 않는다.
fn nested_include(tokens: proc_macro2::TokenStream) -> bool {
    let tokens: Vec<_> = tokens.into_iter().collect();
    tokens.windows(2).any(|pair| {
        matches!(&pair[0], proc_macro2::TokenTree::Ident(name) if matches!(name.to_string().as_str(), "include" | "include_str" | "include_bytes"))
            && matches!(&pair[1], proc_macro2::TokenTree::Punct(punct) if punct.as_char() == '!')
    }) || tokens.into_iter().any(|token| match token {
        proc_macro2::TokenTree::Group(group) => nested_include(group.stream()),
        _ => false,
    })
}

/// `--target`의 cfg 팩트 — `rustc --print cfg` 실측이 권위다.
/// rustc를 못 쓰거나 트리플을 모르면 트리플 추정으로 폴백한다.
/// 어느 쪽이든 프로필·feature·커스텀 --cfg 조건은 미지로 남는다.
pub fn target_facts(triple: &str) -> crate::cfgeval::Facts {
    if let Some(lines) = cargo_meta::rustc_cfg_lines(triple) {
        crate::cfgeval::Facts::from_cfg_lines(lines.iter().map(String::as_str))
    } else {
        crate::cfgeval::Facts::from_triple(triple)
    }
}

/// 문서 정점이 지문이 안 보는 파일을 가리키는가 — `#[path]`로
/// `../shared.rs`처럼 루트를 벗어난 파일, `OUT_DIR`/`include!`로 로드된
/// `target/` 아래 생성 파일, 지문 스캔이 건너뛰는 숨김 디렉터리 아래
/// 파일이면 지문이 바뀌지 않아 캐시가 stale해진다. 그런 문서는 캐시에
/// 쓰지 않는다 — 쓰기와 읽기 양쪽에서 검사해 구 스키마 엔트리도 걸러낸다.
/// 정점 대부분이 몇 안 되는 파일을 공유하므로 위치의 파일 부분을 모아 고유 파일만
/// 검사한다 — 정점마다 canonicalize하면 큰 저장소의 캐시 적중 경로가 이 검사에 초 단위를 쓴다.
fn has_uncovered_input(doc: &Document, root: &Path) -> bool {
    let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let files: BTreeSet<&str> = doc
        .vertices
        .iter()
        .filter_map(|v| v.position.as_deref()?.rsplit_once(':').map(|(f, _)| f))
        .collect();
    files
        .into_iter()
        .any(|f| !fingerprint_covers(Path::new(f), &root))
}

/// 지문 스캔이 이 파일을 보는가 — fingerprint()의 순회 규칙과 같다:
/// 루트 안, target/ 아님, .cargo 외 숨김 디렉터리 아님.
fn fingerprint_covers(file: &Path, root: &Path) -> bool {
    #[cfg(test)]
    COVERAGE_PROBES.with(|c| c.set(c.get() + 1));
    let c = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    let Ok(rel) = c.strip_prefix(root) else {
        return false;
    };
    rel.components().all(|comp| {
        let std::path::Component::Normal(name) = comp else {
            return true;
        };
        let name = name.to_str().unwrap_or("");
        name != "target" && (!name.starts_with('.') || name == ".cargo")
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
    // v2: 커버리지 규칙 강화(루트 밖·target/·숨김 디렉터리·include!) —
    // v1 아래 쓰인 엔트리는 새 규칙에서 불법일 수 있어 키가 다르다.
    feed(b"rustograph-cache-v2");
    feed(env!("CARGO_PKG_VERSION").as_bytes());
    feed(&graph::DOCUMENT_VERSION.to_le_bytes());
    // 툴체인과 빌드 env도 수확 입력이다 — rustc/ra_ap 버전과
    // RUSTFLAGS가 바뀌면 cfg 평가·의미 해석이 달라진다. 프로브
    // 실패는 지문 불가 — 캐시 없는 경로로 돌아간다.
    feed(cargo_meta::rustc_version()?.as_bytes());
    // 메타데이터 단계의 limitation(resolve 없음, 쓰지 않은 rust-toolchain 파일)은 문서에 실린다 —
    // 키에 넣어 다른 실행(다른 작업 디렉터리)의 캐시 문서가 그 사실을 잘못 싣거나 빠뜨리지 않게
    // 한다. 없으면 아무것도 넣지 않아 기존 키가 그대로다.
    for note in &meta.limitations {
        feed(note.as_bytes());
        feed(&[0]);
    }
    for var in ["RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS"] {
        feed(var.as_bytes());
        feed(std::env::var(var).unwrap_or_default().as_bytes());
    }
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
            // file_type은 심볼릭 링크를 따라가지 않는다 — 링크된 디렉터리를
            // 걸으면 순환이 무한 루프다. 링크 자체는 건너뛰고, 그 안의
            // 파일을 쓰는 수확은 정점 위치가 커버리지 검사에 걸려
            // 캐시를 끈다.
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name != "target" && (!name.starts_with('.') || name == ".cargo") {
                    stack.push(p);
                }
                continue;
            }
            // .cargo/config[.toml]도 빌드 입력이다 — RUSTFLAGS·--cfg·
            // rustflags를 바꾸면 cfg 평가와 수확 결과가 달라진다.
            let under_cargo_cfg = p
                .parent()
                .and_then(|d| d.file_name())
                .and_then(|n| n.to_str())
                .is_some_and(|n| n == ".cargo");
            let interesting = p.extension().is_some_and(|x| x == "rs")
                || matches!(
                    p.file_name().and_then(|n| n.to_str()),
                    Some("Cargo.toml" | "Cargo.lock")
                )
                || (under_cargo_cfg
                    && matches!(
                        p.file_name().and_then(|n| n.to_str()),
                        Some("config.toml" | "config")
                    ));
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
/// 읽을 때도 커버리지 규칙을 검사한다 — 예전 스키마로 쓰인 엔트리가
/// 새 규칙에서 불법일 수 있다(스키마 마커와 이중 방어).
fn read_cache(path: &Path, key: u64, root: &Path) -> Option<Document> {
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
    if has_uncovered_input(&cf.document, root) {
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

#[cfg(test)]
thread_local! {
    /// 2패스가 간선을 해석한 본문·속성 참조 수 — 정점 범위·모듈 트리만 쓰는 수확이 버릴
    /// 간선을 만들지 않는지 단위 테스트가 센다(테스트 스레드마다 따로).
    static LINKED_ITEMS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// 테스트용 — 이 스레드에서 지금까지 2패스가 간선을 해석한 본문·속성 참조 수.
#[cfg(test)]
fn linked_items() -> usize {
    LINKED_ITEMS.with(std::cell::Cell::get)
}

#[cfg(test)]
thread_local! {
    /// parse_into가 실제로 파싱한 파일 수 — `--out`이 입력 보호 목록을 만들려고 같은
    /// 워크스페이스를 다시 파싱하지 않는지 단위 테스트가 센다(테스트 스레드마다 따로).
    static PARSED_FILES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// 테스트용 — 이 스레드에서 지금까지 parse_into가 파싱한 파일 수.
#[cfg(test)]
pub(crate) fn parsed_files() -> usize {
    PARSED_FILES.with(std::cell::Cell::get)
}

#[cfg(test)]
thread_local! {
    /// 모듈 정점 규칙으로 생성 코드 마커를 판정한 횟수 — 모듈마다가 아니라 파싱한 파일마다
    /// 한 번인지 단위 테스트가 센다(테스트 스레드마다 따로).
    static MODULE_MARKER_SCANS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
thread_local! {
    /// 캐시 커버리지 검사가 canonicalize한 파일 수 — 정점마다가 아니라 고유 파일마다
    /// 한 번만 재는지 단위 테스트가 센다(테스트 스레드마다 따로).
    static COVERAGE_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
thread_local! {
    /// 매니페스트 이름을 확인한 조상 디렉터리 수 — 보호 목록이 디렉터리마다 한 번만
    /// stat하는지 단위 테스트가 센다(테스트 스레드마다 따로).
    static PROBED_DIRS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// 실제 수확 — 메타데이터 위에서 모듈 트리·간선을 조립한다.
/// (문서, include! 계열 매크로 사용 여부)를 돌린다 — include!는
/// 정점 위치에 안 나타나는 입력이라 캐시 판정이 별도로 필요하다.
/// `sink`가 주어지면 정점별 소스 범위(OwnerSpan)와 모듈 트리·아레나, 읽은 입력도 넘긴다.
/// sink가 간선을 원하지 않으면(`want_edges = false`) 2패스(본문·속성 간선)를 건너뛴다 —
/// 그 문서에는 간선과 2패스가 센 limitation이 없어 정점 ID만 믿을 수 있다.
fn harvest(
    dir: &Path,
    meta: &Metadata,
    opts: &Options,
    mut sink: Option<&mut Sink>,
) -> Result<(Document, bool), String> {
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
            // 멤버는 lib 루트 정점부터 걷는다 — lib 타깃 이름이 정점 ID다.
            // lib이 없는 멤버(proc-macro 타깃만 있거나 순수 bin 패키지)는
            // extern crate로 링크할 수 없으니 크레이트 정점으로 붕괴한다 —
            // bin-only면 bin 루트 정점, lib/bin이 전혀 없으면 패키지 정점.
            // 외부 패키지 정점은 항상 패키지 이름이다(emit_crate_level).
            let (vertex, member) = if to_pkg.workspace_member {
                match to_pkg.targets.iter().find(|t| t.kind == "lib") {
                    Some(t) => (t.name.clone(), true),
                    None => (to_pkg.crate_vertex(), false),
                }
            } else {
                (to_pkg.name.clone(), false)
            };
            // 의존 선언은 패키지의 수확되는 타깃(lib·bin)에 적용된다.
            // example/test/bench는 수확 범위 밖이라 스코프를 만들면 같은
            // 이름의 진짜 루트 스코프를 덮어쓸 수 있다.
            for t in &from_pkg.targets {
                if !matches!(t.kind.as_str(), "lib" | "bin") {
                    continue;
                }
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

    let InputTree {
        mut tree,
        arena,
        markers,
        conditional_mods,
        uses_include,
        ..
    } = input_tree(meta, false)?;
    for t in source_targets(meta).filter(|t| t.kind == "bin") {
        entry_roots.push(format!("{}::main", t.name));
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
    push_skipped_mods(&tree.skipped_mods, &mut limitations);

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
    // 지역명이 fn owner_spans와 같으면 이름 해석이 함수 정점으로 읽어 거짓 순환이 된다.
    let mut item_spans: Vec<harvest::OwnerSpan> = Vec::new();
    for mp in tree.modules.keys().cloned().collect::<Vec<_>>() {
        let mh = harvest_module(
            &mut tree,
            &arena,
            &markers,
            &mp,
            &mut harvest,
            &dep_vertices,
        );
        vertices.push(mh.vertex);
        edges.extend(mh.edges);
        vertices.extend(mh.decls.vertices);
        entry_roots.extend(mh.decls.entry_roots);
        test_roots.extend(mh.decls.test_roots);
        bodies.extend(mh.decls.bodies);
        attr_refs.extend(mh.decls.attr_refs);
        impls.extend(mh.decls.impls);
        item_spans.extend(mh.decls.owner_spans);
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
    if let Some(out) = sink.as_deref_mut().and_then(|k| k.spans.as_mut()) {
        collect_owner_spans(&tree, &impls, &bodies, item_spans, out);
    }

    // 2패스 전체(메서드 색인·본문·속성 간선)는 간선을 쓰는 수확만 돈다 — 정점 ID·범위·
    // 트리만 쓰는 sink(schema·routes)에는 버려질 일이라 건너뛴다.
    let want_edges = sink.as_ref().is_none_or(|k| k.want_edges);
    if want_edges {
        #[cfg(test)]
        LINKED_ITEMS.with(|c| c.set(c.get() + bodies.len() + attr_refs.len()));
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
    }

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
    if let Some(k) = sink {
        if k.want_inputs {
            k.inputs = Some(HarvestInputs {
                arena: arena.clone(),
                orphan_files: tree.orphan_files.clone(),
                unloaded: Vec::new(),
                workspace_root: meta.workspace_root.clone(),
            });
        }
        if k.want_tree {
            k.tree = Some(tree);
            k.arena = Some(arena);
        }
    }
    Ok((
        graph::document(
            level,
            dir.display().to_string(),
            Some(meta.workspace_root.display().to_string()),
            root_set.into_iter().collect(),
            vertices,
            edges,
            limitations,
        ),
        uses_include,
    ))
}

/// 정점별 소스 범위를 모은다 — 본문 아이템(fn·메서드·const·static·
/// 트레이트 기본 메서드), 본문 없는 소유 아이템, 그리고 self 타입이
/// 해석된 impl 블록(메서드 밖 연관 아이템을 self 타입에 귀속)이다.
fn collect_owner_spans(
    tree: &ModTree,
    impls: &[harvest::ImplBlock],
    bodies: &[BodyItem<'_>],
    mut spans: Vec<harvest::OwnerSpan>,
    out: &mut Vec<harvest::OwnerSpan>,
) {
    for block in impls {
        if let Some(self_id) = tree.resolve(
            &block.items_module,
            &block.self_ty,
            &modtree::DepCrates::new(),
        ) {
            spans.push(harvest::OwnerSpan {
                id: self_id,
                file: block.file.clone(),
                range: block.range.clone(),
            });
        }
    }
    spans.extend(bodies.iter().map(|b| harvest::OwnerSpan {
        id: b.id.clone(),
        file: b.file.clone(),
        range: b.range.clone(),
    }));
    *out = spans;
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
    // depends 간선의 양 끝은 *정점* ID여야 한다 — 멤버의 정점은 lib/bin
    // 타깃 이름이지 패키지 이름이 아니니, 패키지명→정점 ID를 매핑해 둔다.
    // 패키지명을 그대로 쓰면 `[lib] name`이 다른 멤버에서 dangling이 생긴다.
    let mut present: BTreeMap<String, String> = BTreeMap::new();
    for p in &meta.packages {
        // 워크스페이스 멤버는 타깃 루트 모듈이 크레이트 정점을 겸한다 —
        // rustc에서 타깃이 곧 크레이트이고, 별도 정점은 패키지명==타깃명일 때
        // 충돌한다.
        if p.workspace_member {
            let vertex = p.crate_vertex();
            // lib/bin 타깃이 없는 멤버(proc-macro 크레이트 등)는 겸임할 루트
            // 모듈이 없다 — depends 간선이 dangling하지 않게 정점을 만든다.
            if p.targets
                .iter()
                .all(|t| !matches!(t.kind.as_str(), "lib" | "bin"))
            {
                vertices.push(Vertex {
                    id: vertex.clone(),
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
            present.insert(p.name.clone(), vertex);
            continue;
        }
        if !include_deps {
            skipped += 1;
            continue;
        }
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
        present.insert(p.name.clone(), p.name.clone());
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
            present.get(meta.packages[*f].name.as_str()),
            present.get(meta.packages[*t].name.as_str()),
        );
        let (Some(fp), Some(tp)) = (fp, tp) else {
            continue;
        };
        if !d.kind.is_empty() {
            dev_edges += 1;
        }
        edges.push(Edge::new(fp.clone(), tp.clone(), EdgeKind::Depends));
    }
    if dev_edges > 0 {
        limitations.push(format!("{dev_edges} dev/build dependency edges included"));
    }
}

/// 파일 머리(앞 10줄)의 생성 코드 마커 판정 — 파싱할 때 읽은 원문으로 파일마다 한 번 낸다.
/// 인라인 모듈은 담은 파일의 판정을 쓴다: 모듈마다 파일을 다시 통째로 읽으면 인라인 모듈이
/// 수천 개인 생성 파일(prost 출력)에서 (모듈 수 × 파일 크기)를 읽었다. 모듈 정점과 아이템
/// 정점의 규칙이 서로 달라 둘 다 둔다 — 합치면 기존 generated 플래그가 바뀐다.
#[derive(Clone, Copy, Default)]
struct FileMarkers {
    /// 모듈 정점의 generated — [`text_has_marker`] 규칙.
    module: bool,
    /// 아이템·impl 메서드 정점의 generated — harvest의 규칙.
    items: bool,
}

/// 수확 입력 — 워크스페이스 멤버의 모듈 트리와 그 파일들의 AST 아레나다.
/// 수확과 `--out` 입력 보호가 [`input_tree`] 하나로 만들어, 보호 목록이 수확이 실제로
/// 읽은 파일과 갈라지지 않는다.
#[derive(Default)]
struct InputTree {
    /// 전 워크스페이스가 한 트리 — 크레이트 간 `othercrate::x` 해석이 가능해야 한다.
    tree: ModTree,
    arena: Arena,
    /// 아레나 파일마다의 생성 코드 마커 — 파싱할 때 읽은 원문으로 판정해 둔다.
    markers: BTreeMap<PathBuf, FileMarkers>,
    /// `#[cfg]` 뒤의 mod 선언 수 — 수확이 limitation으로 올린다.
    conditional_mods: usize,
    /// 수확 파일 중 include! 계열 매크로를 쓰는 파일이 있는가 — 캐시 판정에 쓴다.
    uses_include: bool,
    /// 읽거나 파싱하지 못한 모듈 파일 — Some이면 관대 모드다. 수확(None)은 그런 파일에서
    /// 오류로 끝나지만, `--out` 입력 보호는 파일을 적고 나머지 트리를 계속 찾는다. `schema`는
    /// 그런 파일을 unparsed-sources로 세고 문서를 내는데, 보호 목록 수집이 실패하면 `--out`일
    /// 때만 종료 2가 됐다.
    unloaded: Option<BTreeSet<PathBuf>>,
}

impl InputTree {
    /// 모듈 파일을 싣지 못한 오류를 처리한다 — 엄격 모드는 오류를 그대로 돌려주고, 관대
    /// 모드는 파일을 적고 넘어간다(그 파일이 선언한 하위 모듈은 트리에 실리지 않는다).
    fn tolerate(&mut self, file: &Path, error: String) -> Result<(), String> {
        match self.unloaded.as_mut() {
            Some(unloaded) => {
                unloaded.insert(file.to_path_buf());
                Ok(())
            }
            None => Err(error),
        }
    }
}

/// 수확 대상 타깃 — 워크스페이스 멤버의 lib·bin이다. example/test/bench 타깃은
/// 수확 범위 밖이다(루트가 아님).
fn source_targets(meta: &Metadata) -> impl Iterator<Item = &cargo_meta::Target> {
    meta.packages
        .iter()
        .filter(|p| p.workspace_member)
        .flat_map(|p| &p.targets)
        .filter(|t| matches!(t.kind.as_str(), "lib" | "bin"))
}

/// 수확 대상 타깃마다 mod 선언을 따라 모듈 트리를 키우고, 어느 선언에도 닿지 않는
/// .rs를 orphan으로 센다. 이름 해석·정점 수확은 하지 않는다 — `--out` 입력 보호는
/// 읽은 파일만 알면 된다. `tolerant`가 false면 오류는 grow_tree가 낸 그대로고, true면
/// 싣지 못한 모듈 파일을 `unloaded`에 적는다.
fn input_tree(meta: &Metadata, tolerant: bool) -> Result<InputTree, String> {
    let mut found = InputTree {
        unloaded: tolerant.then(BTreeSet::new),
        ..Default::default()
    };
    let mut scan_roots: BTreeSet<PathBuf> = BTreeSet::new();
    for t in source_targets(meta) {
        if let Ok(f) = t.src.canonicalize() {
            if let Some(d) = f.parent() {
                scan_roots.insert(d.to_path_buf());
            }
        }
        grow_tree(&mut found, &t.name, &t.src)?;
    }
    // orphan .rs — 전 타깃 처리 후에 스캔한다. lib의 스캔이 bin 병합 전에
    // 돌면 main.rs가 orphan으로 오인된다. known은 공유 루트의 extra_files까지.
    let known: BTreeSet<PathBuf> = found
        .tree
        .modules
        .values()
        .flat_map(|m| std::iter::once(m.file.clone()).chain(m.extra_files.iter().cloned()))
        .collect();
    for root in scan_roots {
        scan_orphans(&root, &known, &mut found.tree);
    }
    Ok(found)
}

/// 타깃의 모듈 트리를 키운다 — mod 선언을 따라 파일을 발견한다.
/// `found.uses_include`는 수확 파일 중 하나라도 include! 계열 매크로를
/// 쓰면 true가 된다 — 그 입력은 정점 위치에 안 나타나 지문이 못 본다.
/// 싣지 못한 파일은 [`InputTree::tolerate`]가 모드에 따라 오류로 내거나 적는다.
fn grow_tree(found: &mut InputTree, root_path: &str, root_file: &Path) -> Result<(), String> {
    // 타깃 루트는 cargo metadata가 준 경로라 mod 대상처럼 미리 걸러지지 않는다 — 정규 파일이
    // 아니면(FIFO, `/dev/zero`를 가리키는 링크) 열지 않는다. 링크를 따라간 정규 경로로는 어느
    // 타깃인지 알 수 없어, 판정은 정규화 전 경로로 하고 오류에도 그 경로를 싣는다.
    let checked = require_regular_file(root_file).and_then(|()| root_file.canonicalize());
    let root_file = match checked {
        Ok(file) => file,
        Err(e) => {
            let error = format!("cannot read {}: {e}", root_file.display());
            return found.tolerate(root_file, error);
        }
    };
    match found.tree.modules.get_mut(root_path) {
        // lib와 같은 이름의 bin — 두 크레이트 인스턴스가 루트 경로를 공유한다.
        Some(m) => m.extra_files.push(root_file.clone()),
        None => {
            found.tree.modules.insert(
                root_path.to_string(),
                modtree::Module::new(root_file.clone(), true, true),
            );
        }
    }
    if let Err(e) = parse_into(found, &root_file) {
        found.tolerate(&root_file, e)?;
    }
    let mut queue = vec![root_path.to_string()];
    let mut visited: BTreeSet<String> = BTreeSet::new();
    while let Some(mp) = queue.pop() {
        if !visited.insert(mp.clone()) {
            continue;
        }
        let Some(mut groups) = module_items(&found.tree, &found.arena, &mp) else {
            continue;
        };
        // 루트에서는 이 타깃의 루트 파일에 든 선언만 훑는다. lib과 같은 이름의 bin이 함께 쓰는
        // 루트의 다른 파일 선언은 그 타깃의 grow_tree가 훑는다(타깃 순서와 무관) — 여기서도
        // 훑으면 그 선언과 하위 트리를 이 타깃의 루트 파일을 조상으로 둔 사슬로 잘못 판정한다
        // (거짓 순환).
        if mp == root_path {
            groups.retain(|(file, _)| *file == root_file);
        }
        for sub in modtree::collect_target_submodules(
            Some(&root_file),
            &flatten_items(&groups),
            &mp,
            &mut found.tree,
            &mut found.conditional_mods,
        ) {
            let file = found.tree.modules[&sub].file.clone();
            if found.tree.modules[&sub].file_module {
                if let Err(e) = parse_into(found, &file) {
                    found.tolerate(&file, e)?;
                }
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
            // file_type은 심볼릭 링크를 따라가지 않는다 — 링크된
            // 디렉터리 순환이 무한 루프를 만든다.
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
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

/// 파일을 파싱해 아레나에 넣고, 같은 원문으로 생성 코드 마커를 판정해 둔다. 파싱 실패는
/// limitation이 아니라 오류 — 읽은 파일을 못 읽는 것과 해석 못 하는 것은 다르다…지만
/// 실전에서는 조건부 생성 파일이 깨진 문법을 가질 수 있어 빈 목록으로 둔다.
/// 정규 파일이 아니면 열지 않고 "cannot read" 오류다 — `mod` 대상은 collect_submodules가,
/// 타깃 루트는 grow_tree가 미리 거르지만, stat이 실패해 걸러지지 않은 대상과 검사 뒤에 바뀐
/// 파일도 여기서 막는다. 크기 상한은 두지 않는다(정상인 거대 생성 파일을 오류로 할지
/// limitation으로 할지 정하지 않았다 — 정규 파일은 끝이 있다).
fn parse_into(found: &mut InputTree, file: &Path) -> Result<(), String> {
    if found.arena.contains_key(file) {
        return Ok(());
    }
    #[cfg(test)]
    PARSED_FILES.with(|c| c.set(c.get() + 1));
    let src = require_regular_file(file)
        .and_then(|()| std::fs::read_to_string(file))
        .map_err(|e| format!("cannot read {}: {e}", file.display()))?;
    // include! 계열은 파일 경로 인자를 받아 별도 입력을 읽는다 —
    // 그 입력 파일은 정점 위치에 안 나타나 지문 밖이다.
    if !found.uses_include
        && (src.contains("include!")
            || src.contains("include_str!")
            || src.contains("include_bytes!"))
    {
        found.uses_include = true;
    }
    match syn::parse_file(&src) {
        Ok(ast) => {
            let markers = FileMarkers {
                module: text_has_marker(&src),
                items: harvest::text_has_generated_marker(&src),
            };
            found.markers.insert(file.to_path_buf(), markers);
            found
                .arena
                .insert(file.to_path_buf(), Box::leak(ast.items.into_boxed_slice()));
            Ok(())
        }
        Err(e) => Err(format!("parse error in {}: {e}", file.display())),
    }
}

/// 모듈의 아이템 그룹 — (선언 파일, 그 파일의 아이템 목록) 쌍.
/// 파일 모듈은 파일 AST(루트는 extra_files까지 — 같은 이름의 lib/bin이
/// 루트를 공유할 때 각 아이템의 실제 파일을 보존해야 semantic 엔진이
/// 본문 소유자를 올바른 소스에 맞춘다), 인라인 모듈은 조상의 mod 본문.
fn module_items(
    tree: &ModTree,
    arena: &Arena,
    path: &str,
) -> Option<Vec<(PathBuf, &'static [syn::Item])>> {
    let module = tree.modules.get(path)?;
    if module.file_module {
        let mut out: Vec<(PathBuf, &'static [syn::Item])> = Vec::new();
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
    let mut items: &'static [syn::Item] = arena.get(&tree.modules[&top].file).copied()?;
    // 파일 루트에서 목표까지 인라인 mod 본문을 따라 내려간다.
    let rel = path.strip_prefix(&format!("{top}::"))?;
    for seg in rel.split("::") {
        let mut next: Option<&'static [syn::Item]> = None;
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
/// 생성 코드 마커는 파싱할 때 파일마다 판정해 둔 `markers`에서 찾는다. 모듈 파일과 그룹
/// 파일은 모두 아레나에 있다(엄격 수확은 싣지 못한 파일에서 오류로 끝난다).
fn harvest_module<'a>(
    tree: &mut ModTree,
    arena: &'a Arena,
    markers: &BTreeMap<PathBuf, FileMarkers>,
    mp: &str,
    out: &mut Harvest,
    dep_vertices: &BTreeSet<String>,
) -> ModuleHarvest<'a> {
    let groups = module_items(tree, arena, mp).expect("module items must exist");
    let file = tree.modules[mp].file.clone();
    let krate = modtree::crate_of(mp);
    let markers_of = |f: &Path| markers.get(f).copied().unwrap_or_default();
    // 파라미터 이름이 fn harvest와 충돌하면 ident 경로가 함수 정점으로
    // 해석돼 거짓 간선·사이클이 생긴다 — 지역명은 `out`으로 둔다.
    let decls = harvest::decls_with_markers(mp, &krate, &groups, &|f| markers_of(f).items, out);

    // 모듈 정점 — 타깃 루트는 크레이트 정점을 겸한다(rustc 의미론).
    // 위치는 파일 시작, exported는 `pub mod` 여부를 따른다.
    let is_root = !mp.contains("::");
    let public = tree.modules[mp].public;
    let generated = markers_of(&file).module;
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

/// 원문 앞 10줄에 생성 코드 마커가 있는지 본다 — 모듈 정점의 generated 규칙이다.
/// 아이템 정점의 harvest 규칙과 다르다('do not edit'만 있어도 참, 'autogenerated'는 거짓).
/// 규칙을 합치면 기존 플래그가 바뀌어, 둘 다 두고 [`parse_into`]가 읽은 원문만 같이 쓴다.
fn text_has_marker(src: &str) -> bool {
    #[cfg(test)]
    MODULE_MARKER_SCANS.with(|c| c.set(c.get() + 1));
    src.lines().take(10).any(|l| {
        let l = l.to_lowercase();
        l.contains("do not edit") || l.contains("@generated") || l.contains("auto-generated")
    })
}

/// 정점 없이 건너뛴 `mod` 선언을 사유별 limitation 문장으로 옮긴다 — 셀 것이 없는
/// 사유는 조용하다. 순환 선언을 오류(2)로 만들지 않는 것은 cfg로 막혀 rustc가 받는
/// 크레이트가 있어서다.
fn push_skipped_mods(
    skipped: &BTreeMap<String, modtree::SkippedMod>,
    limitations: &mut Vec<String>,
) {
    for (reason, why) in [
        (
            modtree::SkippedMod::Circular,
            "they re-enter an ancestor module's file (circular modules)".to_string(),
        ),
        (
            modtree::SkippedMod::NotRegularFile,
            "their file is not a regular file (device, FIFO, or directory)".to_string(),
        ),
        (
            modtree::SkippedMod::TooDeep,
            format!(
                "module nesting exceeds {} levels",
                modtree::MAX_MODULE_DEPTH
            ),
        ),
    ] {
        let n = skipped.values().filter(|r| **r == reason).count();
        if n > 0 {
            limitations.push(format!("{n} mod declarations were skipped because {why}"));
        }
    }
}

/// 수확 실측을 limitation 문장으로 옮긴다.
fn push_limitations(h: &Harvest, limitations: &mut Vec<String>) {
    if h.unresolved_paths > 0 {
        limitations.push(format!(
            "{} path references could not be resolved within the workspace (external crates, prelude names, or macro-generated names)",
            h.unresolved_paths
        ));
    }
    if h.unparsed_attrs > 0 {
        limitations.push(format!(
            "{} cfg_attr attribute lists could not be parsed; paths inside them are not counted as references",
            h.unparsed_attrs
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
        let got = read_cache(&path, k2, &tmp).expect("cache hit");
        assert_eq!(got.vertices.len(), doc.vertices.len());
        // 다른 키와 손상된 파일은 None — 새 수확으로 넘어간다.
        assert!(read_cache(&path, k1, &tmp).is_none());
        std::fs::write(&path, "not json").unwrap();
        assert!(read_cache(&path, k2, &tmp).is_none());
        // 캐시 파일이 워크스페이스 안에 있어도 지문을 바꾸지 않는다 —
        // .rustograph는 숨김 디렉터리라 지문 걷기에서 빠진다.
        assert_eq!(k2, fingerprint(&tmp, &meta, &opts).unwrap());
        // 메타데이터 limitation은 문서에 실리므로 키에 들어간다 — 쓰지 않은 rust-toolchain 파일을
        // 알린 실행의 캐시 문서가 그 파일이 없는 실행에 재사용되지 않는다.
        let noted = Metadata {
            packages: vec![],
            by_id: BTreeMap::new(),
            dep_edges: vec![],
            workspace_root: tmp.clone(),
            limitations: vec!["toolchain-file-ignored: example".to_string()],
        };
        assert_ne!(k2, fingerprint(&tmp, &noted, &opts).unwrap());
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// 캐시 커버리지 검사는 정점마다가 아니라 고유 파일마다 한 번 canonicalize한다 — 정점
    /// 대부분이 몇 안 되는 파일을 공유해서, 정점마다 재면 큰 저장소의 캐시 적중 경로가
    /// 지문 검사에만 초 단위를 쓴다. 판정은 그대로다: 지문이 못 보는 파일(루트 밖·target/·
    /// 숨김 디렉터리, `..`로 루트를 벗어나는 경로)을 가리키는 정점이 하나라도 있으면 참이다.
    #[test]
    fn cache_coverage_canonicalizes_each_file_once() {
        let tmp = std::env::temp_dir().join(format!("rustograph-coverage-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let root = tmp.join("ws");
        let covered_files = ["src/lib.rs", "src/a.rs", ".cargo/build.rs"];
        let hidden_files = ["target/debug/gen.rs", ".hidden/x.rs"];
        for rel in covered_files.iter().chain(&hidden_files) {
            let file = root.join(rel);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, "fn f() {}").unwrap();
        }
        std::fs::write(tmp.join("outside.rs"), "fn f() {}").unwrap();
        // 정점 ID 접두사로 정렬 순서를 고정한다 — 커버되는 정점이 모두 앞에 온다.
        let vertex = |id: String, position: Option<String>| Vertex {
            id,
            kind: Kind::Fn,
            krate: "c".to_string(),
            module: "c".to_string(),
            position,
            exported: false,
            generated: false,
            cfg: None,
            unsafe_: false,
        };
        let covered: Vec<Vertex> = (0..300)
            .map(|i| {
                let file = root.join(covered_files[i % covered_files.len()]);
                vertex(
                    format!("c::a{i:04}"),
                    Some(format!("{}:{i}", file.display())),
                )
            })
            .chain([vertex("c::a_crate".to_string(), None)])
            .collect();
        let check = |vertices: Vec<Vertex>| {
            let doc = document(
                Level::Symbol,
                ".".into(),
                None,
                vec![],
                vertices,
                vec![],
                vec![],
            );
            let before = COVERAGE_PROBES.with(std::cell::Cell::get);
            let uncovered = has_uncovered_input(&doc, &root);
            (
                uncovered,
                COVERAGE_PROBES.with(std::cell::Cell::get) - before,
            )
        };
        assert_eq!(check(covered.clone()), (false, covered_files.len()));
        let escaping = root.join("src/../../outside.rs");
        let uncovered_files = [
            tmp.join("outside.rs"),
            escaping,
            root.join(hidden_files[0]),
            root.join(hidden_files[1]),
        ];
        for file in uncovered_files {
            let mut vertices = covered.clone();
            vertices.extend((0..50).map(|i| {
                vertex(
                    format!("c::b{i:04}"),
                    Some(format!("{}:{i}", file.display())),
                )
            }));
            let (uncovered, probes) = check(vertices);
            assert!(
                uncovered,
                "{} is not covered by the fingerprint",
                file.display()
            );
            assert!(
                probes <= covered_files.len() + 1,
                "{probes} canonicalize calls for {} distinct files",
                covered_files.len() + 1
            );
        }
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// semantic 본문 간선은 load가 사이트마다 해석해 둔 선언 정의를 다시 쓴다 — 본문마다 같은
    /// (파일, 범위, 모듈)로 파일 루트부터 다시 찾으면 사이트 탐색이 사이트 수의 두 배가 된다.
    /// 대조: 확정 메서드 호출이 그대로 나온다(재사용한 정의로 본문 소유자를 고르지 못하면
    /// syn 이름 팬아웃의 추정 간선으로 떨어진다).
    #[cfg(feature = "semantic")]
    #[test]
    fn semantic_bodies_reuse_site_definitions_resolved_at_load() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture");
        let (searches_before, sites_before) = sem::site_search_counts();
        let doc = load(
            &fixture,
            &Options {
                symbol_level: true,
                semantic: true,
                ..Default::default()
            },
        )
        .expect("semantic fixture load");
        let (searches, sites) = sem::site_search_counts();
        let (searches, sites) = (searches - searches_before, sites - sites_before);
        assert!(sites > 0, "the fixture registers body sites");
        assert_eq!(
            searches, sites,
            "{searches} site searches for {sites} sites"
        );
        assert!(
            doc.edges.iter().any(|e| e.from == "fixture_app::main"
                && e.to == "fixture_core::Used::<Greet>::greet"
                && e.kind == EdgeKind::Call
                && !e.tentative),
            "the concrete method call must stay a firm semantic edge"
        );
    }

    /// 정점 범위·모듈 트리만 쓰는 명령(schema·routes 서버·클라이언트)과 `--out` 입력 보호는
    /// 본문·속성 간선을 해석하지 않는다 — 대형 저장소에서 버려질 이름 팬아웃 본문 간선이
    /// 그 명령들의 시간·메모리 대부분이었다. 전체 수확은 그대로 해석한다(대조군).
    #[test]
    fn span_only_commands_resolve_no_edges() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
        let resolved_during = |run: &dyn Fn()| {
            let before = linked_items();
            run();
            linked_items() - before
        };
        let full = resolved_during(&|| {
            load(&repo.join("tests/fixture"), &span_options()).unwrap();
        });
        assert!(full > 0, "the full harvest must resolve body edges");
        let commands = [
            (
                "schema",
                resolved_during(&|| {
                    schema::facts(&repo.join("tests/fixture-schema"), "test").unwrap();
                }),
            ),
            (
                "routes server",
                resolved_during(&|| {
                    let dir = repo.join("tests/fixture-routes/axum08");
                    routes::routes(&dir, "test", &Default::default()).unwrap();
                }),
            ),
            (
                "routes client",
                resolved_during(&|| {
                    let dir = repo.join("tests/fixture-client/app");
                    routes::client::client_routes(&dir, "test", &Default::default()).unwrap();
                }),
            ),
            (
                "output inputs",
                resolved_during(&|| {
                    input_files(&repo.join("tests/fixture")).unwrap();
                }),
            ),
        ];
        assert!(commands.iter().all(|(_, n)| *n == 0), "{commands:?}");
    }

    /// 정점 범위 비교 키 — OwnerSpan은 비교 연산이 없어 필드를 그대로 늘어놓는다.
    fn span_keys(spans: &[harvest::OwnerSpan]) -> Vec<(String, PathBuf, std::ops::Range<usize>)> {
        spans
            .iter()
            .map(|s| (s.id.clone(), s.file.clone(), s.range.clone()))
            .collect()
    }

    /// 간선을 건너뛴 수확도 정점 ID·정점 범위·모듈 트리·아레나는 간선까지 만든 수확과
    /// 같다 — schema의 usr와 routes의 핸들러 ID가 impact와 같은 정점을 가리켜야 한다.
    #[test]
    fn edgeless_harvests_keep_vertices_spans_and_tree() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
        for name in [
            "tests/fixture",
            "tests/fixture-schema",
            "tests/fixture-client/app",
        ] {
            let dir = repo.join(name);
            let meta = cargo_meta::load(&dir).unwrap();
            let mut sink = Sink {
                spans: Some(Vec::new()),
                want_tree: true,
                want_edges: true,
                ..Default::default()
            };
            let (full, _) = harvest(&dir, &meta, &span_options(), Some(&mut sink)).unwrap();
            assert!(!full.edges.is_empty(), "{name}");
            let full_ids: BTreeSet<String> = full.vertices.iter().map(|v| v.id.clone()).collect();
            let full_spans = span_keys(sink.spans.as_deref().unwrap());
            let (tree, arena) = (sink.tree.unwrap(), sink.arena.unwrap());
            let (ids, spans) = vertex_spans(&dir, &meta).unwrap();
            assert_eq!(ids, full_ids, "{name}");
            assert_eq!(span_keys(&spans), full_spans, "{name}");
            // 공개 owner_spans는 그대로 간선까지 만든 완전한 문서다.
            let (doc, owners) = owner_spans(&dir, &meta).unwrap();
            let json = |d: &Document| serde_json::to_string(d).unwrap();
            assert_eq!(json(&doc), json(&full), "{name}");
            assert_eq!(span_keys(&owners), full_spans, "{name}");
            let parts = harvest_parts(&dir, &meta).unwrap();
            assert_eq!(parts.vertex_ids, full_ids, "{name}");
            assert_eq!(span_keys(&parts.spans), full_spans, "{name}");
            assert!(parts.tree.modules.keys().eq(tree.modules.keys()), "{name}");
            assert_eq!(parts.tree.orphan_files, tree.orphan_files, "{name}");
            assert!(parts.arena.keys().eq(arena.keys()), "{name}");
        }
    }

    /// 옛 `--out` 보호 목록 — 간선 없는 전체 수확의 아레나·orphan과, 파일마다 조상 디렉터리
    /// 전부의 매니페스트 확인이다. 새 경로가 같은 파일 집합을 내는지 비교하는 오라클이다.
    fn protected_files_by_full_harvest(dir: &Path) -> (Vec<PathBuf>, bool) {
        let metadata = cargo_meta::load(dir).unwrap();
        let parts = harvest_parts(dir, &metadata).unwrap();
        let mut files: BTreeSet<PathBuf> = parts.arena.keys().cloned().collect();
        let unresolved_includes = include_inputs(&parts.arena, &mut files);
        files.extend(parts.tree.orphan_files);
        let root_manifest = metadata.workspace_root.join("Cargo.toml");
        for path in files
            .clone()
            .into_iter()
            .chain(std::iter::once(root_manifest))
        {
            for parent in path.ancestors().skip(1) {
                for name in [
                    "Cargo.toml",
                    "Cargo.lock",
                    ".rustograph.yml",
                    ".rustograph.yaml",
                ] {
                    let input = parent.join(name);
                    if input.is_file() {
                        files.insert(input);
                    }
                }
            }
        }
        (files.into_iter().collect(), unresolved_includes)
    }

    /// `--out` 보호 목록은 문서를 만든 수확의 입력으로 만들든(graph·rules) 모듈 트리만 다시
    /// 키워 만들든(schema·routes) 옛 구현과 같은 파일 집합·include 미해석 표시다. 조상
    /// 디렉터리는 한 번씩만 본다 — 파일마다 조상 전부를 stat하면 깊은 트리에서 수만 번이었다.
    #[test]
    fn output_protection_matches_the_full_harvest_and_probes_each_directory_once() {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
        for name in [
            "tests/fixture",
            "tests/fixture-schema",
            "tests/fixture-client/app",
            "tests/fixture-routes/axum08",
        ] {
            let dir = repo.join(name);
            let expected = protected_files_by_full_harvest(&dir);
            let probed_before = PROBED_DIRS.with(std::cell::Cell::get);
            let rebuilt = input_files(&dir).unwrap();
            let probed = PROBED_DIRS.with(std::cell::Cell::get) - probed_before;
            assert!(rebuilt.unloaded_modules.is_empty(), "{name}");
            assert_eq!(
                (rebuilt.files, rebuilt.unresolved_includes),
                expected,
                "{name}"
            );
            let (_, inputs) = load_with_inputs(&dir, &span_options()).unwrap();
            let reused = inputs
                .expect("a harvest hands over what it read")
                .protected_files();
            assert!(reused.unloaded_modules.is_empty(), "{name}");
            assert_eq!(
                (reused.files, reused.unresolved_includes),
                expected,
                "{name}"
            );
            let directories: BTreeSet<&Path> = expected
                .0
                .iter()
                .flat_map(|path| path.ancestors().skip(1))
                .collect();
            assert_eq!(probed, directories.len(), "{name}");
        }
    }

    /// 테스트용 단일 패키지 크레이트를 만들고 정규화한 루트를 돌려준다. `lib_path`가 있으면
    /// `[lib] path`로 쓴다(없는 파일이어도 cargo metadata는 받는다).
    fn write_crate(
        root: &Path,
        name: &str,
        lib_path: Option<&str>,
        files: &[(&str, &[u8])],
    ) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        let lib = lib_path
            .map(|path| format!("[lib]\npath = \"{path}\"\n"))
            .unwrap_or_default();
        let manifest =
            format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n{lib}");
        std::fs::write(dir.join("Cargo.toml"), manifest).unwrap();
        for (path, text) in files {
            let file = dir.join(path);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }
        dir.canonicalize().unwrap()
    }

    /// 보호 목록 수집은 싣지 못한 모듈 파일(파싱 실패·UTF-8이 아닌 파일·없는 타깃 루트)에서
    /// 멈추지 않는다 — 그 파일을 보호 목록과 unloaded_modules에 두고, 하위 모듈을 못 봐도
    /// orphan으로 잡히는 파일은 보호한다. 같은 입력의 수확(graph 등)은 그대로 오류다.
    #[test]
    fn output_protection_lists_module_files_it_cannot_load() {
        let tmp = std::env::temp_dir().join(format!("rustograph-unloaded-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let broken: [(&str, &[u8]); 3] = [
            ("src/lib.rs", b"pub mod broken;\n"),
            ("src/broken.rs", b"pub mod child;\npub fn oops( {\n"),
            ("src/broken/child.rs", b"pub fn child() {}\n"),
        ];
        let not_utf8: [(&str, &[u8]); 2] = [
            ("src/lib.rs", b"pub mod data;\n"),
            ("src/data.rs", b"pub fn d() {}\n// \xff\n"),
        ];
        let cases = [
            (
                write_crate(&tmp, "parse_failure", None, &broken),
                "src/broken.rs",
                vec!["src/lib.rs", "src/broken/child.rs"],
            ),
            (
                write_crate(&tmp, "not_utf8", None, &not_utf8),
                "src/data.rs",
                vec!["src/lib.rs"],
            ),
            (
                write_crate(&tmp, "missing_root", Some("src/nope.rs"), &[]),
                "src/nope.rs",
                vec![],
            ),
        ];
        for (dir, unloaded, loaded) in cases {
            let listed = input_files(&dir).unwrap();
            assert_eq!(listed.unloaded_modules, [dir.join(unloaded)], "{unloaded}");
            assert!(!listed.unresolved_includes, "{unloaded}");
            for file in std::iter::once(unloaded).chain(loaded) {
                assert!(listed.files.contains(&dir.join(file)), "{unloaded}: {file}");
            }
            assert!(load(&dir, &span_options()).is_err(), "{unloaded}");
        }
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// 상한 읽기는 정규 파일만 열고 상한을 넘으면 거부한다 — FIFO·장치·디렉터리는 열지
    /// 않으니 막히거나 끝없이 읽지 않는다. 상한까지는 그대로 읽는다.
    #[cfg(unix)]
    #[test]
    fn bounded_source_reads_only_regular_files_within_the_limit() {
        let tmp = std::env::temp_dir().join(format!("rustograph-bounded-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        std::fs::create_dir_all(&tmp).unwrap();
        let fits = tmp.join("fits.rs");
        std::fs::write(&fits, "fn a() {}").unwrap();
        assert_eq!(read_source_bounded(&fits, 9).unwrap(), "fn a() {}");
        let over = read_source_bounded(&fits, 8).unwrap_err();
        assert_eq!(over.kind(), std::io::ErrorKind::InvalidData);
        let fifo = tmp.join("pipe.rs");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo must run");
        assert!(made.success());
        for special in [fifo.as_path(), Path::new("/dev/zero"), tmp.as_path()] {
            let err = read_source_bounded(special, MAX_SOURCE_BYTES).unwrap_err();
            assert_eq!(
                err.kind(),
                std::io::ErrorKind::InvalidInput,
                "{}",
                special.display()
            );
        }
        let missing = read_source_bounded(&tmp.join("missing.rs"), MAX_SOURCE_BYTES).unwrap_err();
        assert_eq!(missing.kind(), std::io::ErrorKind::NotFound);
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// 문서에서 `from`이 보내는 call 간선의 (받는 정점, 추정 여부)를 정렬해 돌려준다.
    fn call_targets<'d>(doc: &'d Document, from: &str) -> Vec<(&'d str, bool)> {
        let mut found: Vec<(&str, bool)> = doc
            .edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Call && e.from == from)
            .map(|e| (e.to.as_str(), e.tentative))
            .collect();
        found.sort_unstable();
        found
    }

    /// `Type::assoc()`와 `self.m()`의 타입 범위 후보 접두사 `{타입}::`는 호출마다 한 번
    /// 만든다 — 같은 이름 메서드 후보마다 만들면 `new`·`get`을 정의한 타입이 수천 개인
    /// 워크스페이스에서 할당이 (호출 수 × 같은 이름 정의 수)였다. 해석은 그대로다: 고유
    /// 메서드와 트레이트 impl 메서드(`T::<Tr>::m`)를 함께 잡고, 이름이 접두사인 다른
    /// 타입(`T1`과 `T10`)의 메서드는 잡지 않는다.
    #[test]
    fn type_scoped_calls_build_each_owner_prefix_once() {
        let tmp =
            std::env::temp_dir().join(format!("rustograph-owner-prefix-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let types = 30;
        let mut lib =
            String::from("pub trait Tr {\n    fn new() -> Self;\n    fn get(&self) -> u32;\n}\n");
        for i in 0..types {
            lib.push_str(&format!(
                "pub struct T{i};\nimpl T{i} {{\n    pub fn new() -> Self {{\n        T{i}\n    }}\n    \
                 pub fn get(&self) -> u32 {{\n        {i}\n    }}\n    \
                 pub fn run(&self) -> u32 {{\n        self.get()\n    }}\n}}\n"
            ));
        }
        lib.push_str("impl Tr for T1 {\n    fn new() -> Self {\n        T1\n    }\n");
        lib.push_str("    fn get(&self) -> u32 {\n        1\n    }\n}\n");
        lib.push_str("pub fn make() {\n    T0::new();\n    T1::new();\n    T2::new();\n}\n");
        let dir = write_crate(
            &tmp,
            "owner_prefix",
            None,
            &[("src/lib.rs", lib.as_bytes())],
        );
        let before = harvest::owner_prefixes();
        let doc = load(&dir, &span_options()).unwrap();
        let built = harvest::owner_prefixes() - before;
        // 타입 범위 호출 자리 — `make`의 `Tk::new()` 3곳과 타입마다 `run`의 `self.get()` 1곳.
        let call_sites = 3 + types;
        assert_eq!(
            built, call_sites,
            "{built} owner prefixes for {call_sites} type-scoped calls"
        );
        assert_eq!(
            call_targets(&doc, "owner_prefix::make"),
            [
                ("owner_prefix::T0::new", false),
                ("owner_prefix::T1::<Tr>::new", false),
                ("owner_prefix::T1::new", false),
                ("owner_prefix::T2::new", false),
            ]
        );
        assert_eq!(
            call_targets(&doc, "owner_prefix::T1::run"),
            [
                ("owner_prefix::T1::<Tr>::get", true),
                ("owner_prefix::T1::get", true),
            ]
        );
        assert_eq!(
            call_targets(&doc, "owner_prefix::T10::run"),
            [("owner_prefix::T10::get", true)]
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// 정점 ID의 generated 플래그 — 정점이 없으면 None이다.
    fn generated_flag(doc: &Document, id: &str) -> Option<bool> {
        doc.vertices
            .iter()
            .find(|v| v.id == id)
            .map(|v| v.generated)
    }

    /// 생성 코드 마커는 파싱할 때 읽은 원문으로 파일마다 한 번 판정한다 — 모듈(인라인 모듈
    /// 포함)마다 같은 파일을 다시 통째로 읽으면 인라인 모듈이 수천 개인 생성 파일(prost
    /// 출력)에서 (모듈 수 × 파일 크기)를 읽었다. 인라인 모듈의 모듈·아이템·impl 메서드
    /// 정점은 담은 파일의 판정을 그대로 받는다.
    #[test]
    fn generated_markers_are_judged_once_per_parsed_file() {
        let tmp =
            std::env::temp_dir().join(format!("rustograph-markers-once-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let mut lib = String::from("// This file is @generated by prost-build.\npub mod plain;\n");
        for i in 0..20 {
            lib.push_str(&format!(
                "pub mod m{i} {{\n    pub struct S;\n    impl S {{\n        pub fn f(&self) {{}}\n    }}\n    \
                 pub mod inner {{\n        pub fn g() {{}}\n    }}\n}}\n"
            ));
        }
        let dir = write_crate(
            &tmp,
            "markers_once",
            None,
            &[
                ("src/lib.rs", lib.as_bytes()),
                ("src/plain.rs", b"pub fn p() {}\n".as_slice()),
            ],
        );
        let scans_now = || {
            (
                MODULE_MARKER_SCANS.with(std::cell::Cell::get),
                harvest::generated_marker_scans(),
            )
        };
        let (parsed_before, scans_before) = (parsed_files(), scans_now());
        let doc = load(&dir, &span_options()).unwrap();
        let parsed = parsed_files() - parsed_before;
        let scans = (
            scans_now().0 - scans_before.0,
            scans_now().1 - scans_before.1,
        );
        assert_eq!(parsed, 2, "lib.rs and plain.rs");
        assert_eq!(
            scans,
            (parsed, parsed),
            "(module rule, item rule) marker scans for {parsed} parsed files"
        );
        for id in [
            "markers_once",
            "markers_once::m7",
            "markers_once::m7::inner",
            "markers_once::m7::S",
            "markers_once::m7::S::f",
            "markers_once::m7::inner::g",
        ] {
            assert_eq!(generated_flag(&doc, id), Some(true), "{id}");
        }
        for id in ["markers_once::plain", "markers_once::plain::p"] {
            assert_eq!(generated_flag(&doc, id), Some(false), "{id}");
        }
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// 모듈 정점과 아이템 정점의 생성 마커 규칙은 서로 다르다 — 읽기만 공유하고 규칙은
    /// 합치지 않는다(합치면 기존 generated 플래그가 바뀐다). 둘 다 원문 앞 10줄만 본다:
    /// 'do not edit'만 있으면 모듈 정점만, 'autogenerated'만 있으면 아이템 정점만 참이다.
    #[test]
    fn generated_marker_rules_stay_distinct_for_modules_and_items() {
        let tmp =
            std::env::temp_dir().join(format!("rustograph-marker-rules-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let items = "pub fn f() {}\npub struct S;\nimpl S {\n    pub fn m(&self) {}\n}\n";
        // 주석 `marker`를 `line`번째 줄에 둔 파일 머리.
        let marker_at =
            |line: usize, marker: &str| format!("{}// {marker}\n", "\n".repeat(line - 1));
        // (모듈 이름, 파일 머리, 모듈 정점 generated, 아이템 정점 generated)
        let cases = [
            ("at_ten", marker_at(10, "@generated"), true, true),
            ("at_eleven", marker_at(11, "@generated"), false, false),
            ("edit_only", marker_at(1, "DO NOT EDIT"), true, false),
            ("autogen", marker_at(1, "autogenerated"), false, true),
        ];
        let lib: String = cases
            .iter()
            .map(|(name, ..)| format!("pub mod {name};\n"))
            .collect();
        let sources: Vec<(String, String)> = cases
            .iter()
            .map(|(name, head, ..)| (format!("src/{name}.rs"), format!("{head}{items}")))
            .collect();
        let mut files: Vec<(&str, &[u8])> = vec![("src/lib.rs", lib.as_bytes())];
        files.extend(
            sources
                .iter()
                .map(|(path, text)| (path.as_str(), text.as_bytes())),
        );
        let dir = write_crate(&tmp, "marker_rules", None, &files);
        let doc = load(&dir, &span_options()).unwrap();
        assert_eq!(generated_flag(&doc, "marker_rules"), Some(false));
        for (name, _, module_flag, item_flag) in &cases {
            let module = format!("marker_rules::{name}");
            assert_eq!(
                generated_flag(&doc, &module),
                Some(*module_flag),
                "{module}"
            );
            for item in ["f", "S", "S::m"] {
                let id = format!("{module}::{item}");
                assert_eq!(generated_flag(&doc, &id), Some(*item_flag), "{id}");
            }
        }
        std::fs::remove_dir_all(&tmp).ok();
    }
}
