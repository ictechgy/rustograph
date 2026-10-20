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
fn graph_revision_is_the_sha256_of_the_project_rooted_symbol_graph() {
    use rustograph::traversal::sha256::hex_digest;
    // graphRevision은 root를 project 정규 경로로 둔 심볼 그래프 JSON — `graph --level symbol`이
    // 그 경로로 내는 바이트 — 의 SHA-256이다. 해시를 흘려 계산해도 같은 바이트여야 한다.
    for dir in [fixture(), schema_fixture()] {
        let canon = dir.canonicalize().unwrap().display().to_string();
        let (code, graph, err) = run(&["graph", "--level", "symbol", "--dir", &canon]);
        assert_eq!(code, 0, "{err}");
        let root = json(&graph)["vertices"][0]["id"]
            .as_str()
            .unwrap()
            .to_string();
        let (code, out, err) = run(&["reach", "--dir", &canon, &root]);
        assert_eq!(code, 0, "{err}");
        assert_eq!(json(&out)["graphRevision"], hex_digest(graph.as_bytes()));
    }
    // 저장 문서(--graph)도 같다 — workspace·limitations가 없는 문서와 있는 문서 모두.
    let project = temp_dir("revision").canonicalize().unwrap();
    let canon = project.display().to_string();
    for (workspace, limitations) in [(None, vec![]), (Some(canon.clone()), vec!["x: 1".into()])] {
        let doc = saved_document(&canon, workspace, limitations);
        let text = rustograph::export::to_json(&doc);
        let file = project.join("graph.json");
        std::fs::write(&file, &text).unwrap();
        let path = file.display().to_string();
        let (code, out, err) = run(&["reach", "--graph", &path, "--dir", &canon, "c::a"]);
        assert_eq!(code, 0, "{err}");
        assert_eq!(json(&out)["graphRevision"], hex_digest(text.as_bytes()));
    }
    // 스냅샷이 기록한 revision은 해시하지 않는다 — 같은 커밋에서 새로 수확한 그래프(revision 없음)와
    // 같은 값이라야 isthmus가 같은 플랫폼 분석끼리 stale로 보지 않는다. 기록은 revision으로 간다.
    let text = rustograph::export::to_json(&saved_document(&canon, None, vec![]));
    let mut recorded = json(&text);
    recorded["revision"] = "0123456789abcdef0123456789abcdef01234567".into();
    let file = project.join("recorded.json");
    std::fs::write(&file, recorded.to_string()).unwrap();
    let path = file.display().to_string();
    let (code, out, err) = run(&["reach", "--graph", &path, "--dir", &canon, "c::a"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(json(&out)["revision"], recorded["revision"]);
    assert_eq!(json(&out)["graphRevision"], hex_digest(text.as_bytes()));
}

/// root가 `root`인 두 정점 a→b 심볼 문서.
fn saved_document(
    root: &str,
    workspace: Option<String>,
    limitations: Vec<String>,
) -> rustograph::graph::Document {
    use rustograph::graph::{document, Edge, EdgeKind, Kind, Level, Vertex};
    let vertex = |id: &str| Vertex {
        id: id.to_string(),
        kind: Kind::Fn,
        krate: "c".into(),
        module: "c".into(),
        position: None,
        exported: false,
        generated: false,
        cfg: None,
        unsafe_: false,
    };
    document(
        Level::Symbol,
        root.to_string(),
        workspace,
        vec![],
        vec![vertex("c::a"), vertex("c::b")],
        vec![Edge::new("c::a".into(), "c::b".into(), EdgeKind::Call)],
        limitations,
    )
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
        // `=` 표기도 순회 형식 요청이다 — 파싱 실패는 2가 아니라 64다.
        vec![
            "impact".into(),
            "--format=language-traversal".into(),
            "--bogus".into(),
            "x".into(),
        ],
        vec![
            "reach".into(),
            "x".into(),
            "--generated-at".into(),
            "2026-09-30T00:00:00+99:99".into(),
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

/// fixture 저장소를 준비하는 git — 분석이 아니라 준비 단계다. 사용자·시스템 설정과 훅을 꺼서
/// 그 설정이 fixture를 바꾸지 않게 하고, 로컬 경로 서브모듈을 받으려고 file 프로토콜을 연다.
fn fixture_git(dir: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "protocol.file.allow=always",
        ])
        .current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    command
}

fn git(dir: &Path, args: &[&str]) -> bool {
    fixture_git(dir)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// fixture git의 표준 출력(앞뒤 공백 제거) — 실패하면 테스트가 멈춘다.
fn git_stdout(dir: &Path, args: &[&str]) -> String {
    let out = fixture_git(dir).args(args).output().unwrap();
    assert!(out.status.success(), "fixture git {args:?} failed");
    String::from_utf8(out.stdout).unwrap().trim().to_string()
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

// ── revision 판독의 git 격리(DECISIONS D1) ─────────────────────────────────
// 분석 대상 저장소의 git 설정에 명령(filter 드라이버·fsmonitor 훅·index 훅)을 심고, 그 명령이 남기는
// 마커로 순회 명령이 그것을 실행하지 않는지 본다. 명령은 셸 스크립트라 unix에서만 돈다.

/// git이 파일을 다시 읽게 만드는 먼 과거 시각(2001-01-01) — 같은 초 안의 쓰기는 racy 검사 때문에
/// 결정적이지 않아 시각을 크게 옮긴다.
#[cfg(unix)]
const LONG_AGO: u64 = 978_307_200;
/// `LONG_AGO`와 다른 과거 시각(2000-01-01) — 이미 과거로 둔 파일의 stat을 다시 바꿀 때 쓴다.
#[cfg(unix)]
const LONGER_AGO: u64 = 946_684_800;
/// `git_workspace`가 만드는 추적 파일.
#[cfg(unix)]
const TRACKED: [&str; 4] = [
    "Cargo.toml",
    "app/Cargo.toml",
    "app/src/lib.rs",
    ".gitignore",
];

/// 분석 대상 밖의 흔적 디렉터리 — 저장소 설정에 심은 명령이 실행되면 여기에 마커를 남긴다.
/// 작업 트리 안에 두면 추적하지 않는 파일이 생겨 revision 판정이 달라진다.
#[cfg(unix)]
struct MarkerKit {
    dir: PathBuf,
}

#[cfg(unix)]
impl MarkerKit {
    fn new(tag: &str) -> MarkerKit {
        MarkerKit {
            dir: temp_dir(&format!("{tag}-kit")),
        }
    }

    /// 실행되면 마커 `name`을 남기고 `then`(셸 문장)을 잇는 스크립트의 경로 — git 설정 값에서
    /// `;`·`#`는 주석이라 명령 대신 스크립트 경로를 설정에 넣는다.
    fn script(&self, name: &str, then: &str) -> String {
        use std::os::unix::fs::PermissionsExt;
        let path = self.dir.join(format!("{name}.sh"));
        let marker = self.dir.join(format!("MARKER-{name}"));
        let body = format!("#!/bin/sh\ntouch '{}'\n{then}\n", marker.display());
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.display().to_string()
    }

    /// 지금까지 실행된 명령의 마커 이름(정렬).
    fn fired(&self) -> Vec<String> {
        let mut fired: Vec<String> = std::fs::read_dir(&self.dir)
            .unwrap()
            .filter_map(|e| e.ok()?.file_name().into_string().ok())
            .filter_map(|n| n.strip_prefix("MARKER-").map(str::to_string))
            .collect();
        fired.sort();
        fired
    }

    /// 준비 단계의 git이 남긴 마커를 지운다.
    fn reset(&self) {
        for name in self.fired() {
            std::fs::remove_file(self.dir.join(format!("MARKER-{name}"))).unwrap();
        }
    }
}

/// `process` filter 마커 뒤에 이을 handshake 응답 — 기능 없이 답하고 입력을 비운다. 답하지 않으면
/// git이 128로 죽어 뒤 파일의 마커를 가린다.
#[cfg(unix)]
const PROCESS_HANDSHAKE: &str =
    "printf '0016git-filter-server\\n000eversion=2\\n0000'\nprintf '0000'\ncat >/dev/null";

/// dir을 git 저장소로 만들고 전부 커밋한다 — git이 없으면 None(테스트를 건너뛴다).
#[cfg(unix)]
fn commit_all(dir: PathBuf) -> Option<PathBuf> {
    if !git(&dir, &["init", "-q"]) {
        eprintln!("git unavailable — skipping git isolation test");
        return None;
    }
    assert!(git(&dir, &["add", "-A"]) && git(&dir, &["commit", "-q", "-m", "init"]));
    Some(dir)
}

/// 이미 막혀 있는 실행 경로도 함께 심는다 — fsmonitor 훅과 index를 쓸 때 도는 `post-index-change`
/// 훅. 격리가 풀리면 이 마커가 남는다.
#[cfg(unix)]
fn plant_hooks(repo: &Path, kit: &MarkerKit) {
    let fsmonitor = kit.script("fsmonitor", "exit 1");
    assert!(git(repo, &["config", "core.fsmonitor", &fsmonitor]));
    let hooks = repo.join(".git/hooks");
    std::fs::create_dir_all(&hooks).unwrap();
    std::fs::copy(
        kit.script("post-index-change", "exit 0"),
        hooks.join("post-index-change"),
    )
    .unwrap();
}

/// `.git/info/attributes`에 한 줄을 쓴다(바이트 그대로).
#[cfg(unix)]
fn write_info_attributes(repo: &Path, line: &[u8]) {
    std::fs::create_dir_all(repo.join(".git/info")).unwrap();
    std::fs::write(repo.join(".git/info/attributes"), line).unwrap();
}

/// 파일 시각을 과거로 돌려 git의 stat 캐시와 어긋나게 한다 — git이 파일을 다시 읽어 filter를 거쳐 비교한다.
#[cfg(unix)]
fn backdate(repo: &Path, files: &[&str], secs: u64) {
    for file in files {
        let handle = std::fs::OpenOptions::new()
            .write(true)
            .open(repo.join(file))
            .unwrap();
        handle
            .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs))
            .unwrap();
    }
}

/// 실제 바이너리로 순회 명령을 실행한다 — 사용자·시스템 git 설정을 끄고(fixture 저장소의 설정만
/// 남는다) 분석 대상 밖 `cwd`에서 띄운다. (종료 코드, 문서).
#[cfg(unix)]
fn traverse(cwd: &Path, args: &[&str]) -> (Option<i32>, serde_json::Value) {
    traverse_with_path(cwd, None, args)
}

/// `traverse`와 같되 `path`가 있으면 그 값을 바이너리의 PATH로 준다.
#[cfg(unix)]
fn traverse_with_path(
    cwd: &Path,
    path: Option<&std::ffi::OsStr>,
    args: &[&str],
) -> (Option<i32>, serde_json::Value) {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rustograph"));
    command
        .args(args)
        .current_dir(cwd)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1");
    if let Some(path) = path {
        command.env("PATH", path);
    }
    let out = command.output().unwrap();
    (
        out.status.code(),
        json(&String::from_utf8(out.stdout).unwrap()),
    )
}

/// 문서의 `revision-unverified:` limitation.
#[cfg(unix)]
fn unverified(d: &serde_json::Value) -> Vec<String> {
    labeled_limitations(d, "revision-unverified:")
}

/// 문서의 limitation 가운데 `label`로 시작하는 것.
fn labeled_limitations(d: &serde_json::Value, label: &str) -> Vec<String> {
    d["limitations"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|l| l.as_str())
        .filter(|l| l.starts_with(label))
        .map(str::to_string)
        .collect()
}

/// 항등 filter(내용을 바꾸지 않는다)를 심은 저장소에서 순회 명령이 그 명령을 실행하지 않고, 원문으로
/// 비교해도 같은 내용이라 깨끗한 트리의 HEAD를 싣는지 본다.
#[cfg(unix)]
fn assert_identity_filter_is_not_run(kit: &MarkerKit, repo: &Path, head: &str, command: &[&str]) {
    plant_hooks(repo, kit);
    backdate(repo, &TRACKED, LONG_AGO);
    let dir = repo.display().to_string();
    let mut args = command.to_vec();
    args.extend(["--dir", &dir]);
    let (code, d) = traverse(&kit.dir, &args);
    assert_eq!(code, Some(0), "{d}");
    assert_eq!(kit.fired(), Vec::<String>::new(), "repository commands ran");
    assert_eq!(d["revision"], head, "{d}");
    assert!(unverified(&d).is_empty(), "{d}");
}

#[cfg(unix)]
#[test]
fn reach_runs_no_filter_driver_named_in_info_attributes() {
    let kit = MarkerKit::new("d1-info");
    let Some(repo) = commit_all(git_workspace("d1-info")) else {
        return;
    };
    let head = git_stdout(&repo, &["rev-parse", "HEAD"]);
    let clean = kit.script("clean", "cat");
    assert!(git(&repo, &["config", "filter.evil.clean", &clean]));
    write_info_attributes(&repo, b"* filter=evil\n");
    assert_identity_filter_is_not_run(&kit, &repo, &head, &["reach", "app::a"]);
}

#[cfg(unix)]
#[test]
fn impact_traversal_runs_no_dotted_filter_driver_named_in_gitattributes() {
    let kit = MarkerKit::new("d1-tree");
    let dir = git_workspace("d1-tree");
    // 이름에 점이 든 드라이버 — 이름은 `filter.`과 마지막 `.clean` 사이 전체다.
    std::fs::write(dir.join(".gitattributes"), "* filter=evil.name\n").unwrap();
    let Some(repo) = commit_all(dir) else {
        return;
    };
    let head = git_stdout(&repo, &["rev-parse", "HEAD"]);
    let clean = kit.script("clean", "cat");
    assert!(git(&repo, &["config", "filter.evil.name.clean", &clean]));
    backdate(&repo, &[".gitattributes"], LONG_AGO);
    assert_identity_filter_is_not_run(
        &kit,
        &repo,
        &head,
        &["impact", "--format", "language-traversal", "app::b"],
    );
}

#[cfg(unix)]
#[test]
fn reach_runs_no_process_filter_driver() {
    let kit = MarkerKit::new("d1-process");
    let Some(repo) = commit_all(git_workspace("d1-process")) else {
        return;
    };
    let head = git_stdout(&repo, &["rev-parse", "HEAD"]);
    let process = kit.script("process", PROCESS_HANDSHAKE);
    assert!(git(&repo, &["config", "filter.long.process", &process]));
    write_info_attributes(&repo, b"* filter=long\n");
    assert_identity_filter_is_not_run(&kit, &repo, &head, &["reach", "app::a"]);
}

#[cfg(unix)]
#[test]
fn a_filter_driver_name_with_equals_stops_revision_reading() {
    // git의 -c는 첫 `=`에서 키와 값을 나눠 `a=b` 드라이버를 덮지 못한다.
    assert_unoverridable_driver_is_not_run("d1-equals", b"a=b");
}

#[cfg(unix)]
#[test]
fn a_filter_driver_name_in_invalid_utf8_stops_revision_reading() {
    // 깨진 UTF-8 이름은 인자로 옮길 수 없다(windows는 인자를 UTF-16으로 바꾼다).
    assert_unoverridable_driver_is_not_run("d1-utf8", b"\xff\xfe");
}

/// 덮을 수 없는 이름의 드라이버를 심은 저장소 — git을 실행하지 않고 revision을 생략해 이유를 알린다.
#[cfg(unix)]
fn assert_unoverridable_driver_is_not_run(tag: &str, name: &[u8]) {
    let kit = MarkerKit::new(tag);
    let Some(repo) = commit_all(git_workspace(tag)) else {
        return;
    };
    let clean = kit.script("clean", "cat");
    let mut config = std::fs::read(repo.join(".git/config")).unwrap();
    config.extend_from_slice(b"[filter \"");
    config.extend_from_slice(name);
    config.extend_from_slice(format!("\"]\n\tclean = {clean}\n").as_bytes());
    std::fs::write(repo.join(".git/config"), config).unwrap();
    write_info_attributes(&repo, &[b"* filter=", name, b"\n"].concat());
    backdate(&repo, &TRACKED, LONG_AGO);
    let dir = repo.display().to_string();
    let (code, d) = traverse(&kit.dir, &["reach", "--dir", &dir, "app::a"]);
    assert_eq!(code, Some(0), "{tag}: {d}");
    assert_eq!(kit.fired(), Vec::<String>::new(), "{tag}: driver ran");
    assert!(d.get("revision").is_none(), "{tag}: {d}");
    assert_eq!(unverified(&d).len(), 1, "{tag}: {d}");
}

#[cfg(unix)]
#[test]
fn revision_reading_runs_no_git_inside_submodules() {
    let kit = MarkerKit::new("d1-sub");
    let upstream = temp_dir("d1-sub-upstream");
    std::fs::write(upstream.join("notes.txt"), "notes\n").unwrap();
    let Some(upstream) = commit_all(upstream) else {
        return;
    };
    let repo = commit_all(git_workspace("d1-sub")).unwrap();
    let source = upstream.display().to_string();
    assert!(git(&repo, &["submodule", "add", "-q", &source, "vendored"]));
    assert!(git(&repo, &["commit", "-q", "-m", "vendored"]));
    // 서브모듈 저장소 자신의 설정 — 상위 저장소의 설정 조회에는 보이지 않는다.
    let gitdir = PathBuf::from(git_stdout(
        &repo.join("vendored"),
        &["rev-parse", "--absolute-git-dir"],
    ));
    let config = gitdir.join("config").display().to_string();
    let clean = kit.script("submodule-clean", "cat");
    assert!(git(
        &repo,
        &["config", "--file", &config, "filter.evil.clean", &clean]
    ));
    std::fs::create_dir_all(gitdir.join("info")).unwrap();
    std::fs::write(gitdir.join("info/attributes"), "* filter=evil\n").unwrap();
    backdate(&repo, &["vendored/notes.txt"], LONG_AGO);
    let dir = repo.display().to_string();
    let (code, d) = traverse(&kit.dir, &["reach", "--dir", &dir, "app::a"]);
    assert_eq!(code, Some(0), "{d}");
    assert_eq!(kit.fired(), Vec::<String>::new(), "submodule driver ran");
    // 서브모듈 작업 트리를 보지 않고는 깨끗함을 증명할 수 없다 — revision을 싣지 않고 이유를 알린다.
    assert!(d.get("revision").is_none(), "{d}");
    let notes = unverified(&d);
    assert_eq!(notes.len(), 1, "{d}");
    assert!(notes[0].contains("1 git submodule"), "{notes:?}");
}

#[cfg(unix)]
#[test]
fn a_file_its_filter_normalizes_makes_the_tree_dirty_once_git_rereads_it() {
    let kit = MarkerKit::new("d1-lfs");
    let dir = git_workspace("d1-lfs");
    if !git(&dir, &["init", "-q"]) {
        eprintln!("git unavailable — skipping git isolation test");
        return;
    }
    // git-lfs처럼 저장소에는 포인터를, 작업 트리에는 원문을 두는 filter.
    let pointer = kit.script("pointer", "cat >/dev/null\nprintf 'pointer\\n'");
    assert!(git(&dir, &["config", "filter.pointer.clean", &pointer]));
    std::fs::write(dir.join(".gitattributes"), "*.bin filter=pointer\n").unwrap();
    std::fs::write(dir.join("app/data.bin"), "raw content\n").unwrap();
    // 커밋 전에 시각을 과거로 둔다 — index가 그 stat을 기록해 racy 재검사 없이 캐시를 믿는다.
    backdate(&dir, &TRACKED, LONG_AGO);
    backdate(&dir, &[".gitattributes", "app/data.bin"], LONG_AGO);
    assert!(git(&dir, &["add", "-A"]) && git(&dir, &["commit", "-q", "-m", "init"]));
    let head = git_stdout(&dir, &["rev-parse", "HEAD"]);
    kit.reset();
    let path = dir.display().to_string();
    let args = ["reach", "--dir", path.as_str(), "app::a"];
    // stat 캐시가 맞으면 git은 파일을 다시 읽지 않는다 — 깨끗한 트리의 HEAD다.
    let (_, d) = traverse(&kit.dir, &args);
    assert_eq!(d["revision"], head, "{d}");
    // stat이 어긋나면 git이 원문을 filter 없이 읽어 포인터와 다르다고 본다 — 확인하지 못한 깨끗함을
    // 주장하지 않는다(fail-safe). 더러운 트리와 같아 limitation은 없다.
    backdate(&dir, &["app/data.bin"], LONGER_AGO);
    let (_, d) = traverse(&kit.dir, &args);
    assert!(d.get("revision").is_none(), "{d}");
    assert!(unverified(&d).is_empty(), "{d}");
    assert_eq!(kit.fired(), Vec::<String>::new(), "the filter ran");
}

#[cfg(unix)]
#[test]
fn git_is_never_resolved_from_relative_path_entries() {
    // revision 판독의 git은 `--dir`을 작업 디렉터리로 띄운다 — PATH의 빈 항목·`.`·`bin` 같은 상대
    // 항목이 그 디렉터리 기준으로 풀리면 분석 대상이 심은 `git`이 실행된다.
    let kit = MarkerKit::new("d1-path");
    let dir = git_workspace("d1-path");
    std::fs::write(dir.join(".gitignore"), "target/\nCargo.lock\n/git\n/bin/\n").unwrap();
    let Some(repo) = commit_all(dir) else {
        return;
    };
    let head = git_stdout(&repo, &["rev-parse", "HEAD"]);
    // 준비 단계의 git이 집지 않게 커밋 뒤에 심는다(무시 목록에 있어 트리는 깨끗하다).
    let planted = kit.script("planted-git", "exit 1");
    std::fs::create_dir_all(repo.join("bin")).unwrap();
    std::fs::copy(&planted, repo.join("git")).unwrap();
    std::fs::copy(&planted, repo.join("bin/git")).unwrap();
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let dir = repo.display().to_string();
    let args = ["reach", "--dir", dir.as_str(), "app::a"];
    // 체크아웃 밖에서 띄운 실행과, 흔한 경우인 체크아웃 안에서 띄운 실행 모두.
    for cwd in [&kit.dir, &repo] {
        for entry in ["", ".", "bin"] {
            let mut path = std::ffi::OsString::from(entry);
            path.push(":");
            path.push(&inherited);
            let (code, d) = traverse_with_path(cwd, Some(&path), &args);
            assert_eq!(code, Some(0), "{d}");
            let fired = kit.fired();
            assert!(
                fired.is_empty(),
                "cwd {cwd:?}, PATH entry {entry:?}: {fired:?}"
            );
            // 절대 경로 항목에서 찾은 git이 돈다 — 깨끗한 트리의 HEAD다.
            assert_eq!(
                d["revision"], head,
                "cwd {cwd:?}, PATH entry {entry:?}: {d}"
            );
        }
    }
}

// ── 저장 스냅샷의 revision(arch#1) ─────────────────────────────────────────
// `--graph`로 읽은 그래프의 소스는 지금의 작업 트리가 아니라 저장할 때의 트리다. 순회 명령은 git HEAD를
// 보지 않고 스냅샷이 기록한 revision(`graph --out`이 수확 시점의 깨끗한 HEAD를 적는다)을 쓴다.

/// 저장한 JSON 파일.
fn read_json(path: &str) -> serde_json::Value {
    json(&std::fs::read_to_string(path).unwrap())
}

#[test]
fn snapshot_traversal_reports_the_revision_the_snapshot_records() {
    let dir = git_workspace("snap-rev");
    if !git(&dir, &["init", "-q"]) {
        eprintln!("git unavailable — skipping snapshot revision test");
        return;
    }
    assert!(git(&dir, &["add", "-A"]) && git(&dir, &["commit", "-q", "-m", "c1"]));
    let c1 = git_stdout(&dir, &["rev-parse", "HEAD"]);
    let d = dir.display().to_string();
    // 스냅샷은 저장소 밖에 둔다 — 작업 트리 안에 두면 추적하지 않는 파일이 생긴다.
    let saved = temp_dir("snap-rev-out");
    let snapshot = saved.join("graph.json").display().to_string();
    let (code, _, err) = run(&[
        "graph", "--level", "symbol", "--dir", &d, "--out", &snapshot,
    ]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(read_json(&snapshot)["revision"], c1.as_str());
    // 같은 커밋에서 새로 수확한 순회와 graphRevision이 같다 — 기록한 revision은 해시에 들지 않는다.
    let (_, fresh, _) = run(&["reach", "--dir", &d, "app::a"]);
    let (_, from_snapshot, _) = run(&["reach", "--graph", &snapshot, "--dir", &d, "app::a"]);
    assert_eq!(
        json(&from_snapshot)["graphRevision"],
        json(&fresh)["graphRevision"]
    );
    // 소스를 고쳐 C2로 커밋한다 — 지금의 깨끗한 HEAD는 C2지만 스냅샷은 C1의 그래프다.
    std::fs::write(
        dir.join("app/src/lib.rs"),
        "pub fn a() { b(); c() }\npub fn b() {}\npub fn c() {}\n",
    )
    .unwrap();
    assert!(git(&dir, &["commit", "-q", "-am", "c2"]));
    let c2 = git_stdout(&dir, &["rev-parse", "HEAD"]);
    let (_, fresh, _) = run(&["reach", "--dir", &d, "app::a"]);
    assert_eq!(json(&fresh)["revision"], c2.as_str());
    for command in [
        vec!["reach", "app::a"],
        vec!["impact", "--format", "language-traversal", "app::b"],
    ] {
        let mut args = command.clone();
        args.extend(["--graph", snapshot.as_str(), "--dir", d.as_str()]);
        let (code, out, err) = run(&args);
        assert_eq!(code, 0, "{command:?}: {err}");
        let doc = json(&out);
        assert_eq!(doc["revision"], c1.as_str(), "{command:?}: {doc}");
        assert!(
            labeled_limitations(&doc, "revision-").is_empty(),
            "{command:?}: {doc}"
        );
    }
    // 명시 --revision이 이긴다.
    let (_, out, _) = run(&[
        "reach",
        "--graph",
        &snapshot,
        "--dir",
        &d,
        "--revision",
        "rev-7",
        "app::a",
    ]);
    assert_eq!(json(&out)["revision"], "rev-7");
    // --graph 입력을 다시 저장해도(레벨 투영 포함) 기록을 옮긴다 — 지금의 HEAD로 다시 읽지 않는다.
    let copy = saved.join("module.json").display().to_string();
    let (code, _, err) = run(&[
        "graph", "--graph", &snapshot, "--level", "module", "--out", &copy,
    ]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(read_json(&copy)["revision"], c1.as_str());
    // 표준 출력으로 내도 옮긴다 — revision을 싣지 않는 표준 출력은 수확한 그래프의 것뿐이다.
    let (code, projected, err) = run(&["graph", "--graph", &snapshot, "--level", "module"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(json(&projected)["revision"], c1.as_str());
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&saved);
}

#[test]
fn snapshot_traversal_without_a_usable_recorded_revision_omits_it_and_says_why() {
    let dir = git_workspace("snap-norev");
    if !git(&dir, &["init", "-q"]) {
        eprintln!("git unavailable — skipping snapshot revision test");
        return;
    }
    assert!(git(&dir, &["add", "-A"]) && git(&dir, &["commit", "-q", "-m", "init"]));
    let d = dir.display().to_string();
    let saved = temp_dir("snap-norev-out");
    // 표준 출력 그래프는 지금과 같은 바이트다 — revision을 싣지 않는다. 리디렉트한 스냅샷도 그렇다.
    let (code, graph, err) = run(&["graph", "--level", "symbol", "--dir", &d]);
    assert_eq!(code, 0, "{err}");
    assert!(json(&graph).get("revision").is_none(), "{graph}");
    let mut snapshots = vec![saved.join("redirected.json")];
    std::fs::write(&snapshots[0], &graph).unwrap();
    // 기록이 --revision 규칙(1~256자, 제어 문자·U+2028/2029 없음)을 어기면 쓰지 않는다.
    let bad_records = [
        "a\nb".to_string(),
        String::new(),
        "r".repeat(257),
        "r\u{2028}".to_string(),
    ];
    for (i, bad) in bad_records.into_iter().enumerate() {
        let mut tampered = json(&graph);
        tampered["revision"] = bad.into();
        let file = saved.join(format!("tampered-{i}.json"));
        std::fs::write(&file, tampered.to_string()).unwrap();
        snapshots.push(file);
    }
    // 더러운 트리에서 저장한 스냅샷 — HEAD가 분석한 소스가 아니라 기록하지 않는다.
    std::fs::write(dir.join("app/src/extra.rs"), "pub fn c() {}\n").unwrap();
    let dirty = saved.join("dirty.json").display().to_string();
    let (code, _, err) = run(&["graph", "--level", "symbol", "--dir", &d, "--out", &dirty]);
    assert_eq!(code, 0, "{err}");
    assert!(read_json(&dirty).get("revision").is_none());
    snapshots.push(PathBuf::from(dirty));
    // 트리를 다시 깨끗하게 둔다 — 그래도 스냅샷 순회는 지금의 HEAD를 싣지 않는다.
    std::fs::remove_file(dir.join("app/src/extra.rs")).unwrap();
    for snapshot in &snapshots {
        let path = snapshot.display().to_string();
        let (code, out, err) = run(&["reach", "--graph", &path, "--dir", &d, "app::a"]);
        assert_eq!(code, 0, "{path}: {err}");
        let doc = json(&out);
        assert!(doc.get("revision").is_none(), "{path}: {doc}");
        let notes = labeled_limitations(&doc, "revision-unknown:");
        assert_eq!(notes.len(), 1, "{path}: {doc}");
        assert!(notes[0].contains("--revision"), "{notes:?}");
        // 명시 --revision은 그대로 싣고 이유를 달지 않는다.
        let (_, out, _) = run(&[
            "reach",
            "--graph",
            &path,
            "--dir",
            &d,
            "--revision",
            "rev-7",
            "app::a",
        ]);
        let doc = json(&out);
        assert_eq!(doc["revision"], "rev-7", "{path}: {doc}");
        assert!(
            labeled_limitations(&doc, "revision-").is_empty(),
            "{path}: {doc}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&saved);
}

/// 실제 바이너리를 PATH `path`로 분석 대상 밖에서 실행해(종료 코드 0이어야 한다) 흔적을 남기는 `git`이
/// 돌았는지 본다.
#[cfg(unix)]
fn runs_git(kit: &MarkerKit, path: &std::ffi::OsStr, args: &[&str]) -> bool {
    kit.reset();
    let status = Command::new(env!("CARGO_BIN_EXE_rustograph"))
        .args(args)
        .current_dir(&kit.dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("PATH", path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(0), "{args:?}");
    !kit.fired().is_empty()
}

#[cfg(unix)]
#[test]
fn only_runs_that_read_a_revision_from_the_work_tree_run_git() {
    let kit = MarkerKit::new("snap-git");
    let Some(repo) = commit_all(git_workspace("snap-git")) else {
        return;
    };
    let dir = repo.display().to_string();
    let (code, graph, err) = run(&["graph", "--level", "symbol", "--dir", &dir]);
    assert_eq!(code, 0, "{err}");
    let snapshot = kit.dir.join("graph.json").display().to_string();
    std::fs::write(&snapshot, graph).unwrap();
    // PATH 맨 앞의 절대 경로 항목에 흔적을 남기는 `git`을 둔다 — rustograph가 git을 띄우면 이것이 돈다.
    let bin = kit.dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    std::fs::copy(kit.script("git", "exit 1"), bin.join("git")).unwrap();
    let mut path = bin.into_os_string();
    path.push(":");
    path.push(std::env::var_os("PATH").unwrap_or_default());
    let mermaid = kit.dir.join("graph.mmd").display().to_string();
    let copy = kit.dir.join("copy.json").display().to_string();
    // 저장 스냅샷을 읽는 순회와 표준 출력·Mermaid 그래프는 revision을 판독하지 않는다.
    for args in [
        vec!["reach", "--graph", &snapshot, "--dir", &dir, "app::a"],
        vec![
            "impact",
            "--format",
            "language-traversal",
            "--graph",
            &snapshot,
            "--dir",
            &dir,
            "app::b",
        ],
        vec!["graph", "--level", "symbol", "--dir", &dir],
        vec![
            "graph", "--format", "mermaid", "--dir", &dir, "--out", &mermaid,
        ],
        vec!["graph", "--graph", &snapshot, "--out", &copy],
    ] {
        assert!(!runs_git(&kit, &path, &args), "{args:?} ran git");
    }
    // 대조 — 작업 트리에서 revision을 판독하는 실행은 git을 띄운다(흔적 git이 실패해 revision은 빠진다).
    let fresh = kit.dir.join("fresh.json").display().to_string();
    for args in [
        vec!["reach", "--dir", &dir, "app::a"],
        vec!["graph", "--level", "symbol", "--dir", &dir, "--out", &fresh],
    ] {
        assert!(runs_git(&kit, &path, &args), "{args:?} did not run git");
    }
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&kit.dir);
}
