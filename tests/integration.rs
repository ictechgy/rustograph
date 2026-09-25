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
fn unparsable_cfg_attr_is_measured_and_silent_otherwise() {
    // 읽지 못한 cfg_attr 속성 목록은 limitation으로 드러나야 한다 —
    // 안쪽 경로가 참조로 안 잡혀 속성으로만 쓰는 dep이 미사용으로 보인다.
    // rustc도 거부하는 입력이라 공유 fixture 대신 임시 크레이트를 쓴다.
    let tmp = std::env::temp_dir().join(format!("rg-cfgattr-{}", std::process::id()));
    std::fs::create_dir_all(tmp.join("src")).unwrap();
    std::fs::write(
        tmp.join("Cargo.toml"),
        "[package]\nname = \"bad_attr\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        tmp.join("src/lib.rs"),
        "#[cfg_attr(test, 1 + 2)]\npub fn f() {}\n",
    )
    .unwrap();
    let d = source::load(&tmp, &Default::default()).expect("temp crate harvest failed");
    let _ = std::fs::remove_dir_all(&tmp);
    assert!(
        d.limitations
            .iter()
            .any(|l| l.starts_with("1 cfg_attr attribute lists could not be parsed")),
        "{:?}",
        d.limitations
    );
    // 셀 것이 없으면 조용해야 한다.
    let clean = doc(&Default::default());
    assert!(!clean.limitations.iter().any(|l| l.contains("cfg_attr")));
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
        // 없는 정점은 오류 — 후보가 있으면 함께 보여준다.
        let (code, _, err) = run(&["query", "fixture_core::nope"]);
        assert_eq!(code, 2);
        assert!(err.contains("not found"));
        // 애매한 부분 문자열은 조용히 고르지 않고 후보를 열거한다 —
        // `nest`는 nest/deep/leaf 등 여러 정점에 걸린다.
        let (code, _, err2) = run(&["query", "nest"]);
        assert_eq!(code, 2);
        assert!(err2.contains("did you mean"));
        // 위치 인자 없으면 오류.
        let (code, _, _) = run(&["query"]);
        assert_eq!(code, 2);
    }

    #[test]
    fn paths_and_search() {
        // main -> entry 호출 경로가 발견된다.
        let (code, out, _) = run(&["paths", "fixture_app::main", "fixture_core::entry"]);
        assert_eq!(code, 0);
        assert!(out.contains("\"found\": true"));
        assert!(out.contains("fixture_app::main"));
        // 없는 끝점은 사용법 오류가 아니라 분석 오류(2)다.
        let (code, _, err) = run(&["paths", "fixture_app::main", "fixture_core::nope"]);
        assert_eq!(code, 2);
        assert!(err.contains("not found"));
        // 인자 부족도 2.
        let (code, _, _) = run(&["paths", "fixture_app::main"]);
        assert_eq!(code, 2);
        // search — `::entry` 꼬리 일치가 먼저 나온다.
        let (code, out, _) = run(&["search", "entry"]);
        assert_eq!(code, 0);
        assert!(out.contains("\"fixture_core::entry\""));
        let (code, _, _) = run(&["search"]);
        assert_eq!(code, 2);
    }

    #[test]
    fn invalid_numeric_limits_are_errors() {
        // 잘못된 한계를 기본값으로 되돌리면 사용자가 준 한계가 무시된다 —
        // 모두 사용법 오류(2)다.
        for args in [
            &[
                "paths",
                "fixture_app::main",
                "fixture_core::entry",
                "--max",
                "abc",
            ][..],
            &[
                "paths",
                "fixture_app::main",
                "fixture_core::entry",
                "--budget",
                "-1",
            ][..],
            &["search", "entry", "--max", "1.5"][..],
            &["query", "fixture_core::entry", "--depth", "x"][..],
        ] {
            let (code, _, err) = run(args);
            assert_eq!(code, 2, "{args:?}: {err}");
            assert!(err.contains("invalid"), "{args:?}: {err}");
        }
        // 올바른 값은 그대로 동작한다.
        let (code, out, _) = run(&[
            "paths",
            "fixture_app::main",
            "fixture_core::entry",
            "--max",
            "1",
        ]);
        assert_eq!(code, 0);
        assert!(out.contains("\"found\": true"));
    }

    #[test]
    fn deps_rejects_document_filters() {
        // deps는 자체 수확을 쓴다 — 필터·저장 그래프·semantic은
        // 사용 증거를 바꿔 거짓 미사용을 만들 수 있어 거부한다.
        for args in [
            &["deps", "--focus", "fixture_app"][..],
            &["deps", "--target", "x86_64-pc-windows-msvc"][..],
            &["deps", "--exclude-tests"][..],
            &["deps", "--semantic"][..],
        ] {
            let (code, _, err) = run(args);
            assert_eq!(code, 2, "{args:?}: {err}");
            assert!(err.contains("not supported"), "{args:?}: {err}");
        }
    }

    #[test]
    fn deps_reports_unused_declared_dep() {
        // fixture_unused는 선언만 됐다 — 미사용 판정이 잡혀야 한다.
        let (code, out, _) = run(&["deps"]);
        assert_eq!(code, 0);
        assert!(out.contains("fixture_app -> fixture_unused"));
        // fixture_macros는 속성 경로(#[fixture_macros::x])로 관측돼 쓰인다.
        assert!(!out.contains("fixture_macros (kind"));
        let (code, out, _) = run(&["deps", "--format", "json"]);
        assert_eq!(code, 0);
        assert!(out.contains("\"fixture_unused\""));
        // strict는 미사용 발견이면 1.
        let (code, _, _) = run(&["deps", "--strict"]);
        assert_eq!(code, 1);
    }

    #[test]
    fn filters_focus_exclude_tests_target() {
        // --focus — 서브트리만 남고 조상·형제는 사라진다.
        let (code, out, _) = run(&["graph", "--level", "symbol", "--focus", "fixture_core::t"]);
        assert_eq!(code, 0);
        assert!(out.contains("fixture_core::t"));
        assert!(!out.contains("fixture_app"));
        // --exclude-tests — #[cfg(test)] 모듈과 그 내용이 빠진다.
        let (code, out, _) = run(&["graph", "--level", "symbol", "--exclude-tests"]);
        assert_eq!(code, 0);
        assert!(!out.contains("\"fixture_core::t\""));
        assert!(!out.contains("fixture_core::t::t1"));
        // --tests와 --exclude-tests는 공존 불가 — 사용법 오류.
        let (code, _, _) = run(&["dead", "--tests", "--exclude-tests"]);
        assert_eq!(code, 2);
        // --target — unix 게이트 정점은 windows 트리플에서 빠진다.
        let (code, out, _) = run(&[
            "graph",
            "--level",
            "symbol",
            "--target",
            "x86_64-pc-windows-msvc",
        ]);
        assert_eq!(code, 0);
        assert!(!out.contains("\"fixture_core::unix_only\""));
        assert!(out.contains("cfg-gated vertices excluded"));
    }

    #[test]
    fn rules_baseline_freezes_existing_violations() {
        let tmp = std::env::temp_dir().join(format!("rg-base-{}", std::process::id()));
        let cfg = tmp.with_extension("yml");
        let base = tmp.with_extension("txt");
        // app이 core에 의존하면 안 되는 규칙 — fixture는 위반이 있다.
        let mut f = std::fs::File::create(&cfg).unwrap();
        writeln!(
            f,
            "components:\n  app: [\"fixture_app/**\"]\n  core: [\"fixture_core/**\"]\ndeps:\n  app: []\n  core: []"
        )
        .unwrap();
        drop(f);
        let cfgp = cfg.to_str().unwrap().to_string();
        let basep = base.to_str().unwrap().to_string();
        // baseline 없이 strict → 위반이 있어 1.
        let (code, _, _) = run(&["rules", "--strict", "--config", &cfgp]);
        assert_eq!(code, 1);
        // 얼리기 — 현재 위반을 기준선으로 기록하고 0을 돌린다.
        let (code, out, _) = run(&[
            "rules",
            "--config",
            &cfgp,
            "--baseline",
            &basep,
            "--write-baseline",
        ]);
        assert_eq!(code, 0);
        assert!(out.contains("wrote baseline"));
        // 얼린 뒤 strict → 기존 위반은 억눌려 0.
        let (code, out, _) = run(&["rules", "--strict", "--config", &cfgp, "--baseline", &basep]);
        assert_eq!(code, 0);
        assert!(out.contains("suppressed by baseline"));
        // 없는 명시 baseline 파일은 오타일 수 있으니 오류.
        let (code, _, _) = run(&[
            "rules",
            "--config",
            &cfgp,
            "--baseline",
            "/nonexistent-xyz.txt",
        ]);
        assert_eq!(code, 2);
        // baseline 경로가 디렉터리면 NotFound가 아니다 — 설정에서 온
        // 경로여도 읽기 실패는 조용히 넘기지 않고 오류다.
        let (code, _, err) = run(&["rules", "--config", &cfgp, "--baseline", "/tmp"]);
        assert_eq!(code, 2);
        assert!(err.contains("cannot read baseline"));
        let _ = std::fs::remove_file(&cfg);
        let _ = std::fs::remove_file(&base);
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
fn cfg_conditions_are_metadata_on_vertices_and_edges() {
    let d = doc(&source::Options {
        symbol_level: true,
        ..Default::default()
    });
    let v = |id: &str| d.vertices.iter().find(|v| v.id == id).unwrap();
    // 아이템·모듈의 자체 #[cfg]가 정점에 실린다 — 토큰 그대로.
    assert_eq!(
        v("fixture_core::cfg_gated").cfg.as_deref(),
        Some("feature = \"never\"")
    );
    assert_eq!(v("fixture_core::unix_only").cfg.as_deref(), Some("unix"));
    assert_eq!(v("fixture_core::t").cfg.as_deref(), Some("test"));
    // 조건 없는 정점은 키가 없다 — "무조건"과 "모름"을 구분하는 계약.
    assert!(v("fixture_core::entry").cfg.is_none());
    // cfg-gated use → uses 간선에 조건이 실린다.
    let e = d
        .edges
        .iter()
        .find(|e| {
            e.from == "fixture_app" && e.to == "fixture_core::cfg_gated" && e.kind == EdgeKind::Uses
        })
        .unwrap();
    assert_eq!(e.cfg.as_deref(), Some("feature = \"never\""));
    // 조건부 mod의 contains 간선도 조건을 물려받는다.
    let e = d
        .edges
        .iter()
        .find(|e| {
            e.from == "fixture_core"
                && e.to == "fixture_core::unix_only"
                && e.kind == EdgeKind::Contains
        })
        .unwrap();
    assert_eq!(e.cfg.as_deref(), Some("unix"));
}

#[test]
fn unsafe_boundaries_mark_vertices_and_entry_edges() {
    let d = doc(&source::Options {
        symbol_level: true,
        ..Default::default()
    });
    let v = |id: &str| d.vertices.iter().find(|v| v.id == id).unwrap();
    // unsafe fn / unsafe 블록 포함 본문 / unsafe trait / unsafe fn 선언.
    assert!(v("fixture_core::raw_read").unsafe_);
    assert!(v("fixture_core::safe_wrapper").unsafe_);
    assert!(v("fixture_core::RawBytes").unsafe_);
    assert!(v("fixture_core::PtrMath::deref_raw").unsafe_);
    assert!(!v("fixture_core::entry").unsafe_);
    // unsafe 블록 안의 호출은 경계 진입 간선이다.
    let e = d
        .edges
        .iter()
        .find(|e| {
            e.from == "fixture_core::safe_wrapper"
                && e.to == "fixture_core::raw_read"
                && e.kind == EdgeKind::Call
        })
        .unwrap();
    assert!(e.unsafe_);
    // unsafe impl의 implements 간선도 경계다.
    let e = d
        .edges
        .iter()
        .find(|e| {
            e.from == "fixture_core::Used"
                && e.to == "fixture_core::RawBytes"
                && e.kind == EdgeKind::Implements
        })
        .unwrap();
    assert!(e.unsafe_);
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
