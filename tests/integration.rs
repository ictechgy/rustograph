//! tests/fixture 워크스페이스에 대한 통합 수확 테스트.
//! fixture는 두 멤버(fixture_core lib + fixture_app bin)로 크로스 크레이트
//! 해석·orphan·cfg·generated·테스트 게이트를 검증한다.

use rustograph::graph::{EdgeKind, Kind, Level};
use rustograph::{analysis, source};
use std::path::PathBuf;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixture")
}

fn doc(opts: &source::Options) -> rustograph::graph::Document {
    source::load(&fixture(), opts).expect("fixture harvest failed")
}

#[test]
fn crate_level_has_members_and_dep_edge() {
    let d = doc(&Default::default()).view(Level::Crate);
    let ids = d.vertex_ids();
    assert!(ids.contains("fixture_core") && ids.contains("fixture_app"));
    assert!(d
        .edges
        .iter()
        .any(|e| e.from == "fixture_app" && e.to == "fixture_core" && e.kind == EdgeKind::Depends));
}

#[test]
fn cross_crate_calls_resolve() {
    let d = doc(&source::Options {
        symbol_level: true,
        ..Default::default()
    });
    // main -> fixture_core::entry 는 매크로 토큰 안에서도 잡혀야 한다.
    assert!(d.edges.iter().any(|e| e.from == "fixture_app::main"
        && e.to == "fixture_core::entry"
        && e.kind == EdgeKind::Call));
    // 인라인 모듈의 함수도 해석된다.
    assert!(d.edges.iter().any(|e| e.from == "fixture_app::main"
        && e.to == "fixture_core::inline::inline_fn"
        && e.kind == EdgeKind::Call));
}

#[test]
fn trait_impl_and_struct_literal_edges() {
    let d = doc(&source::Options {
        symbol_level: true,
        ..Default::default()
    });
    // impl Greet for Used → implements.
    assert!(d.edges.iter().any(|e| e.from == "fixture_core::Used"
        && e.to == "fixture_core::Greet"
        && e.kind == EdgeKind::Implements));
    // Used { v: 1 } 리터럴 → references.
    assert!(d.edges.iter().any(|e| e.from == "fixture_app::main"
        && e.to == "fixture_core::Used"
        && e.kind == EdgeKind::References));
    // u.greet() 팬아웃 — impl 메서드로 추정 간선.
    assert!(d.edges.iter().any(|e| e.from == "fixture_app::main"
        && e.to == "fixture_core::Used::<Greet>::greet"
        && e.tentative));
}

#[test]
fn orphan_and_cfg_are_measured() {
    let d = doc(&Default::default());
    assert!(d.limitations.iter().any(|l| l.contains("orphans")));
    assert!(d.limitations.iter().any(|l| l.contains("#[cfg]")));
}

#[test]
fn generated_files_are_marked() {
    let d = doc(&source::Options {
        symbol_level: true,
        ..Default::default()
    });
    let gen = d
        .vertices
        .iter()
        .find(|v| v.id == "fixture_core::gen::generated_fn")
        .expect("generated_fn vertex");
    assert!(gen.generated);
}

#[test]
fn tests_flag_gates_test_roots() {
    let without = doc(&Default::default());
    assert!(!without.roots.contains(&"fixture_core::t::t1".to_string()));
    let with = doc(&source::Options {
        tests: true,
        ..Default::default()
    });
    assert!(with.roots.contains(&"fixture_core::t::t1".to_string()));
}

#[test]
fn dead_reports_private_unused() {
    let d = doc(&source::Options {
        symbol_level: true,
        ..Default::default()
    });
    let rep = analysis::dead(&d);
    let ids: Vec<&str> = rep.findings.iter().map(|f| f.id.as_str()).collect();
    assert!(ids.contains(&"fixture_core::dead_private"));
    assert!(ids.contains(&"fixture_app::extra::dead"));
    assert!(ids.contains(&"fixture_core::util::unused_in_util"));
    // 공개 API도 호출자가 없으면 unreachable로 보고된다(retain-public 없이는).
    assert!(ids.contains(&"fixture_core::gen::generated_fn"));
}

#[test]
fn retain_public_keeps_exported() {
    let d = doc(&source::Options {
        symbol_level: true,
        retain_public: true,
        ..Default::default()
    });
    let rep = analysis::dead(&d);
    let ids: Vec<&str> = rep.findings.iter().map(|f| f.id.as_str()).collect();
    assert!(!ids.contains(&"fixture_core::gen::generated_fn"));
    assert!(!ids.contains(&"fixture_core::Unused")); // pub struct는 보존.
                                                     // private 미사용은 여전히 잡힌다.
    assert!(ids.contains(&"fixture_core::dead_private"));
}

#[test]
fn module_level_graph() {
    // CLI와 같은 의미론: 심볼 수확 후 모듈 레벨로 투영.
    let d = doc(&source::Options {
        symbol_level: true,
        ..Default::default()
    })
    .view(Level::Module);
    assert!(d.vertex_ids().contains("fixture_core::util"));
    assert!(d.vertex_ids().contains("fixture_app::extra"));
    // uses 간선: app -> core의 심볼 임포트는 모듈로 귀속.
    assert!(d
        .edges
        .iter()
        .any(|e| e.from == "fixture_app" && e.to == "fixture_core" && e.kind == EdgeKind::Uses));
}

#[test]
fn kinds_cover_traits_and_methods() {
    let d = doc(&source::Options {
        symbol_level: true,
        ..Default::default()
    });
    let trait_v = d
        .vertices
        .iter()
        .find(|v| v.id == "fixture_core::Greet")
        .unwrap();
    assert_eq!(trait_v.kind, Kind::Trait);
    let m = d
        .vertices
        .iter()
        .find(|v| v.id == "fixture_core::Used::<Greet>::greet")
        .unwrap();
    assert_eq!(m.kind, Kind::Method);
}

#[test]
fn deterministic_across_runs() {
    let a = rustograph::export::to_json(&doc(&Default::default()));
    let b = rustograph::export::to_json(&doc(&Default::default()));
    assert_eq!(a, b);
}

mod cli_tests {
    //! cli::run을 fixture로 직접 구동 — 명령 본문과 종료 코드 계약.
    use super::fixture;
    use rustograph::cli;
    use std::io::Write;

    fn run(args: &[&str]) -> (i32, String, String) {
        let mut argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        argv.push("--dir".into());
        argv.push(fixture().display().to_string());
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = cli::run(&argv, &mut out, &mut err);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn graph_all_levels_and_formats() {
        for level in ["crate", "module", "type", "symbol"] {
            let (code, out, _) = run(&["graph", "--level", level]);
            assert_eq!(code, 0, "level {level}");
            assert!(out.contains("\"level\""));
        }
        let (code, out, _) = run(&["graph", "--format", "mermaid"]);
        assert_eq!(code, 0);
        assert!(out.contains("graph TD"));
        // --out으로 저장하고 --graph로 다시 읽는 왕복.
        let tmp = std::env::temp_dir().join(format!("rg-cli-{}", std::process::id()));
        let path = tmp.with_extension("json");
        let (code, out, _) = run(&[
            "graph",
            "--level",
            "symbol",
            "--out",
            path.to_str().unwrap(),
        ]);
        assert_eq!(code, 0);
        assert!(out.contains("wrote"));
        let (code, out, _) = run(&["cycles", "--graph", path.to_str().unwrap()]);
        assert_eq!(code, 0);
        assert!(out.contains("cycle(s)"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn cycles_text_json_and_strict() {
        let (code, out, _) = run(&["cycles", "--strict"]);
        assert_eq!(code, 0);
        assert!(out.contains("0 cycle(s)"));
        let (code, out, _) = run(&["cycles", "--format", "json"]);
        assert_eq!(code, 0);
        assert!(out.contains("\"cycles\""));
    }

    #[test]
    fn dead_formats_explain_and_strict() {
        let (code, out, _) = run(&["dead"]);
        assert_eq!(code, 0);
        assert!(out.contains("unreachable"));
        let (code, out, _) = run(&["dead", "--format", "json"]);
        assert_eq!(code, 0);
        assert!(out.contains("\"findings\""));
        let (code, out, _) = run(&["dead", "--explain", "fixture_core::Used"]);
        assert_eq!(code, 0);
        assert!(out.contains("fixture_app::main") || out.contains("fixture_core::Used"));
        // strict은 unreachable이 있으면 1.
        let (code, _, _) = run(&["dead", "--strict"]);
        assert_eq!(code, 1);
        let (code, _, _) = run(&["dead", "--strict", "--retain-public", "--tests"]);
        assert_eq!(code, 1); // private 미도달이 남는다.
    }

    #[test]
    fn rules_formats_and_strict() {
        let cfg = std::env::temp_dir().join(format!("rg-rules-{}", std::process::id()));
        let mut f = std::fs::File::create(&cfg).unwrap();
        writeln!(f, "components:\n  app: [\"fixture_app/**\"]\n  core: [\"fixture_core/**\"]\ndeps:\n  app: [core]\n  core: []").unwrap();
        drop(f);
        let cfgp = cfg.to_str().unwrap().to_string();
        for fmt in ["text", "json", "sarif"] {
            let (code, _, err) = run(&["rules", "--format", fmt, "--config", &cfgp]);
            assert_eq!(code, 0, "fmt {fmt}: {err}");
        }
        let (code, out, _) = run(&["rules", "--strict", "--config", &cfgp]);
        assert_eq!(code, 0);
        assert!(out.contains("0 violations"));
        let _ = std::fs::remove_file(&cfg);
        // 설정 없으면 오류.
        let (code, _, _) = run(&["rules"]);
        assert_eq!(code, 2);
    }

    #[test]
    fn query_and_impact() {
        let (code, out, _) = run(&["query", "fixture_core::entry"]);
        assert_eq!(code, 0);
        assert!(out.contains("\"found\": true"));
        let (code, out, _) = run(&[
            "query",
            "fixture_core::entry",
            "--depth",
            "2",
            "--max",
            "50",
        ]);
        assert_eq!(code, 0);
        assert!(out.contains("\"depth\": 2"));
        let (code, out, _) = run(&["impact", "fixture_core::Used"]);
        assert_eq!(code, 0);
        assert!(out.contains("\"impacted\""));
        // 없는 정점은 오류.
        let (code, _, _) = run(&["query", "fixture_core::nope"]);
        assert_eq!(code, 2);
        // 위치 인자 없으면 오류.
        let (code, _, _) = run(&["query"]);
        assert_eq!(code, 2);
    }
}

#[test]
fn glob_import_expands_to_items() {
    // use fixture_core::inline::* — 글롭은 대상 모듈의 아이템을 스코프에 펼친다.
    let d = doc(&source::Options {
        symbol_level: true,
        ..Default::default()
    });
    assert!(d.edges.iter().any(|e| e.from == "fixture_app::main"
        && e.to == "fixture_core::inline::inline_fn"
        && e.kind == EdgeKind::Call));
}

#[test]
fn harvest_covers_all_item_kinds() {
    let d = doc(&source::Options {
        symbol_level: true,
        ..Default::default()
    });
    let has = |id: &str, k: Kind| d.vertices.iter().any(|v| v.id == id && v.kind == k);
    assert!(has("fixture_core::IntOrFloat", Kind::Union));
    assert!(has("fixture_core::Color", Kind::Enum));
    assert!(has("fixture_core::GLOBAL_SEED", Kind::Static));
    assert!(has("fixture_core::Score", Kind::TypeAlias));
    assert!(has("fixture_core::BASE", Kind::Const));
    assert!(has("fixture_core::local_shout", Kind::Macro));
    assert!(has("fixture_core::nested", Kind::Module));
    assert!(has("fixture_core::Named::name", Kind::Method));
    // impl 블록 메서드와 트레이트 impl 메서드 ID 형태가 다르다.
    assert!(has("fixture_core::Used::doubled", Kind::Method));
    assert!(has("fixture_core::Used::<Greet>::greet", Kind::Method));
}

#[test]
fn local_macro_and_variant_and_rename_resolve() {
    let d = doc(&source::Options {
        symbol_level: true,
        ..Default::default()
    });
    // 매크로 토큰 안의 호출: entry -> local_shout.
    assert!(d.edges.iter().any(|e| e.from == "fixture_core::entry"
        && e.to == "fixture_core::local_shout"
        && e.kind == EdgeKind::Call));
    // E::V 배리언트는 앞 타입으로의 참조.
    assert!(d.edges.iter().any(|e| e.from == "fixture_app::main"
        && e.to == "fixture_core::Color"
        && e.kind == EdgeKind::References));
    // `use X as Y` 별칭을 통한 구조체 리터럴.
    assert!(d.edges.iter().any(|e| e.from == "fixture_app::main"
        && e.to == "fixture_core::Used"
        && e.kind == EdgeKind::References));
}

#[test]
fn no_mangle_is_retention_root() {
    let d = doc(&source::Options {
        symbol_level: true,
        ..Default::default()
    });
    assert!(d.roots.contains(&"fixture_core::ffi_entry".to_string()));
}

#[test]
fn inherent_impl_methods_and_scoped_self_call() {
    let d = doc(&source::Options {
        symbol_level: true,
        ..Default::default()
    });
    // self.doubled() — enclosing 타입으로 좁힌 추정 호출.
    assert!(d
        .edges
        .iter()
        .any(|e| e.from == "fixture_core::Used::quadrupled"
            && e.to == "fixture_core::Used::doubled"
            && e.kind == EdgeKind::Call
            && e.tentative));
}
