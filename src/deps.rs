//! 선언 의존성 질의 — 미사용 의존성과 중복 버전.
//!
//! 사용 증거는 그래프 간선이다: `--deps` 수확이 `depname::x` 참조를
//! 크레이트 정점으로 붕괴해 두었으니, 여기서는 그 정점으로 들어오는
//! 의존 간선이 있는지만 본다. 못 보는 것은 숨기지 않고 limitation으로
//! 센다 — proc 매크로 derive·build.rs·테스트 전용 사용은 수확 밖이다.

use crate::cargo_meta::{self, Metadata};
use crate::graph::{Document, EdgeKind};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// 참조가 한 건도 관측되지 않은 선언 의존.
#[derive(Debug, Serialize)]
pub struct UnusedDep {
    /// 선언한 워크스페이스 멤버 패키지.
    pub package: String,
    /// 선언된 의존 패키지 이름.
    pub dep: String,
    /// ""(normal) | "dev" | "build" — cargo dep_kinds 그대로.
    pub kind: String,
    /// proc-macro 크레이트는 `use` 없이 derive/속성으로만 쓰일 수 있다 —
    /// 그런 사용은 그래프에 안 잡히니 약한 증거로 표시한다.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub proc_macro: bool,
}

/// 여러 버전이 해석된 패키지 — `cargo tree -d`의 그래프 표현.
#[derive(Debug, Serialize)]
pub struct DupDep {
    pub name: String,
    pub versions: Vec<String>,
}

/// deps 보고서 — 미사용·중복·실측 한계.
#[derive(Debug, Serialize)]
pub struct DepsReport {
    pub unused: Vec<UnusedDep>,
    pub duplicates: Vec<DupDep>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub limitations: Vec<String>,
}

/// `dir`의 deps 보고서를 만든다 — 메타데이터와 문서를 직접 수확한다.
/// 사용 증거는 syn 문서가 완전하다 — semantic 문서는 외부 경로를
/// `external` 카운터로만 세서 미사용 오탐이 생기고, 호출자 문서를
/// 받으면 focus/target 필터가 증거를 지워 거짓 미사용이 된다.
/// 그래서 deps는 항상 자체 syn 수확으로 만든다.
pub fn report(dir: &Path) -> Result<DepsReport, String> {
    let meta = cargo_meta::load(dir)?;
    report_with(dir, &meta)
}

/// [`report`]와 같은 보고서를, `dir`의 cargo 워크스페이스 루트가 `workspace`와 같은 디렉터리일 때만
/// 만든다. 다르면 수확하지 않고 `Ok(Err(dir의 워크스페이스 루트))`다 — 바깥 `Err`는 메타데이터·수확
/// 오류다. 저장 스냅샷을 서빙하는 쪽(MCP `--graph`)은 deps만 `--dir`을 따로 수확한다 — 두 워크스페이스가
/// 다르면 한 세션의 답이 서로 다른 저장소를 말하는데 받는 쪽은 구분할 수 없다. `workspace`는 신뢰하지
/// 않는 저장 문서의 값이라 그 경로에서는 cargo를 실행하지 않는다(문서가 Cargo.lock을 쓸 곳과 네트워크
/// 대상을 고르게 된다). cargo는 받은 경로 표기를 그대로 돌려주므로, 같은 디렉터리의 다른 표기(심볼릭
/// 링크)는 정규화한 경로로 같게 본다.
pub(crate) fn report_in_workspace(
    dir: &Path,
    workspace: &Path,
) -> Result<Result<DepsReport, PathBuf>, String> {
    let meta = cargo_meta::load(dir)?;
    if !same_directory(&meta.workspace_root, workspace) {
        return Ok(Err(meta.workspace_root));
    }
    report_with(dir, &meta).map(Ok)
}

/// 이미 읽은 메타데이터로 `dir`을 수확해 보고서를 만든다 — 같은 메타데이터로 수확한다(source::load는
/// cargo metadata를 한 번 더 돌린다).
fn report_with(dir: &Path, meta: &Metadata) -> Result<DepsReport, String> {
    let doc = crate::source::load_with(
        dir,
        meta,
        &crate::source::Options {
            symbol_level: true,
            include_deps: true,
            ..Default::default()
        },
    )?;
    Ok(report_from(meta, &doc))
}

/// 두 경로가 같은 디렉터리인가 — 표기가 같거나, 둘 다 정규화한 경로가 같다.
fn same_directory(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        // 정규화하지 못한 경로(없는 경로·권한 없음)는 같은 디렉터리임을 확인하지 못한 것이다 — fail-closed로
        // 다르다고 답하고, 호출자가 그 사실을 원인과 해결 방향이 담긴 오류로 알린다.
        _ => false,
    }
}

#[cfg(test)]
thread_local! {
    /// 보고서가 정점·간선을 살핀 수 — 선언 의존·외부 패키지마다 문서 전체를 다시 훑지
    /// 않는지 단위 테스트가 센다(테스트 스레드마다 따로).
    static VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// 테스트용 — 이 스레드에서 지금까지 살핀 정점·간선 수.
#[cfg(test)]
fn visits() -> usize {
    VISITS.with(std::cell::Cell::get)
}

/// 테스트용 — 정점·간선 하나를 살폈다고 센다.
#[cfg(test)]
fn note_visit() {
    VISITS.with(|c| c.set(c.get() + 1));
}

/// 이미 로드된 메타데이터와 문서로 보고서를 만든다 — 캐시·테스트용.
pub fn report_from(meta: &Metadata, doc: &Document) -> DepsReport {
    let pkg_name =
        |id: &str| -> Option<&str> { meta.by_id.get(id).map(|i| meta.packages[*i].name.as_str()) };
    // 정점 → 소유 크레이트.
    let krate_of: BTreeMap<&str, &str> = doc
        .vertices
        .iter()
        .map(|v| {
            #[cfg(test)]
            note_visit();
            (v.id.as_str(), v.krate.as_str())
        })
        .collect();
    // 외부 정점의 존재 — 없으면 사용 간선은 수확 자체가 안 됐다. krate_of의 키가 곧
    // 정점 ID 전부다 — 외부 패키지마다 정점 ID 집합을 새로 만들면 O(P·V)다.
    let has_dep_vertices = meta
        .packages
        .iter()
        .filter(|p| !p.workspace_member)
        .any(|p| krate_of.contains_key(p.name.as_str()));
    let evidence = usage_evidence(doc, &krate_of);

    let mut unused = Vec::new();
    let mut build_skipped = 0usize;
    let mut dev_skipped = 0usize;
    let mut ambiguous_external = 0usize;
    let mut seen: BTreeSet<(String, String, String)> = BTreeSet::new();
    // 멤버의 그래프 루트 ID들 — lib/bin 타깃 이름(없으면 패키지 이름).
    // 외부 패키지명과 충돌하면 사용 증거가 어느 쪽 것인지 문자열로
    // 구별할 수 없다 — 그 dep은 모호로 센다.
    let member_roots: BTreeSet<String> = meta
        .packages
        .iter()
        .filter(|p| p.workspace_member)
        .flat_map(|p| {
            let names: Vec<String> = p
                .targets
                .iter()
                .filter(|t| matches!(t.kind.as_str(), "lib" | "bin"))
                .map(|t| t.name.clone())
                .collect();
            if names.is_empty() {
                vec![p.name.clone()]
            } else {
                names
            }
        })
        .collect();
    for e in &meta.dep_edges {
        let (Some(from), Some(to)) = (pkg_name(&e.from), pkg_name(&e.to)) else {
            continue;
        };
        let fp = &meta.packages[meta.by_id[&e.from]];
        if !fp.workspace_member {
            continue; // 미사용 판정은 멤버의 선언만 대상이다.
        }
        if !seen.insert((from.to_string(), to.to_string(), e.kind.clone())) {
            continue;
        }
        if e.kind == "build" {
            build_skipped += 1;
            continue; // build.rs는 수확 범위 밖 — 판정 불가로 넘긴다.
        }
        if e.kind == "dev" {
            dev_skipped += 1;
            continue; // tests/examples/benches는 수확 범위 밖 — 사용
                      // 증거가 원천적으로 불완전해 미사용 판정이 오탐이
                      // 된다. 판정 불가로 넘기고 limitation에 센다.
        }
        // 멤버의 그래프상 크레이트 이름 — 패키지 이름과 lib/bin 타깃 이름.
        let member_krates: BTreeSet<&str> = std::iter::once(fp.name.as_str())
            .chain(
                fp.targets
                    .iter()
                    .filter(|t| matches!(t.kind.as_str(), "lib" | "bin"))
                    .map(|t| t.name.as_str()),
            )
            .collect();
        // 사용 쪽도 정점 ID로 맞춘다 — 멤버 의존은 lib 타깃 이름으로
        // 해석되지 패키지 이름으로 해석되지 않는다(`[lib] name`이 다른
        // 패키지는 패키지명 정점이 없다). 외부는 패키지명이 정점이다 —
        // 외부 패키지의 타깃 이름까지 넣으면 동명 멤버 정점과 오매칭된다.
        let tp = &meta.packages[meta.by_id[&e.to]];
        let to_vertices: BTreeSet<&str> = if tp.workspace_member {
            std::iter::once(tp.name.as_str())
                .chain(
                    tp.targets
                        .iter()
                        .filter(|t| matches!(t.kind.as_str(), "lib" | "bin"))
                        .map(|t| t.name.as_str()),
                )
                .collect()
        } else if member_roots.contains(tp.name.as_str()) {
            // 외부 패키지명이 멤버 루트 ID와 같다 — `x::`로의 간선이
            // 멤버 것인지 이 dep 것인지 구별할 방법이 없다. 멤버
            // 증거를 외부 dep 사용으로 세지 않고 모호로 남긴다.
            ambiguous_external += 1;
            BTreeSet::new()
        } else {
            BTreeSet::from([tp.name.as_str()])
        };
        // 사용 증거: dep 정점(또는 dep::* 경로)으로 들어오는 확정
        // 비-depends 간선 중 이 멤버의 크레이트가 보낸 것. tentative
        // 팬아웃은 "이 중 하나일 수 있다"는 추정이라 증거가 아니다 —
        // 추정을 증거로 쓰면 미사용 의존이 우연히 숨겨진다.
        if !is_used(&evidence, &member_krates, &to_vertices) {
            let proc_macro = meta.packages[meta.by_id[&e.to]].proc_macro;
            unused.push(UnusedDep {
                package: from.to_string(),
                dep: to.to_string(),
                kind: e.kind.clone(),
                proc_macro,
            });
        }
    }
    unused.sort_by(|a, b| (&a.package, &a.dep, &a.kind).cmp(&(&b.package, &b.dep, &b.kind)));

    // 중복 버전 — 패키지 이름으로 묶어 버전이 2개 이상이면 보고.
    let mut by_name: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    for p in &meta.packages {
        by_name
            .entry(p.name.as_str())
            .or_default()
            .insert(p.version.as_str());
    }
    let duplicates: Vec<DupDep> = by_name
        .into_iter()
        .filter(|(_, vs)| vs.len() > 1)
        .map(|(name, vs)| DupDep {
            name: name.to_string(),
            versions: vs.into_iter().map(|s| s.to_string()).collect(),
        })
        .collect();

    let mut limitations = meta.limitations.clone();
    if !has_dep_vertices && meta.packages.iter().any(|p| !p.workspace_member) {
        limitations.push(
            "document has no external crate vertices (not built with --deps); \
             unused-dep evidence is incomplete"
                .to_string(),
        );
    }
    if build_skipped > 0 {
        limitations.push(format!(
            "{build_skipped} build-dependencies not checked (build scripts are outside the harvested graph)"
        ));
    }
    if dev_skipped > 0 {
        limitations.push(format!(
            "{dev_skipped} dev-dependencies not checked (test/example/bench targets are not harvested, so their usage is invisible)"
        ));
    }
    if ambiguous_external > 0 {
        limitations.push(format!(
            "{ambiguous_external} external dependencies share a name with a member crate root — usage evidence is ambiguous"
        ));
    }
    if !unused.is_empty() {
        limitations.push(
            "dep usage is measured from references observed in the harvested graph; \
             proc-macro derives, macro-generated code, and doc-tests may use a dep invisibly"
                .to_string(),
        );
    }
    DepsReport {
        unused,
        duplicates,
        limitations,
    }
}

/// 사용 증거 색인 — 확정 비-depends 간선마다 (보낸 정점의 크레이트, 도착 ID의 첫 `::`
/// 세그먼트) 한 쌍. dep 정점 이름(패키지·lib/bin 타깃 이름)에는 `::`가 없으니 "도착이 dep
/// 정점이거나 `dep::`로 시작한다"는 "도착의 첫 세그먼트가 dep 이름이다"와 같다. 선언
/// 의존마다 전 간선을 다시 훑으면(간선마다 `format!` 할당까지) O(D·E)라, 간선을 한 번만
/// 돌아 색인한다. 보낸 쪽이 정점이 아닌 간선은 소유 크레이트를 몰라 증거가 아니다.
fn usage_evidence<'a>(
    doc: &'a Document,
    krate_of: &BTreeMap<&str, &'a str>,
) -> BTreeSet<(&'a str, &'a str)> {
    let mut evidence = BTreeSet::new();
    for e in &doc.edges {
        #[cfg(test)]
        note_visit();
        if e.kind == EdgeKind::Depends || e.tentative {
            continue;
        }
        if let Some(krate) = krate_of.get(e.from.as_str()) {
            let head =
                e.to.split_once("::")
                    .map_or(e.to.as_str(), |(head, _)| head);
            evidence.insert((*krate, head));
        }
    }
    evidence
}

/// 선언 의존 하나가 쓰였는가 — 멤버의 그래프상 크레이트 하나가 dep 정점 하나로 보낸
/// 사용 증거([`usage_evidence`])가 있으면 참이다.
fn is_used<'a>(
    evidence: &BTreeSet<(&'a str, &'a str)>,
    member_krates: &BTreeSet<&'a str>,
    to_vertices: &BTreeSet<&'a str>,
) -> bool {
    to_vertices
        .iter()
        .any(|v| member_krates.iter().any(|k| evidence.contains(&(*k, *v))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cargo_meta::{DepEdge, Package};
    use crate::graph::{document, Edge, Kind, Level, Vertex};
    use std::path::PathBuf;

    fn v(id: &str, kind: Kind) -> Vertex {
        Vertex {
            id: id.to_string(),
            kind,
            krate: id.split("::").next().unwrap_or(id).to_string(),
            module: id.to_string(),
            position: Some("src/lib.rs:1".into()),
            exported: false,
            generated: false,
            cfg: None,
            unsafe_: false,
        }
    }

    fn meta() -> Metadata {
        // app → serde(정상), app → unused_dep(미사용), app → tool(build, 건너뜀).
        let pkgs = [
            ("app", "0.1.0", true, vec!["bin"]),
            ("serde", "1.0.0", false, vec!["lib"]),
            ("unused_dep", "2.0.0", false, vec!["lib"]),
            ("tool", "1.0.0", false, vec!["lib"]),
            ("dup", "1.0.0", false, vec!["lib"]),
            ("dup", "2.0.0", false, vec!["lib"]),
        ];
        let mut packages = Vec::new();
        let mut by_id = BTreeMap::new();
        for (i, (name, ver, member, kinds)) in pkgs.iter().enumerate() {
            by_id.insert(format!("pkg{i}"), i);
            packages.push(Package {
                name: name.to_string(),
                version: ver.to_string(),
                workspace_member: *member,
                proc_macro: *name == "tool",
                targets: kinds
                    .iter()
                    .map(|k| crate::cargo_meta::Target {
                        name: name.to_string(),
                        kind: k.to_string(),
                        src: PathBuf::from("x.rs"),
                    })
                    .collect(),
            });
        }
        let dep_edges = vec![
            DepEdge {
                from: "pkg0".into(),
                to: "pkg1".into(),
                kind: String::new(),
                lib_name: "serde".into(),
            },
            DepEdge {
                from: "pkg0".into(),
                to: "pkg2".into(),
                kind: String::new(),
                lib_name: "unused_dep".into(),
            },
            DepEdge {
                from: "pkg0".into(),
                to: "pkg3".into(),
                kind: "build".into(),
                lib_name: "tool".into(),
            },
        ];
        Metadata {
            packages,
            by_id,
            dep_edges,
            workspace_root: PathBuf::from("."),
            limitations: vec![],
        }
    }

    #[test]
    fn unused_and_duplicates_and_build_skipped() {
        let meta = meta();
        let doc = document(
            Level::Symbol,
            ".".into(),
            None,
            vec![],
            vec![
                v("app", Kind::Crate),
                v("app::main", Kind::Fn),
                // 외부 크레이트 정점 — position 없음.
                Vertex {
                    position: None,
                    ..v("serde", Kind::Crate)
                },
                Vertex {
                    position: None,
                    ..v("unused_dep", Kind::Crate)
                },
                Vertex {
                    position: None,
                    ..v("dup", Kind::Crate)
                },
            ],
            vec![
                Edge::new("app".into(), "serde".into(), EdgeKind::Depends),
                Edge::new("app".into(), "unused_dep".into(), EdgeKind::Depends),
                Edge::new("app".into(), "dup".into(), EdgeKind::Depends),
                // serde는 실제로 참조된다 — 미사용이 아니다.
                Edge::new("app::main".into(), "serde".into(), EdgeKind::References),
            ],
            vec![],
        );
        let rep = report_from(&meta, &doc);
        assert_eq!(rep.unused.len(), 1);
        assert_eq!(rep.unused[0].dep, "unused_dep");
        // build 의존은 판정 제외 — limitation에 계수된다.
        assert!(rep
            .limitations
            .iter()
            .any(|l| l.contains("build-dependencies")));
        // dup은 버전이 2개라 보고된다.
        assert_eq!(rep.duplicates.len(), 1);
        assert_eq!(rep.duplicates[0].versions, vec!["1.0.0", "2.0.0"]);
    }

    #[test]
    fn tentative_edges_are_not_usage_evidence() {
        // 메서드 팬아웃의 tentative 간선은 "이 중 하나일 수 있다"는
        // 추정이다 — 사용 증거로 세면 미사용 의존이 숨겨진다.
        let meta = meta();
        let mut e = Edge::new("app::main".into(), "unused_dep".into(), EdgeKind::Call);
        e.tentative = true;
        let doc = document(
            Level::Symbol,
            ".".into(),
            None,
            vec![],
            vec![
                v("app", Kind::Crate),
                v("app::main", Kind::Fn),
                Vertex {
                    position: None,
                    ..v("serde", Kind::Crate)
                },
                Vertex {
                    position: None,
                    ..v("unused_dep", Kind::Crate)
                },
                Vertex {
                    position: None,
                    ..v("dup", Kind::Crate)
                },
            ],
            vec![
                Edge::new("app".into(), "serde".into(), EdgeKind::Depends),
                Edge::new("app".into(), "unused_dep".into(), EdgeKind::Depends),
                Edge::new("app".into(), "dup".into(), EdgeKind::Depends),
                Edge::new("app::main".into(), "serde".into(), EdgeKind::References),
                e,
            ],
            vec![],
        );
        let rep = report_from(&meta, &doc);
        assert_eq!(rep.unused.len(), 1);
        assert_eq!(rep.unused[0].dep, "unused_dep");
    }

    #[test]
    fn renamed_member_lib_matches_target_vertex() {
        // 패키지명 real_pkg, [lib] name = "real_lib" — 그래프 정점은
        // real_lib다. 패키지명으로만 찾으면 사용 중인 dep이 미사용으로
        // 보고된다.
        let mut packages = Vec::new();
        let mut by_id = BTreeMap::new();
        by_id.insert("p_app".to_string(), 0);
        packages.push(Package {
            name: "app".into(),
            version: "0.1.0".into(),
            workspace_member: true,
            proc_macro: false,
            targets: vec![crate::cargo_meta::Target {
                name: "app".into(),
                kind: "bin".into(),
                src: PathBuf::from("x.rs"),
            }],
        });
        by_id.insert("p_real".to_string(), 1);
        packages.push(Package {
            name: "real_pkg".into(),
            version: "0.1.0".into(),
            workspace_member: true,
            proc_macro: false,
            targets: vec![crate::cargo_meta::Target {
                name: "real_lib".into(), // [lib] name ≠ package name
                kind: "lib".into(),
                src: PathBuf::from("x.rs"),
            }],
        });
        let meta = Metadata {
            packages,
            by_id,
            dep_edges: vec![DepEdge {
                from: "p_app".into(),
                to: "p_real".into(),
                kind: String::new(),
                lib_name: "real_lib".into(),
            }],
            workspace_root: PathBuf::from("."),
            limitations: vec![],
        };
        let doc = document(
            Level::Symbol,
            ".".into(),
            None,
            vec![],
            vec![
                v("app", Kind::Crate),
                v("app::main", Kind::Fn),
                v("real_lib", Kind::Crate),
                v("real_lib::helper", Kind::Fn),
            ],
            vec![
                Edge::new("app".into(), "real_lib".into(), EdgeKind::Depends),
                Edge::new(
                    "app::main".into(),
                    "real_lib::helper".into(),
                    EdgeKind::Call,
                ),
            ],
            vec![],
        );
        let rep = report_from(&meta, &doc);
        assert!(
            rep.unused.is_empty(),
            "renamed member lib must match by target name: {:?}",
            rep.unused
        );
    }

    /// 라이브러리 타깃 하나인 패키지 — 이름이 곧 타깃 이름이다.
    fn lib_package(name: &str, workspace_member: bool) -> Package {
        Package {
            name: name.into(),
            version: "1.0.0".into(),
            workspace_member,
            proc_macro: false,
            targets: vec![crate::cargo_meta::Target {
                name: name.into(),
                kind: "lib".into(),
                src: PathBuf::from("x.rs"),
            }],
        }
    }

    /// 멤버 `app` 하나가 외부 `ext0..ext{n}`을 선언하고 짝수 번째만 쓰는 메타데이터·문서.
    /// 외부 정점이 없는 전이 의존 `t0..t9`가 패키지 목록 앞쪽에 있다 — cargo metadata는
    /// 패키지를 이름순으로 주므로 외부 정점 탐색이 그 앞의 패키지들을 먼저 지난다.
    fn wide_workspace(n: usize) -> (Metadata, Document) {
        let mut packages = vec![lib_package("app", true)];
        packages.extend((0..10).map(|i| lib_package(&format!("t{i}"), false)));
        packages.extend((0..n).map(|i| lib_package(&format!("ext{i}"), false)));
        let by_id = (0..packages.len()).map(|i| (format!("p{i}"), i)).collect();
        let dep_edges = (0..n)
            .map(|i| DepEdge {
                from: "p0".into(),
                to: format!("p{}", 11 + i),
                kind: String::new(),
                lib_name: format!("ext{i}"),
            })
            .collect();
        let meta = Metadata {
            packages,
            by_id,
            dep_edges,
            workspace_root: PathBuf::from("."),
            limitations: vec![],
        };
        let mut vertices = vec![v("app", Kind::Crate)];
        let mut edges = Vec::new();
        for i in 0..n {
            vertices.push(v(&format!("app::f{i}"), Kind::Fn));
            vertices.push(Vertex {
                position: None,
                ..v(&format!("ext{i}"), Kind::Crate)
            });
            edges.push(Edge::new(
                "app".into(),
                format!("ext{i}"),
                EdgeKind::Depends,
            ));
            if i > 0 {
                edges.push(Edge::new(
                    format!("app::f{}", i - 1),
                    format!("app::f{i}"),
                    EdgeKind::Call,
                ));
            }
            if i % 2 == 0 {
                edges.push(Edge::new(
                    format!("app::f{i}"),
                    format!("ext{i}"),
                    EdgeKind::References,
                ));
            }
        }
        let doc = document(
            Level::Symbol,
            ".".into(),
            None,
            vec![],
            vertices,
            edges,
            vec![],
        );
        (meta, doc)
    }

    /// 선언 의존마다 전 간선을 다시 훑고(간선마다 `format!` 할당까지) 외부 패키지마다 정점
    /// ID 집합을 새로 만들면 O(D·E + P·V)다 — 선언 1080개·함수 4만 합성 워크스페이스의
    /// `deps`가 7.7초였다. 보고서가 정점 색인(정점마다 한 번)과 사용 증거 색인(간선마다 한 번)을
    /// 정확히 한 번씩 만드는지 센다 — 상한이 아니라 정확한 횟수라서, 선언 의존마다 색인을 다시
    /// 만들어도(넘침) 증거 색인을 빼고 옛 간선 전체 탐색으로 돌아가도(모자람) 실패한다.
    #[test]
    fn report_visits_each_vertex_and_edge_once() {
        let n = 40;
        let (meta, doc) = wide_workspace(n);
        let before = visits();
        let rep = report_from(&meta, &doc);
        let visited = visits() - before;
        let unused: Vec<&str> = rep.unused.iter().map(|u| u.dep.as_str()).collect();
        let odd: Vec<String> = (1..n).step_by(2).map(|i| format!("ext{i}")).collect();
        let mut odd: Vec<&str> = odd.iter().map(String::as_str).collect();
        odd.sort();
        assert_eq!(unused, odd);
        let each_once = doc.vertices.len() + doc.edges.len();
        assert_eq!(
            visited, each_once,
            "{visited} vertex/edge visits for {each_once} vertices and edges"
        );
    }

    /// deps 보고서는 cargo metadata를 한 번만 돌린다 — 수확이 메타데이터를 다시 읽으면
    /// tests/fixture 같은 작은 워크스페이스에서 실행 시간의 4할이 두 번째 cargo 실행이었다.
    #[test]
    fn report_runs_cargo_metadata_once() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture");
        let before = crate::cargo_meta::loads();
        let rep = report(&dir).unwrap();
        assert_eq!(crate::cargo_meta::loads() - before, 1);
        // 수확이 같은 메타데이터를 받아도 판정은 그대로다 — fixture_unused는 선언만 됐다.
        assert!(
            rep.unused
                .iter()
                .any(|u| u.package == "fixture_app" && u.dep == "fixture_unused"),
            "{:?}",
            rep.unused
        );
    }

    /// 결정적 의사 난수(LCG) — 무작위 비교 테스트가 실행마다 같은 문서를 본다.
    struct Lcg(u64);

    impl Lcg {
        fn below(&mut self, n: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) as usize % n
        }
    }

    /// 색인 전 구현 그대로의 사용 판정 — 선언 의존마다 간선 전체를 훑어 dep 정점(또는
    /// `dep::…`)에 닿는 이 멤버의 확정 비-depends 간선을 찾는다. 색인 판정의 오라클이다.
    fn used_by_scan(
        doc: &Document,
        krate_of: &BTreeMap<&str, &str>,
        member_krates: &BTreeSet<&str>,
        to_vertices: &BTreeSet<&str>,
    ) -> bool {
        doc.edges.iter().any(|e2| {
            e2.kind != EdgeKind::Depends
                && !e2.tentative
                && to_vertices
                    .iter()
                    .any(|v| e2.to == *v || e2.to.starts_with(&format!("{v}::")))
                && krate_of
                    .get(e2.from.as_str())
                    .is_some_and(|k| member_krates.contains(k))
        })
    }

    /// 접두사가 겹치는 크레이트 이름(`a`·`ab`·`a_b`), 경로 중간의 같은 이름(`b::a`), 트레이트
    /// 경로(`c::<a as b>::m`), 정점이 아닌 출발점, 같은 ID의 크레이트가 다른 변형, 모든 간선
    /// 종류·추정 간선을 섞은 무작위 문서에서 색인 판정이 간선 전체 탐색과 같다.
    #[test]
    fn usage_index_matches_edge_scan_on_random_documents() {
        let names = ["a", "ab", "a_b", "b", "c"];
        let ids = [
            "a",
            "a::x",
            "a::x::y",
            "ab",
            "ab::y",
            "a_b::z",
            "b",
            "b::a",
            "b::a::w",
            "c::<a as b>::m",
            "zz::q",
        ];
        let kinds = [
            EdgeKind::Depends,
            EdgeKind::Uses,
            EdgeKind::Contains,
            EdgeKind::Call,
            EdgeKind::References,
            EdgeKind::Implements,
            EdgeKind::Signature,
        ];
        let mut rng = Lcg(0xdeb5);
        // 크레이트 이름 대개 1~2개, 가끔 빈 집합 — 멤버의 크레이트 집합과 dep 정점 집합에 쓴다.
        let pick = |rng: &mut Lcg| -> BTreeSet<&str> {
            let count = if rng.below(8) == 0 {
                0
            } else {
                1 + rng.below(2)
            };
            (0..count).map(|_| names[rng.below(names.len())]).collect()
        };
        let (mut used, mut unused) = (0, 0);
        for _ in 0..3_000 {
            let vertices: Vec<Vertex> = (0..1 + rng.below(8))
                .map(|_| Vertex {
                    krate: names[rng.below(names.len())].into(),
                    ..v(ids[rng.below(ids.len())], Kind::Fn)
                })
                .collect();
            let edges: Vec<Edge> = (0..rng.below(10))
                .map(|_| {
                    // 출발점은 대개 정점이고 가끔 정점이 아닌 ID다.
                    let from = if rng.below(4) == 0 {
                        ids[rng.below(ids.len())].to_string()
                    } else {
                        vertices[rng.below(vertices.len())].id.clone()
                    };
                    let mut e = Edge::new(
                        from,
                        ids[rng.below(ids.len())].into(),
                        kinds[rng.below(kinds.len())],
                    );
                    e.tentative = rng.below(4) == 0;
                    e
                })
                .collect();
            let doc = document(
                Level::Symbol,
                ".".into(),
                None,
                vec![],
                vertices,
                edges,
                vec![],
            );
            // report_from과 같은 수집 — 같은 ID가 여럿이면 나중 정점의 크레이트가 남는다.
            let krate_of: BTreeMap<&str, &str> = doc
                .vertices
                .iter()
                .map(|v| (v.id.as_str(), v.krate.as_str()))
                .collect();
            let evidence = usage_evidence(&doc, &krate_of);
            for _ in 0..4 {
                let (member_krates, to_vertices) = (pick(&mut rng), pick(&mut rng));
                let expected = used_by_scan(&doc, &krate_of, &member_krates, &to_vertices);
                assert_eq!(
                    is_used(&evidence, &member_krates, &to_vertices),
                    expected,
                    "members {member_krates:?}, deps {to_vertices:?}, edges {:?}",
                    doc.edges
                );
                used += usize::from(expected);
                unused += usize::from(!expected);
            }
        }
        // 비교가 공허하지 않은지 — 양쪽 판정을 충분히 봤다.
        assert!(used > 500 && unused > 500, "used {used}, unused {unused}");
    }

    #[test]
    fn missing_dep_vertices_is_limited_not_wrong() {
        let meta = meta();
        // --deps 없이 수확된 문서 — 외부 정점이 없다.
        let doc = document(
            Level::Symbol,
            ".".into(),
            None,
            vec![],
            vec![v("app", Kind::Crate), v("app::main", Kind::Fn)],
            vec![],
            vec![],
        );
        let rep = report_from(&meta, &doc);
        assert!(rep
            .limitations
            .iter()
            .any(|l| l.contains("no external crate vertices")));
    }
}
