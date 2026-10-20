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
    let d = doc(&Default::default()).view(Level::Crate).unwrap();
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
    let rep = analysis::dead(&d).unwrap();
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
    let rep = analysis::dead(&d).unwrap();
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
    .view(Level::Module)
    .unwrap();
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

/// 라이브러리 호출자가 쓰는 0.5.0 공개 시그니처 그대로다 — 인자를 늘리면 이 외부 크레이트
/// 테스트가 컴파일되지 않는다. 드라이버 버전은 호출자가 넘기지 않아도 이 크레이트의 버전이다.
#[test]
fn sarif_keeps_its_published_library_signature() {
    let render: fn(&rustograph::graph::Document, &[rustograph::rules::Violation]) -> String =
        rustograph::sarif::rules_sarif;
    let log: serde_json::Value =
        serde_json::from_str(&render(&doc(&Default::default()), &[])).expect("SARIF must be JSON");
    assert_eq!(
        log["runs"][0]["tool"]["driver"]["version"],
        env!("CARGO_PKG_VERSION")
    );
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

    /// 질의 인자에는 문서화된 상한이 있다 — query·impact·search `--max` 10,000, query·impact `--depth`
    /// 64(0은 깊이 제한 없음), paths `--max` 1,024·`--budget` 1,000,000. 상한까지는 그대로 받고, 1만 넘어도
    /// 조용히 깎지 않고 사용법 오류(2)다. 예전에는 상한이 없어 `paths --budget`·`--max` 하나로 메모리를
    /// 수 GB까지 쓸 수 있었다. 순회 형식의 자체 상한(`--depth`/`--max-depth` 1~128)은 그대로다.
    #[test]
    fn query_arguments_past_their_limits_are_usage_errors() {
        let (from, to) = ("fixture_app::main", "fixture_core::entry");
        for (args, flag, limit) in [
            (&["query", to, "--max", "10001"][..], "--max 10001", "10000"),
            (&["query", to, "--depth", "65"][..], "--depth 65", "64"),
            (
                &["impact", to, "--max", "10001"][..],
                "--max 10001",
                "10000",
            ),
            (&["impact", to, "--depth", "65"][..], "--depth 65", "64"),
            (
                &["search", "entry", "--max", "10001"][..],
                "--max 10001",
                "10000",
            ),
            (
                &["paths", from, to, "--max", "1025"][..],
                "--max 1025",
                "1024",
            ),
            (
                &["paths", from, to, "--budget", "1000001"][..],
                "--budget 1000001",
                "1000000",
            ),
            (
                &["paths", from, to, "--budget", "1000000000000"][..],
                "--budget 1000000000000",
                "1000000",
            ),
        ] {
            let (code, out, err) = run(args);
            assert_eq!(code, 2, "{args:?}: {err}");
            assert!(out.is_empty(), "{args:?}: {out}");
            assert!(
                err.contains(flag) && err.contains(&format!("limit of {limit}")),
                "{args:?}: {err}"
            );
        }
        for args in [
            &["query", to, "--depth", "64", "--max", "10000"][..],
            &["query", to, "--depth", "0", "--max", "10000"][..],
            &["impact", to, "--depth", "64", "--max", "10000"][..],
            &["search", "entry", "--max", "10000"][..],
            &["paths", from, to, "--max", "1024", "--budget", "1000000"][..],
            &[
                "impact",
                "--format",
                "language-traversal",
                to,
                "--depth",
                "128",
            ][..],
        ] {
            let (code, _, err) = run(args);
            assert_eq!(code, 0, "{args:?}: {err}");
        }
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

    /// 명령이 쓰지 않는 플래그·위치 인자는 조용히 무시하지 않고 사용법 오류(2)다. `cycles --out`은
    /// 파일을 쓰지 않은 채 0으로 끝나 CI의 다음 단계가 이전 실행의 파일을 읽었고, `query --format
    /// mermaid`는 JSON을 냈다. 오류는 명령과 그 플래그(인자)를 대고, 수확 전에 끝나 표준 출력이 비어 있다.
    #[test]
    fn commands_reject_flags_and_arguments_they_do_not_use() {
        let tmp = std::env::temp_dir().join(format!("rg-unused-{}", std::process::id()));
        let out = tmp.join("written.json");
        let out = out.to_str().unwrap();
        for (args, unused) in [
            (&["cycles", "--format", "json", "--out", out][..], "--out"),
            (
                &["query", "fixture_core::entry", "--format", "mermaid"][..],
                "--format",
            ),
            (
                &["impact", "fixture_core::Used", "--format", "json"][..],
                "--format",
            ),
            (
                &["impact", "fixture_core::Used", "--max-depth", "3"][..],
                "--max-depth",
            ),
            (
                &[
                    "paths",
                    "fixture_app::main",
                    "fixture_core::entry",
                    "--role",
                    "server",
                ][..],
                "--role",
            ),
            (&["search", "entry", "--budget", "5"][..], "--budget"),
            (&["dead", "--level", "module"][..], "--level"),
            (&["graph", "--strict"][..], "--strict"),
            (&["rules", "--depth", "3"][..], "--depth"),
            (&["cycles", "--root", "fixture_app::main"][..], "--root"),
            (&["deps", "--out", out][..], "--out"),
            (&["cycles", "stray"][..], "\"stray\""),
            (
                &["query", "fixture_core::entry", "fixture_core::Used"][..],
                "\"fixture_core::Used\"",
            ),
            (
                &["paths", "fixture_app::main", "fixture_core::entry", "x"][..],
                "\"x\"",
            ),
            (&["search", "entry", "more"][..], "\"more\""),
        ] {
            let (code, stdout, err) = run(args);
            assert_eq!(code, 2, "{args:?}: {err}");
            assert!(stdout.is_empty(), "{args:?}: {stdout}");
            assert!(
                err.contains(args[0]) && err.contains(unused),
                "{args:?}: {err}"
            );
        }
        assert!(!std::path::Path::new(out).exists());
    }

    /// 거부는 명령이 쓰지 않는 조합만이다 — USAGE의 공유 플래그와 `--dir`·`--graph`는 문서를 얻는 모든
    /// 명령이 계속 받고, 명령 고유 플래그(`graph --root`·`dead --root`, `deps --deps`)도 그대로다.
    #[test]
    fn commands_keep_accepting_documented_shared_flags() {
        let shared = [
            "--deps",
            "--tests",
            "--retain-public",
            "--no-cache",
            "--focus",
            "fixture_core",
            "--target",
            "x86_64-pc-windows-msvc",
        ];
        let cfg = std::env::temp_dir().join(format!("rg-shared-{}.yml", std::process::id()));
        std::fs::write(
            &cfg,
            "components:\n  core: [\"fixture_core/**\"]\ndeps:\n  core: []\n",
        )
        .unwrap();
        let cfg = cfg.to_str().unwrap().to_string();
        for command in [
            &[
                "graph",
                "--level",
                "symbol",
                "--root",
                "fixture_core::entry",
            ][..],
            &[
                "cycles", "--level", "module", "--format", "json", "--strict",
            ][..],
            &["dead", "--root", "fixture_core::entry", "--format", "json"][..],
            &["query", "fixture_core::entry", "--depth", "2", "--max", "5"][..],
            &["impact", "fixture_core::Used", "--depth", "0"][..],
            &[
                "paths",
                "fixture_core::entry",
                "fixture_core::Used",
                "--budget",
                "9",
            ][..],
            &["search", "entry", "--max", "3"][..],
            &["rules", "--config", &cfg, "--format", "json"][..],
        ] {
            let args: Vec<&str> = command.iter().chain(&shared).copied().collect();
            let (code, _, err) = run(&args);
            assert_eq!(code, 0, "{args:?}: {err}");
            let args: Vec<&str> = command
                .iter()
                .chain(&["--exclude-tests"])
                .copied()
                .collect();
            let (code, _, err) = run(&args);
            assert_eq!(code, 0, "{args:?}: {err}");
        }
        let (code, _, err) = run(&["deps", "--deps", "--format", "json"]);
        assert_eq!(code, 0, "{err}");
        std::fs::remove_file(&cfg).unwrap();
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

    /// README의 기준선 흐름 — 설정 옆 `.rustograph/rules-baseline.txt`가 아직 없으면 도입
    /// 전으로 보고, `--write-baseline`이 디렉터리째 만들어 얼린 뒤 strict가 억누른다.
    #[test]
    fn rules_baseline_under_dot_rustograph_round_trips() {
        let tmp = std::env::temp_dir().join(format!("rg-dot-base-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        std::fs::create_dir_all(&tmp).unwrap();
        let cfg = tmp.join(".rustograph.yml");
        std::fs::write(
            &cfg,
            "components:\n  app: [\"fixture_app/**\"]\n  core: [\"fixture_core/**\"]\n\
             deps:\n  app: []\n  core: []\nbaseline: .rustograph/rules-baseline.txt\n",
        )
        .unwrap();
        let cfgp = cfg.to_str().unwrap();
        let (code, _, err) = run(&["rules", "--strict", "--config", cfgp]);
        assert_eq!(code, 1, "{err}");
        let (code, out, err) = run(&["rules", "--config", cfgp, "--write-baseline"]);
        assert_eq!(code, 0, "{err}");
        assert!(out.contains("wrote baseline"));
        let frozen = std::fs::read_to_string(tmp.join(".rustograph/rules-baseline.txt")).unwrap();
        assert!(frozen.contains("allow|fixture_app"), "{frozen}");
        let (code, out, err) = run(&["rules", "--strict", "--config", cfgp]);
        assert_eq!(code, 0, "{err}");
        assert!(out.contains("suppressed by baseline"));
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// 설정의 `baseline:`과 디렉터리에서 찾은 `.rustograph.yml`은 분석 대상 저장소가 고른다 —
    /// FIFO는 여는 순간 쓰는 쪽을 영원히 기다리고 장치는 끝없이 읽힌다. rules는 막히거나
    /// 메모리를 다 쓰지 않고 원인을 담은 오류(2)로 끝나야 한다. 명시 `--baseline`은 실행한
    /// 사람이 고른 입력이라 파이프도 읽지만, 장치를 주어도 크기 상한에서 2로 끝난다.
    #[cfg(unix)]
    #[test]
    fn rules_refuses_special_config_and_baseline_files() {
        let tmp = std::env::temp_dir().join(format!("rg-special-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        std::fs::create_dir_all(&tmp).unwrap();
        let fifo = tmp.join("pipe");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo must run");
        assert!(made.success());
        let cfg = tmp.join("rules.yml");
        std::fs::write(
            &cfg,
            "components:\n  app: [\"fixture_app/**\"]\ndeps:\n  app: []\nbaseline: pipe\n",
        )
        .unwrap();
        let cfgp = cfg.to_str().unwrap();
        for (args, cause, why) in [
            // 설정의 baseline이 FIFO — 열지 않는다.
            (
                vec!["rules", "--config", cfgp],
                "cannot read baseline ",
                "not a regular file",
            ),
            // 명시 --baseline이 장치 — 읽되 상한에서 끝난다.
            (
                vec!["rules", "--config", cfgp, "--baseline", "/dev/zero"],
                "cannot read baseline /dev/zero: ",
                "larger than",
            ),
        ] {
            let (code, _, err) = run(&args);
            assert_eq!(code, 2, "{args:?}: {err}");
            assert!(err.contains(cause) && err.contains(why), "{args:?}: {err}");
        }
        // --dir에서 찾은 .rustograph.yml이 FIFO여도 같다 — 설정을 읽은 뒤에야 수확한다.
        std::fs::remove_file(&cfg).unwrap();
        std::fs::rename(&fifo, tmp.join(".rustograph.yml")).unwrap();
        let argv: Vec<String> = ["rules", "--dir", tmp.to_str().unwrap()]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = cli::run(&argv, &mut out, &mut err);
        let err = String::from_utf8(err).unwrap();
        assert_eq!(code, 2, "{err}");
        assert!(err.contains("not a regular file"), "{err}");
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// 명시 `--config`·`--baseline`은 실행한 사람이 고른 입력이라 파이프도 읽는다 —
    /// `cat rules.yml | rustograph rules --config /dev/stdin`과 `--baseline <(…)`는 셸에서 흔한
    /// 쓰임이다. 정규 파일만 받으면 이런 실행의 종료 코드가 1·0에서 2로 바뀐다.
    #[cfg(unix)]
    #[test]
    fn rules_reads_piped_config_and_baseline_arguments() {
        let tmp = std::env::temp_dir().join(format!("rg-piped-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        std::fs::create_dir_all(&tmp).unwrap();
        // app이 core를 참조하지 못하게 거꾸로 선 룰셋 — fixture에 위반이 생긴다.
        let deny = "components:\n  app: [\"fixture_app/**\"]\n  core: [\"fixture_core/**\"]\n\
                    deps:\n  app: []\n  core: []\n";
        let cfg = tmp.join("deny.yml");
        std::fs::write(&cfg, deny).unwrap();
        let base = tmp.join("base.txt");
        let (cfgp, basep) = (cfg.to_str().unwrap(), base.to_str().unwrap());
        let (code, _, err) = run(&[
            "rules",
            "--config",
            cfgp,
            "--baseline",
            basep,
            "--write-baseline",
        ]);
        assert_eq!(code, 0, "{err}");
        let frozen = std::fs::read_to_string(&base).unwrap();
        // 설정을 표준 입력 파이프로 — 위반이 남으니 strict는 1이다.
        let (code, err) = piped(&["rules", "--strict", "--config", "/dev/stdin"], deny);
        assert_eq!(code, Some(1), "{err}");
        // 기준선을 파이프로 — 얼린 위반이 억눌려 0이다(읽지 않았다면 1).
        let (code, err) = piped(
            &[
                "rules",
                "--strict",
                "--config",
                cfgp,
                "--baseline",
                "/dev/stdin",
            ],
            &frozen,
        );
        assert_eq!(code, Some(0), "{err}");
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// 실제 바이너리를 fixture에 돌리며 `input`을 표준 입력 파이프로 준다 — (종료 코드, stderr).
    #[cfg(unix)]
    fn piped(args: &[&str], input: &str) -> (Option<i32>, String) {
        use std::process::{Command, Stdio};
        let mut child = Command::new(env!("CARGO_BIN_EXE_rustograph"))
            .args(args)
            .arg("--dir")
            .arg(fixture())
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("rustograph binary must start");
        let mut stdin = child.stdin.take().expect("stdin is piped");
        match stdin.write_all(input.as_bytes()) {
            Ok(()) => {}
            // 파이프를 읽지 않고 먼저 끝난 실행 — 그 이유는 종료 코드와 stderr가 말한다.
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
            Err(e) => panic!("writing the piped input failed: {e}"),
        }
        drop(stdin);
        let output = child.wait_with_output().expect("rustograph must finish");
        (
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).into_owned(),
        )
    }

    /// 다른 도구가 만든 `--graph` 문서의 ID에 ESC·개행이 섞여 있으면 어떤 명령도 그 ID를
    /// 텍스트·Mermaid·순회 출력에 옮기지 않고 입력 오류(2)로 끝난다. 오류는 한 줄이고
    /// 그 문자를 싣지 않는다 — 터미널 화면 지우기나 Mermaid `click` 줄이 끼어들 수 없다.
    #[test]
    fn tainted_graph_documents_are_input_errors() {
        let tmp = std::env::temp_dir().join(format!("rg-tainted-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let path = tmp.join("g.json");
        // JSON 이스케이프(\u001b·\n·\")로 싣는다 — 읽으면 실제 ESC·개행·따옴표다.
        let tainted = r#"c::\u001b[2Jx\"]\n    click x"#;
        std::fs::write(
            &path,
            format!(
                r#"{{"version":1,"tool":"rustograph","level":"symbol","root":".","roots":["c::r"],
"vertices":[{{"id":"c::r","kind":"fn","crate":"c","module":"c"}},
{{"id":"{tainted}","kind":"fn","crate":"c","module":"c"}}],
"edges":[{{"from":"c::r","to":"{tainted}","kind":"call"}},{{"from":"{tainted}","to":"c::r","kind":"call"}}]}}"#
            ),
        )
        .unwrap();
        let g = path.to_str().unwrap();
        for args in [
            vec![
                "graph", "--level", "symbol", "--format", "mermaid", "--graph", g,
            ],
            vec!["cycles", "--level", "symbol", "--graph", g],
            vec!["dead", "--explain", "c::r", "--graph", g],
            vec!["reach", "c::r", "--graph", g],
        ] {
            let (code, out, err) = run(&args);
            assert_eq!(code, 2, "{args:?}: {err:?}");
            assert!(out.is_empty(), "{args:?}: {out:?}");
            assert!(
                err.contains("vertices[1].id contains U+001B"),
                "{args:?}: {err:?}"
            );
            assert_eq!(err.lines().count(), 1, "{args:?}: {err:?}");
            assert!(!err.contains('\u{1b}'), "{args:?}: {err:?}");
        }
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}

/// 실제 바이너리로 `mcp <args>`를 띄워 도구 하나를 부른다 — (isError 여부, 답 텍스트).
fn mcp_tool(args: &[&std::ffi::OsStr], tool: &str) -> (bool, String) {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(env!("CARGO_BIN_EXE_rustograph"))
        .arg("mcp")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("rustograph binary must start");
    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": tool, "arguments": {}},
    });
    writeln!(child.stdin.take().expect("stdin is piped"), "{call}").unwrap();
    let output = child.wait_with_output().expect("rustograph must finish");
    assert!(
        output.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let result = &response["result"];
    (
        result["isError"] == true,
        result["content"][0]["text"].as_str().unwrap().to_string(),
    )
}

/// 같은 프로세스에서 CLI를 돌린 표준 출력 — 종료 코드가 0이 아니면 stderr로 실패한다.
fn cli_stdout(args: &[&str]) -> String {
    let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = rustograph::cli::run(&argv, &mut out, &mut err);
    assert_eq!(code, 0, "{args:?}: {}", String::from_utf8_lossy(&err));
    String::from_utf8(out).unwrap()
}

/// 의존성 없는 단일 패키지 워크스페이스를 `dir`에 만든다.
fn lone_workspace(dir: &std::path::Path, name: &str) {
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n"),
    )
    .unwrap();
    std::fs::write(dir.join("src/lib.rs"), "pub fn f() {}\n").unwrap();
}

/// `mcp --graph`는 저장 스냅샷을 서빙하지만 rustograph_deps는 `--dir` 워크스페이스를 따로 수확한다 —
/// 그 워크스페이스가 스냅샷이 기록한 `workspace`(cargo가 준 절대 경로)와 같은 디렉터리일 때만 답한다.
/// 예전에는 다른 저장소의 deps를 isError 없이 답해, 에이전트가 같은 세션의 summary·query와 다른
/// 코드를 말하는 답을 구분할 수 없었다. 기록이 없거나 절대 경로가 아니면 같음을 확인할 수 없어
/// isError다. 스냅샷이 고른 경로에서는 cargo를 실행하지 않는다(Cargo.lock이 생기지 않는다).
#[test]
fn mcp_deps_answers_only_for_the_snapshot_workspace() {
    use std::ffi::OsStr;
    let tmp = std::env::temp_dir().join(format!("rg-mcp-deps-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let fixture = fixture();
    let fixture_dir = fixture.to_str().unwrap();
    let (other, decoy) = (tmp.join("other"), tmp.join("decoy"));
    lone_workspace(&other, "other");
    lone_workspace(&decoy, "decoy");
    // 표준 출력 graph는 git을 띄우지 않는다 — fixture의 cargo 워크스페이스를 `workspace`로 싣는다.
    let mut snapshot: serde_json::Value = serde_json::from_str(&cli_stdout(&[
        "graph",
        "--level",
        "symbol",
        "--dir",
        fixture_dir,
    ]))
    .unwrap();
    let workspace = snapshot["workspace"].as_str().unwrap().to_string();
    assert_eq!(
        std::fs::canonicalize(&workspace).unwrap(),
        std::fs::canonicalize(&fixture).unwrap()
    );
    let mut write_snapshot = |name: &str, workspace: Option<&str>| {
        match workspace {
            Some(w) => snapshot["workspace"] = serde_json::json!(w),
            None => {
                snapshot.as_object_mut().unwrap().remove("workspace");
            }
        }
        let path = tmp.join(name);
        std::fs::write(&path, snapshot.to_string()).unwrap();
        path
    };
    let same = write_snapshot("same.json", Some(&workspace));
    let unrecorded = write_snapshot("unrecorded.json", None);
    let relative = write_snapshot("relative.json", Some("."));
    let elsewhere = write_snapshot("elsewhere.json", Some(decoy.to_str().unwrap()));
    let missing = write_snapshot("missing.json", Some(tmp.join("gone").to_str().unwrap()));
    let expected = cli_stdout(&["deps", "--format", "json", "--dir", fixture_dir]);
    let graph = OsStr::new("--graph");
    let dir = OsStr::new("--dir");
    // 같은 워크스페이스 — CLI deps와 같은 답이다.
    let answer = mcp_tool(
        &[graph, same.as_os_str(), dir, fixture.as_os_str()],
        "rustograph_deps",
    );
    assert_eq!(answer, (false, expected.clone()));
    // 다른 워크스페이스·기록 없음·상대 경로 기록·다른 디렉터리·없는 디렉터리 기록은 isError — 원인과
    // 해결을 싣는다.
    let refused = [
        (&same, other.as_os_str(), "not the workspace"),
        (&unrecorded, fixture.as_os_str(), "does not record"),
        (&relative, fixture.as_os_str(), "does not record"),
        (&elsewhere, fixture.as_os_str(), "not the workspace"),
        (&missing, fixture.as_os_str(), "not the workspace"),
    ];
    for (snapshot, dir_value, cause) in refused {
        let (is_error, text) = mcp_tool(
            &[graph, snapshot.as_os_str(), dir, dir_value],
            "rustograph_deps",
        );
        assert!(is_error, "{snapshot:?} --dir {dir_value:?}: {text}");
        assert!(text.contains(cause), "{text}");
        assert!(text.contains("without --graph"), "{text}");
    }
    // 스냅샷이 고른 디렉터리에서는 cargo metadata를 돌리지 않았다.
    assert!(!decoy.join("Cargo.lock").exists());
    // `--graph` 없이 띄운 세션은 지금처럼 `--dir`을 수확해 답한다.
    let (is_error, text) = mcp_tool(&[dir, other.as_os_str()], "rustograph_deps");
    assert!(!is_error, "{text}");
    // 같은 디렉터리의 다른 표기(심볼릭 링크)도 같은 워크스페이스다 — cargo는 표기를 그대로 돌려준다.
    #[cfg(unix)]
    {
        let alias = tmp.join("alias");
        std::os::unix::fs::symlink(&fixture, &alias).unwrap();
        let answer = mcp_tool(
            &[graph, same.as_os_str(), dir, alias.as_os_str()],
            "rustograph_deps",
        );
        assert_eq!(answer, (false, expected));
    }
    std::fs::remove_dir_all(&tmp).unwrap();
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

/// 표현식의 단일 식별자는 지역 묶음이 아이템보다 먼저다(rustc 값 이름공간).
/// 같은 이름의 크레이트 루트 모듈·함수로 읽으면 가짜 references 간선이
/// 생기고, 순회가 그 모듈이 import한 아이템 전부로 번진다.
#[test]
fn locals_shadow_items_and_modules_are_never_values() {
    let tmp = std::env::temp_dir().join(format!("rg-locals-{}", std::process::id()));
    std::fs::create_dir_all(tmp.join("src")).unwrap();
    std::fs::write(
        tmp.join("Cargo.toml"),
        "[package]\nname = \"locals\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        tmp.join("src/lib.rs"),
        r#"
pub mod repo { pub struct Repo; impl Repo { pub fn all(&self) -> u32 { 1 } } }
pub fn helper() -> u32 { 2 }
pub fn item() -> u32 { 3 }
pub const MAX: u32 = 9;
pub struct S;
impl S { pub fn me(&self) -> u32 { self.me2() } pub fn me2(&self) -> u32 { 0 } }

pub fn param(repo: &repo::Repo) -> u32 { repo.all() }
pub fn let_shadow() -> u32 { let helper = helper(); helper + 1 }
pub fn closure() -> u32 { let f = |item: u32| item + 1; f(1) }
pub fn arms(x: Option<u32>) -> u32 {
    match x { Some(item) => item, None => MAX }
}
pub fn if_let(x: Option<u32>) -> u32 {
    if let Some(helper) = x { helper } else { helper() }
}
pub fn loops(v: Vec<u32>) -> u32 { let mut n = 0; for item in v { n += item; } n }
pub fn block_scope() -> u32 { { let item = 1; let _ = item; } item() }
pub fn capture(repo: u32) -> String { format!("{repo}") }
pub mod util { pub fn inner() {} }
pub fn util() -> u32 { 4 }
pub fn same_name() -> u32 { util() }
pub fn nested_fn() { let _ = { fn go(helper: u32) -> u32 { helper } go(1) }; }
"#,
    )
    .unwrap();
    let d = source::load(
        &tmp,
        &source::Options {
            symbol_level: true,
            ..Default::default()
        },
    )
    .expect("temp crate harvest failed");
    let _ = std::fs::remove_dir_all(&tmp);
    let has = |from: &str, to: &str| d.edges.iter().any(|e| e.from == from && e.to == to);
    // 매개변수·let·클로저·match 팔·if let·for 묶음은 아이템을 가린다.
    assert!(
        !has("locals::param", "locals::repo"),
        "param shadowed module"
    );
    assert!(!has("locals::closure", "locals::item"));
    assert!(!has("locals::arms", "locals::item"));
    assert!(!has("locals::loops", "locals::item"));
    assert!(!has("locals::capture", "locals::repo"), "format capture");
    // 초기식은 새 묶음을 못 본다 — `let helper = helper()`는 fn 호출이다.
    assert!(has("locals::let_shadow", "locals::helper"));
    // if let 묶음은 else에서 안 보인다.
    assert!(has("locals::if_let", "locals::helper"));
    // 대문자 식별자 패턴은 묶음이 아니다(상수 비교) — 참조는 남는다.
    assert!(has("locals::arms", "locals::MAX"));
    // 블록을 나가면 지역이 사라진다.
    assert!(has("locals::block_scope", "locals::item"));
    // 모듈과 같은 이름의 fn(다른 이름공간)은 값 자리에서 fn으로 남는다.
    assert!(
        has("locals::same_name", "locals::util"),
        "fn sharing a module name"
    );
    // 블록 안 fn의 매개변수도 지역이다.
    assert!(
        !has("locals::nested_fn", "locals::helper"),
        "nested fn param"
    );
    // `self` 수신자는 모듈이 아니다.
    assert!(!d
        .edges
        .iter()
        .any(|e| e.from == "locals::S::me" && e.to == "locals"));
}

/// src 아래 파일들로 임시 크레이트(`pathcycle` 패키지)를 만든다 — 공유 fixture의 계수를
/// 건드리지 않고 모듈 트리 모양 하나만 검증할 때 쓴다. 파일 이름의 디렉터리도 만든다.
fn temp_crate(tag: &str, files: &[(String, String)]) -> PathBuf {
    let tmp = std::env::temp_dir().join(format!("rg-modtree-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("src")).unwrap();
    std::fs::write(
        tmp.join("Cargo.toml"),
        "[package]\nname = \"pathcycle\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    for (name, text) in files {
        let path = tmp.join("src").join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    tmp
}

/// (이름, 내용) 쌍 목록을 소유 문자열로 바꾼다 — 표 테스트를 짧게 쓰기 위한 도우미.
fn owned(files: &[(&str, &str)]) -> Vec<(String, String)> {
    files
        .iter()
        .map(|(name, text)| (name.to_string(), text.to_string()))
        .collect()
}

/// 실제 CLI 경로(`graph --level symbol`)로 임시 크레이트를 수확한 JSON 문서다. 종료 코드가
/// 0이 아니면 그 코드와 stderr로 실패한다. 크레이트 디렉터리는 지운다.
fn graph_json(dir: &std::path::Path) -> serde_json::Value {
    let argv: Vec<String> = ["graph", "--level", "symbol", "--dir"]
        .iter()
        .map(|s| s.to_string())
        .chain(std::iter::once(dir.display().to_string()))
        .collect();
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = rustograph::cli::run(&argv, &mut out, &mut err);
    let _ = std::fs::remove_dir_all(dir);
    assert_eq!(code, 0, "{}", String::from_utf8_lossy(&err));
    serde_json::from_slice(&out).unwrap()
}

/// [`graph_json`]으로 수확해 (모듈 정점 ID 정렬 목록, limitation 목록)을 돌려준다.
fn graph_modules(dir: &std::path::Path) -> (Vec<String>, Vec<String>) {
    let doc = graph_json(dir);
    let mut modules: Vec<String> = doc["vertices"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| matches!(v["kind"].as_str(), Some("crate" | "module")))
        .map(|v| v["id"].as_str().unwrap().to_string())
        .collect();
    modules.sort();
    // 셀 것이 없으면 키가 빠진다 — 빈 목록과 같다.
    let limitations = doc["limitations"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|l| l.as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default();
    (modules, limitations)
}

/// 건너뛴 `mod` 선언을 세는 limitation 줄만 고른다.
fn skipped_mod_lines(limitations: &[String]) -> Vec<&str> {
    limitations
        .iter()
        .filter(|l| l.contains("mod declarations were skipped"))
        .map(String::as_str)
        .collect()
}

/// `#[path]`가 자기 파일이나 조상 모듈의 파일을 다시 싣는 선언(rustc의 circular modules)은
/// 정점 없이 한 번 센다. 방문 집합이 모듈 경로 기준이라 같은 파일이 `m`, `m::m`, …으로
/// 끝없이 늘었다(2줄 크레이트로 수 초에 RSS 수 GB). cfg로 막힌 변형은 rustc가 받으므로
/// 오류(2)가 아니라 limitation이다. 인라인 조상의 파일도 조상 파일이다.
#[test]
fn circular_path_modules_are_skipped_and_counted() {
    assert_one_circular(
        "self",
        &[(
            "lib.rs",
            "#[path = \"lib.rs\"]\npub mod m;\npub fn f() {}\n",
        )],
        &["pathcycle"],
    );
    assert_one_circular(
        "cfg",
        &[(
            "lib.rs",
            "#[cfg(feature = \"never-enabled\")]\n#[path = \"lib.rs\"]\npub mod again;\npub fn f() {}\n",
        )],
        &["pathcycle"],
    );
    assert_one_circular(
        "mutual",
        &[
            ("lib.rs", "pub mod a;\npub fn f() {}\n"),
            ("a.rs", "#[path = \"b.rs\"]\npub mod b;\npub fn in_a() {}\n"),
            ("b.rs", "#[path = \"a.rs\"]\npub mod a;\npub fn in_b() {}\n"),
        ],
        &["pathcycle", "pathcycle::a", "pathcycle::a::b"],
    );
    assert_one_circular(
        "inline",
        &[
            (
                "lib.rs",
                "pub mod outer {\n    #[path = \"../lib.rs\"]\n    pub mod back;\n}\npub fn f() {}\n",
            ),
            // `src/outer/../lib.rs`가 풀리려면 디렉터리가 있어야 한다(rustc도 같다).
            ("outer/.keep", ""),
        ],
        &["pathcycle", "pathcycle::outer"],
    );
}

/// lib과 같은 이름의 bin은 트리에서 루트 모듈 하나를 함께 쓰지만 rustc에게는 루트가 다른 두
/// 크레이트다. bin의 `mod lib;`(lib.rs를 `lib` 모듈로 싣는다 — rustc는 special_module_name
/// 경고만 낸다)와 lib의 `#[path = "main.rs"] mod m;`은 순환이 아니다 — 정점·간선이 그대로
/// 실리고 순환 limitation이 없어야 한다. 루트 파일을 모두 조상으로 보던 판정은 두 선언을
/// 순환으로 세어 `lib` 모듈과 `main → lib::run` 호출을 버렸다.
#[test]
fn bin_and_lib_of_one_name_may_load_each_others_root_file() {
    let doc = graph_json(&temp_crate(
        "binlib",
        &owned(&[
            (
                "lib.rs",
                "pub fn run() -> u32 {\n    helper()\n}\nfn helper() -> u32 {\n    1\n}\n",
            ),
            (
                "main.rs",
                "mod lib;\nfn main() {\n    println!(\"{}\", lib::run());\n}\n",
            ),
        ]),
    ));
    let ids: Vec<&str> = doc["vertices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["id"].as_str().unwrap())
        .collect();
    for id in [
        "pathcycle::lib",
        "pathcycle::lib::run",
        "pathcycle::lib::helper",
    ] {
        assert!(ids.contains(&id), "missing {id} in {ids:?}");
    }
    let calls_lib_run = doc["edges"].as_array().unwrap().iter().any(|e| {
        e["from"] == "pathcycle::main" && e["to"] == "pathcycle::lib::run" && e["kind"] == "call"
    });
    assert!(calls_lib_run, "main must call lib::run");
    let limitations: Vec<String> = doc["limitations"]
        .as_array()
        .map(|list| {
            list.iter()
                .map(|l| l.as_str().unwrap().to_string())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(skipped_mod_lines(&limitations), Vec::<&str>::new());
    assert!(
        !limitations
            .iter()
            .any(|l| l.contains("could not be resolved")),
        "{limitations:?}"
    );

    let (modules, limitations) = graph_modules(&temp_crate(
        "libpath",
        &owned(&[
            (
                "lib.rs",
                "#[path = \"main.rs\"]\nmod m;\npub fn run() -> u32 {\n    m::helper()\n}\n",
            ),
            (
                "main.rs",
                "pub fn helper() -> u32 {\n    1\n}\nfn main() {}\n",
            ),
        ]),
    ));
    assert_eq!(modules, ["pathcycle", "pathcycle::m"]);
    assert_eq!(skipped_mod_lines(&limitations), Vec::<&str>::new());
}

/// 같은 이름의 lib·bin에서도 진짜 순환은 끊긴다 — rustc가 circular modules로 거부하는
/// 입력이다. 서로의 루트 파일을 싣는 두 선언은 각 타깃의 사슬에서 자기 루트로 돌아오고,
/// bin 루트가 자기 파일을 싣는 선언은 bin 루트에서 끊긴다.
#[test]
fn circular_modules_across_one_name_lib_and_bin_are_still_cut() {
    let (modules, limitations) = graph_modules(&temp_crate(
        "crossroots",
        &owned(&[
            (
                "lib.rs",
                "#[path = \"main.rs\"]\nmod m;\npub fn run() -> u32 {\n    1\n}\n",
            ),
            (
                "main.rs",
                "mod lib;\nfn main() {\n    println!(\"{}\", lib::run());\n}\n",
            ),
        ]),
    ));
    assert_eq!(modules, ["pathcycle", "pathcycle::lib", "pathcycle::m"]);
    assert_eq!(
        skipped_mod_lines(&limitations),
        ["2 mod declarations were skipped because they re-enter an ancestor module's file (circular modules)"]
    );
    assert_one_circular(
        "binself",
        &[
            ("lib.rs", "pub fn run() -> u32 {\n    1\n}\n"),
            (
                "main.rs",
                "#[path = \"main.rs\"]\nmod again;\nfn main() {}\n",
            ),
        ],
        &["pathcycle"],
    );
}

/// 임시 크레이트가 종료 코드 0으로 모듈 정점 `want`와 순환 limitation 한 줄을 내는지 본다.
fn assert_one_circular(tag: &str, files: &[(&str, &str)], want: &[&str]) {
    let (modules, limitations) = graph_modules(&temp_crate(tag, &owned(files)));
    assert_eq!(modules, want, "{tag}");
    assert_eq!(
        skipped_mod_lines(&limitations),
        ["1 mod declarations were skipped because they re-enter an ancestor module's file (circular modules)"],
        "{tag}"
    );
}

/// 정규 파일이 아닌 모듈 대상(FIFO, `/dev/zero`, 디렉터리)은 읽지 않고 센다 — FIFO는 여는
/// 순간 쓰는 쪽을 영원히 기다리고 `/dev/zero`는 1초에 수 GB를 읽는다. cfg로 막혀 rustc가
/// 읽지 않는 선언도 수확은 조건을 평가하지 않으니 같은 자리에서 걸러야 한다.
#[cfg(unix)]
#[test]
fn non_regular_module_files_are_skipped_and_counted() {
    let lib = "#[cfg(feature = \"never-enabled\")]\n#[path = \"pipe.rs\"]\nmod pipe;\n\
               #[cfg(feature = \"never-enabled\")]\n#[path = \"/dev/zero\"]\nmod zero;\n\
               #[cfg(feature = \"never-enabled\")]\n#[path = \"folder\"]\nmod folder;\n\
               pub fn f() {}\n";
    let dir = temp_crate("special", &owned(&[("lib.rs", lib), ("folder/.keep", "")]));
    let status = std::process::Command::new("mkfifo")
        .arg(dir.join("src/pipe.rs"))
        .status()
        .expect("mkfifo must run");
    assert!(status.success());
    let (modules, limitations) = graph_modules(&dir);
    assert_eq!(modules, ["pathcycle"]);
    assert_eq!(
        skipped_mod_lines(&limitations),
        ["3 mod declarations were skipped because their file is not a regular file (device, FIFO, or directory)"]
    );
}

/// 타깃 루트 파일(cargo metadata가 준 src/lib.rs)이 정규 파일이 아니면 읽지 않고, 그 경로와
/// 원인을 담은 오류(2)로 끝난다. FIFO는 여는 순간 쓰는 쪽을 영원히 기다리고 `/dev/zero`를
/// 가리키는 링크(git이 담을 수 있다)는 끝없이 읽혀, 수확을 거치는 graph·rules·MCP 기동이
/// 멈추거나 메모리를 다 썼다. 회귀해도 테스트가 멈추지 않게 실제 바이너리를 시간 제한 안에서
/// 돌린다.
#[cfg(unix)]
#[test]
fn non_regular_target_roots_are_input_errors() {
    for (tag, link_to_device) in [("rootfifo", false), ("rootzero", true)] {
        let dir = temp_crate(tag, &owned(&[("lib.rs", "pub fn f() {}\n")]));
        let lib = dir.join("src/lib.rs");
        std::fs::remove_file(&lib).unwrap();
        if link_to_device {
            std::os::unix::fs::symlink("/dev/zero", &lib).unwrap();
        } else {
            let made = std::process::Command::new("mkfifo")
                .arg(&lib)
                .status()
                .expect("mkfifo must run");
            assert!(made.success());
        }
        let (code, err) = run_with_deadline(&["graph", "--dir", dir.to_str().unwrap()], 10);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(code, Some(2), "{tag}: {err}");
        // 링크를 따라간 `/dev/zero`가 아니라 타깃 경로를 말해야 어느 타깃인지 안다.
        assert!(
            err.contains("src/lib.rs: not a regular file"),
            "{tag}: {err}"
        );
    }
}

/// 실제 바이너리를 돌려 (종료 코드, stderr)를 돌려준다 — `seconds` 안에 끝나지 않으면 죽이고
/// 실패한다(멈추는 회귀가 테스트 전체를 멈추지 않게).
#[cfg(unix)]
fn run_with_deadline(args: &[&str], seconds: u64) -> (Option<i32>, String) {
    use std::io::Read;
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_rustograph"))
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("rustograph binary must start");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    loop {
        match child.try_wait().expect("polling the child must work") {
            Some(status) => {
                let mut err = String::new();
                child
                    .stderr
                    .take()
                    .expect("stderr is piped")
                    .read_to_string(&mut err)
                    .expect("stderr must be UTF-8");
                return (status.code(), err);
            }
            None if std::time::Instant::now() >= deadline => {
                child.kill().expect("killing the stuck child must work");
                child.wait().expect("reaping the killed child must work");
                panic!("rustograph {args:?} did not finish within {seconds}s");
            }
            None => std::thread::sleep(std::time::Duration::from_millis(20)),
        }
    }
}

/// 순환이 아니어도 모듈 중첩은 `MAX_MODULE_DEPTH`에서 멈추고 넘는 선언을 센다 — 조상
/// 파일 검사를 비껴가는 입력에도 트리 확장이 끝나게 하는 보조 상한이다. 서로 다른 정규
/// 파일 600개가 차례로 다음 파일을 싣는다.
#[test]
fn module_nesting_beyond_the_depth_cap_is_counted() {
    let cap = rustograph::modtree::MAX_MODULE_DEPTH;
    let files: Vec<(String, String)> = (0..cap + 88)
        .map(|i| {
            let name = if i == 0 {
                "lib.rs".to_string()
            } else {
                format!("f{i}.rs")
            };
            let next = i + 1;
            (
                name,
                format!("#[path = \"f{next}.rs\"]\npub mod m;\npub fn g{i}() {{}}\n"),
            )
        })
        .collect();
    let (modules, limitations) = graph_modules(&temp_crate("deep", &files));
    assert_eq!(modules.len(), cap);
    let deepest = format!("pathcycle{}", "::m".repeat(cap - 1));
    assert!(modules.contains(&deepest));
    assert_eq!(
        skipped_mod_lines(&limitations),
        [
            format!("1 mod declarations were skipped because module nesting exceeds {cap} levels")
                .as_str()
        ]
    );
}

/// 같은 파일을 서로 다른(조상이 아닌) 모듈이 싣는 `#[path]`와 인라인·디렉터리 `#[path]`는
/// 그대로 정점이다 — 조상 파일만 순환이다. 셀 것이 없으니 limitation도 조용하다.
#[test]
fn path_attribute_fixture_modules_are_kept() {
    let d = doc(&Default::default());
    let ids = d.vertex_ids();
    for id in [
        "fixture_core::gen",
        "fixture_core::shared",
        "fixture_core::nest::inner::leaf",
        "fixture_core::outer_a::duplex",
        "fixture_core::outer_b::inner",
        "fixture_core::single::inner",
        "fixture_core::single::pin::pin_leaf",
        "fixture_core::single::pin::pin_plain",
        "fixture_core::single::pmod::inline::leaf",
    ] {
        assert!(ids.contains(id), "missing module {id}");
    }
    assert_eq!(skipped_mod_lines(&d.limitations), Vec::<&str>::new());
}
