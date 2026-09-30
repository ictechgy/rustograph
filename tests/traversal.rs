//! `reach`·`impact --format language-traversal` CLI 계약 — 실제 fixture 워크스페이스로
//! 문서 필드·종료 코드(0/64/2)·root 입력 규칙·revision을 검증한다.

use rustograph::cli;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixture")
}

fn schema_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixture-schema")
}

/// (종료 코드, 표준 출력, 표준 오류).
fn run(args: &[&str]) -> (i32, String, String) {
    let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = cli::run(&argv, &mut out, &mut err);
    (
        code,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

fn json(text: &str) -> serde_json::Value {
    serde_json::from_str(text).expect("traversal output must be JSON")
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rustograph-trav-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn reach_writes_dependencies_document() {
    let dir = fixture().display().to_string();
    let (code, out, err) = run(&[
        "reach",
        "--dir",
        &dir,
        "--generated-at",
        "2026-09-30T00:00:00Z",
        "fixture_app::main",
    ]);
    assert_eq!(code, 0, "stderr: {err}");
    let d = json(&out);
    assert_eq!(d["format"], "language-traversal");
    assert_eq!(d["version"], 1);
    assert_eq!(d["tool"]["name"], "rustograph");
    assert_eq!(d["platform"], "rust");
    assert_eq!(d["direction"], "dependencies");
    assert_eq!(d["generatedAt"], "2026-09-30T00:00:00Z");
    // project는 schema 문서의 project와 같은 realpath다 — isthmus가 문자열로 비교한다.
    assert_eq!(
        d["project"],
        fixture().canonicalize().unwrap().display().to_string()
    );
    let rev = d["graphRevision"].as_str().unwrap();
    assert_eq!(rev.len(), 64);
    assert!(rev
        .bytes()
        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
    assert_eq!(d["roots"][0]["id"], "fixture_app::main");
    assert_eq!(d["roots"][0]["symbol"]["usr"], "fixture_app::main");
    let reached = d["reached"].as_array().unwrap();
    let entry = reached
        .iter()
        .find(|r| r["symbol"]["usr"] == "fixture_core::entry")
        .expect("main reaches fixture_core::entry");
    assert_eq!(entry["via"], "fixture_app::main");
    assert_eq!(entry["depth"], 1);
    assert_eq!(entry["roots"], serde_json::json!([0]));
    assert_eq!(entry["evidence"], "direct");
    assert!(entry["relationships"]
        .as_array()
        .unwrap()
        .contains(&"call".into()));
    // 위치는 project 상대 경로다.
    assert_eq!(
        entry["symbol"]["location"]["path"],
        "fixture_core/src/lib.rs"
    );
    // 모든 정점이 등급을 싣는다 — dispatch·unresolvedCalls는 완전성을 주장하지 않아 없다.
    assert!(reached.iter().all(|r| r["evidence"].is_string()));
    assert!(d.get("dispatch").is_none());
    assert!(reached.iter().all(|r| r.get("unresolvedCalls").is_none()));
    assert_eq!(d["truncated"], false);
    assert!(d.get("truncationReasons").is_none());
}

#[test]
fn fixed_generated_at_is_byte_identical_and_graph_revision_ignores_dir_spelling() {
    let dir = fixture().display().to_string();
    let dotted = format!("{dir}/.");
    let args = |d: &str| {
        run(&[
            "impact",
            "--format",
            "language-traversal",
            "--dir",
            d,
            "--generated-at",
            "2026-09-30T00:00:00Z",
            "fixture_core::Used",
            "fixture_core::entry",
        ])
    };
    let (c1, a, _) = args(&dir);
    let (c2, b, _) = args(&dir);
    let (c3, c, _) = args(&dotted);
    assert_eq!((c1, c2, c3), (0, 0, 0));
    assert_eq!(a, b);
    assert_eq!(json(&a)["graphRevision"], json(&c)["graphRevision"]);
    assert_eq!(json(&a)["direction"], "dependents");
}

#[test]
fn root_not_found_writes_document_and_exits_64() {
    let dir = fixture().display().to_string();
    let (code, out, err) = run(&[
        "impact",
        "--format",
        "language-traversal",
        "--dir",
        &dir,
        "no_such::root",
        "fixture_core::Used",
    ]);
    assert_eq!(code, 64);
    assert!(err.contains("root-not-found"));
    let d = json(&out);
    assert_eq!(d["roots"][0]["id"], "no_such::root");
    assert!(d["roots"][0].get("symbol").is_none());
    assert_eq!(d["roots"][1]["symbol"]["usr"], "fixture_core::Used");
    assert_eq!(d["truncated"], true);
    assert!(d["truncationReasons"]
        .as_array()
        .unwrap()
        .contains(&"root-not-found".into()));
    assert!(d["limitations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|l| l.as_str().unwrap().starts_with("root-not-found:")));
    // 도달 정점의 root 인덱스는 요청 순서 기준이다 — 찾은 root는 1번이다.
    let reached = d["reached"].as_array().unwrap();
    assert!(!reached.is_empty());
    assert!(reached.iter().all(|r| r["roots"] == serde_json::json!([1])));
}

#[test]
fn usage_errors_exit_64_with_empty_stdout() {
    let dir = fixture().display().to_string();
    let missing_file = temp_dir("missing").join("absent.json");
    let bad_json = temp_dir("badjson").join("roots.json");
    std::fs::write(&bad_json, "{\"format\":\"other\"}").unwrap();
    let cases: Vec<Vec<String>> = vec![
        vec!["reach".into()],
        vec!["reach".into(), "a\u{7}b".into()],
        vec!["reach".into(), "a\u{2028}".into()],
        vec!["reach".into(), "".into()],
        vec!["reach".into(), "x".into(), "--max-depth".into(), "0".into()],
        vec![
            "reach".into(),
            "x".into(),
            "--max-depth".into(),
            "129".into(),
        ],
        vec![
            "reach".into(),
            "x".into(),
            "--max-reached".into(),
            "0".into(),
        ],
        vec![
            "reach".into(),
            "x".into(),
            "--depth".into(),
            "3".into(),
            "--max-depth".into(),
            "3".into(),
        ],
        vec![
            "reach".into(),
            "x".into(),
            "--revision".into(),
            "r\u{1}".into(),
        ],
        vec![
            "reach".into(),
            "x".into(),
            "--revision".into(),
            "r".repeat(257),
        ],
        vec![
            "reach".into(),
            "x".into(),
            "--generated-at".into(),
            "yesterday".into(),
        ],
        vec!["reach".into(), "x".into(), "--format".into(), "json".into()],
        vec!["reach".into(), "x".into(), "--max".into(), "3".into()],
        vec![
            "reach".into(),
            "x".into(),
            "--max-depth".into(),
            "2".into(),
            "--max-depth".into(),
            "3".into(),
        ],
        vec!["reach".into(), "x".into(), "--bogus".into()],
        vec![
            "reach".into(),
            "--roots-from".into(),
            missing_file.display().to_string(),
        ],
        vec![
            "reach".into(),
            "--roots-from".into(),
            bad_json.display().to_string(),
        ],
        vec![
            "impact".into(),
            "--format".into(),
            "language-traversal".into(),
            "--bogus".into(),
        ],
        vec![
            "impact".into(),
            "--format".into(),
            "language-traversal".into(),
        ],
    ];
    for mut argv in cases {
        argv.extend(["--dir".to_string(), dir.clone()]);
        let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
        let (code, out, err) = run(&refs);
        assert_eq!(code, 64, "{argv:?} stderr: {err}");
        assert!(out.is_empty(), "{argv:?} must not write a document");
        assert!(err.starts_with("error:"), "{argv:?}");
    }
    // 10,000개를 넘는 root도 사용법 오류다.
    let many = temp_dir("many").join("roots.json");
    let ids: Vec<String> = (0..10_001).map(|i| format!("x::f{i}")).collect();
    std::fs::write(&many, serde_json::to_string(&ids).unwrap()).unwrap();
    let (code, out, _) = run(&[
        "reach",
        "--dir",
        &dir,
        "--roots-from",
        &many.display().to_string(),
    ]);
    assert_eq!((code, out.is_empty()), (64, true));
    // 수확 실패는 사용법이 아니라 분석 오류(2)다.
    let (code, out, _) = run(&["reach", "--dir", "/nonexistent-rustograph-dir", "x"]);
    assert_eq!((code, out.is_empty()), (2, true));
}

#[test]
fn every_schema_usr_is_accepted_as_a_root() {
    // schema 사실의 usr는 순회의 정점 id와 같은 문자열이어야 한다 — bridge-facts 문서를
    // --roots-from으로 그대로 넘겨 root-not-found가 하나도 없어야 한다.
    let schema_dir = schema_fixture().display().to_string();
    let (code, facts, err) = run(&["schema", "--dir", &schema_dir]);
    assert_eq!(code, 0, "{err}");
    let facts_path = temp_dir("facts").join("schema.json");
    std::fs::write(&facts_path, &facts).unwrap();
    let (code, out, err) = run(&[
        "impact",
        "--format",
        "language-traversal",
        "--dir",
        &schema_dir,
        "--roots-from",
        &facts_path.display().to_string(),
    ]);
    assert_eq!(code, 0, "stderr: {err}");
    let d = json(&out);
    let usrs: std::collections::BTreeSet<String> = json(&facts)["facts"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["symbol"]["usr"].as_str().map(str::to_string))
        .collect();
    let roots = d["roots"].as_array().unwrap();
    assert_eq!(roots.len(), usrs.len(), "roots are deduplicated usrs");
    assert!(roots.iter().all(|r| r["symbol"]["usr"] == r["id"]));
    // schema 문서와 순회 문서의 project가 같아야 trace가 둘을 한 project로 읽는다.
    assert_eq!(d["project"], json(&facts)["project"]);
}

#[test]
fn separator_and_stdin_roots() {
    let bin = env!("CARGO_BIN_EXE_rustograph");
    let dir = fixture().display().to_string();
    let mut child = Command::new(bin)
        .args([
            "reach",
            "--dir",
            &dir,
            "--roots-from",
            "-",
            "--",
            "fixture_app::main",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(br#"["fixture_core::entry", "fixture_app::main"]"#)
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    let d = json(&String::from_utf8(out.stdout).unwrap());
    let ids: Vec<&str> = d["roots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    // 위치 인자가 먼저, 중복은 처음 나온 자리에만.
    assert_eq!(ids, vec!["fixture_app::main", "fixture_core::entry"]);
}

/// git 저장소 안의 최소 워크스페이스를 만든다.
fn git_workspace(tag: &str) -> PathBuf {
    let dir = temp_dir(tag);
    std::fs::create_dir_all(dir.join("app/src")).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        "[workspace]\nmembers = [\"app\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("app/src/lib.rs"),
        "pub fn a() { b() }\npub fn b() {}\n",
    )
    .unwrap();
    std::fs::write(dir.join(".gitignore"), "target/\nCargo.lock\n").unwrap();
    dir
}

fn git(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[test]
fn revision_is_head_only_on_a_clean_tree() {
    let dir = git_workspace("git");
    if !git(&dir, &["init", "-q"]) {
        eprintln!("git unavailable — skipping revision test");
        return;
    }
    assert!(git(&dir, &["add", "-A"]) && git(&dir, &["commit", "-q", "-m", "init"]));
    let head = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&dir)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let d = dir.display().to_string();
    let (code, out, err) = run(&["reach", "--dir", &d, "app::a"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(json(&out)["revision"], head.trim());
    // 추적하지 않는 파일이 생기면 HEAD는 분석한 소스가 아니다.
    std::fs::write(dir.join("app/src/extra.rs"), "pub fn c() {}\n").unwrap();
    let (_, out, _) = run(&["reach", "--dir", &d, "app::a"]);
    assert!(json(&out).get("revision").is_none());
    // 명시 --revision은 그대로 싣는다.
    let (_, out, _) = run(&["reach", "--dir", &d, "--revision", "rev-7", "app::a"]);
    assert_eq!(json(&out)["revision"], "rev-7");
    let _ = std::fs::remove_dir_all(&dir);
}
