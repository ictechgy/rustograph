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
use std::path::Path;

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
    let doc = crate::source::load(
        dir,
        &crate::source::Options {
            symbol_level: true,
            include_deps: true,
            ..Default::default()
        },
    )?;
    Ok(report_from(&meta, &doc))
}

/// 이미 로드된 메타데이터와 문서로 보고서를 만든다 — 캐시·테스트용.
pub fn report_from(meta: &Metadata, doc: &Document) -> DepsReport {
    let pkg_name =
        |id: &str| -> Option<&str> { meta.by_id.get(id).map(|i| meta.packages[*i].name.as_str()) };
    // 정점 → 소유 크레이트.
    let krate_of: BTreeMap<&str, &str> = doc
        .vertices
        .iter()
        .map(|v| (v.id.as_str(), v.krate.as_str()))
        .collect();
    // 외부 정점의 존재 — 없으면 사용 간선은 수확 자체가 안 됐다.
    let has_dep_vertices = meta
        .packages
        .iter()
        .filter(|p| !p.workspace_member)
        .any(|p| doc.vertex_ids().contains(p.name.as_str()));

    let mut unused = Vec::new();
    let mut build_skipped = 0usize;
    let mut seen: BTreeSet<(String, String, String)> = BTreeSet::new();
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
        } else {
            BTreeSet::from([tp.name.as_str()])
        };
        // 사용 증거: dep 정점(또는 dep::* 경로)으로 들어오는 확정
        // 비-depends 간선 중 이 멤버의 크레이트가 보낸 것. tentative
        // 팬아웃은 "이 중 하나일 수 있다"는 추정이라 증거가 아니다 —
        // 추정을 증거로 쓰면 미사용 의존이 우연히 숨겨진다.
        let used = doc.edges.iter().any(|e2| {
            e2.kind != EdgeKind::Depends
                && !e2.tentative
                && to_vertices
                    .iter()
                    .any(|v| e2.to == *v || e2.to.starts_with(&format!("{v}::")))
                && krate_of
                    .get(e2.from.as_str())
                    .is_some_and(|k| member_krates.contains(k))
        });
        if !used {
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
    let dev_findings = unused.iter().filter(|u| u.kind == "dev").count();
    if dev_findings > 0 {
        limitations.push(format!(
            "{dev_findings} findings are dev-dependencies — test/example targets are not harvested, so their usage is invisible"
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
