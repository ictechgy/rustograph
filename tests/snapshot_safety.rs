//! 저장 문서 세밀도와 파일 출력은 실패 시 원본을 보존해야 한다.
use rustograph::{
    analysis, export,
    graph::{document, Level},
};
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::Stdio;

struct Fixture(PathBuf);
impl Fixture {
    fn new(name: &str) -> Self {
        let directory =
            std::env::temp_dir().join(format!("rustograph-safety-{}-{name}", std::process::id()));
        std::fs::create_dir_all(directory.join("src")).unwrap();
        std::fs::write(
            directory.join("Cargo.toml"),
            "[package]\nname='safety_fixture'\nversion='0.1.0'\nedition='2021'\n",
        )
        .unwrap();
        std::fs::write(
            directory.join("src/lib.rs"),
            "pub fn entry() {}\nfn unused() {}\n",
        )
        .unwrap();
        Self(directory)
    }
    fn snapshot(&self, level: Level) -> PathBuf {
        let path = self.0.join("graph.json");
        let doc = document(
            level,
            self.0.display().to_string(),
            None,
            vec![],
            vec![],
            vec![],
            vec![],
        );
        export::save_file(&doc, &path).unwrap();
        path
    }
    fn run(&self, args: &[&str], output: &Path) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_rustograph"))
            .args(args)
            .arg("--dir")
            .arg(&self.0)
            .arg("--out")
            .arg(output)
            .output()
            .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn coarse_snapshots_cannot_be_promoted_or_report_symbol_analysis() {
    for level in [Level::Crate, Level::Module, Level::Type] {
        let fixture = Fixture::new(&format!("level-{level:?}"));
        let path = fixture.snapshot(level);
        let doc = export::load_file(&path).unwrap();
        assert!(doc.view(Level::Symbol).unwrap_err().contains("regenerate"));
        assert!(analysis::dead(&doc).unwrap_err().contains("regenerate"));
        for args in [
            vec!["dead", "--strict"],
            vec!["graph", "--level", "symbol"],
            vec!["cycles", "--level", "symbol", "--strict"],
            vec!["impact", "missing"],
            vec!["reach", "missing"],
        ] {
            let output = Command::new(env!("CARGO_BIN_EXE_rustograph"))
                .args(args)
                .arg("--graph")
                .arg(&path)
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(2));
            assert!(output.stdout.is_empty());
            assert!(String::from_utf8_lossy(&output.stderr).contains("regenerate"));
        }
        let output = Command::new(env!("CARGO_BIN_EXE_rustograph"))
            .args(["graph", "--level", "crate", "--graph"])
            .arg(&path)
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap()["level"],
            "crate"
        );
    }
}

#[test]
fn mcp_keeps_summary_available_but_rejects_insufficient_analysis() {
    let fixture = Fixture::new("mcp");
    let path = fixture.snapshot(Level::Module);
    let mut child = Command::new(env!("CARGO_BIN_EXE_rustograph"))
        .args(["mcp", "--graph"])
        .arg(path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    for (id, name, arguments) in [
        (1, "rustograph_summary", json!({})),
        (2, "rustograph_dead", json!({})),
        (3, "rustograph_cycles", json!({"level":"symbol"})),
        (4, "rustograph_impact", json!({"id":"missing"})),
    ] {
        writeln!(input, "{}", json!({"jsonrpc":"2.0", "id":id, "method":"tools/call", "params":{"name":name,"arguments":arguments}})).unwrap();
    }
    drop(input);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let replies: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(replies.len(), 4);
    assert!(replies[0]["result"]["isError"].is_null());
    for reply in &replies[1..] {
        assert_eq!(reply["result"]["isError"], true);
        assert!(reply["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("regenerate"));
    }
}

#[test]
fn graph_input_aliases_are_preserved_in_both_formats() {
    let fixture = Fixture::new("input-alias");
    let input = fixture.snapshot(Level::Symbol);
    for format in ["json", "mermaid"] {
        let before = std::fs::read(&input).unwrap();
        let output = fixture.run(
            &[
                "graph",
                "--graph",
                input.to_str().unwrap(),
                "--format",
                format,
            ],
            &input,
        );
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(std::fs::read(&input).unwrap(), before);
    }
}

#[test]
fn every_output_command_preserves_source_and_manifest() {
    let fixture = Fixture::new("source");
    for args in [
        vec!["graph"],
        vec!["graph", "--format", "mermaid"],
        vec!["schema"],
        vec!["routes", "--role", "server"],
        vec!["routes", "--role", "client"],
    ] {
        for path in [fixture.0.join("src/lib.rs"), fixture.0.join("Cargo.toml")] {
            let before = std::fs::read(&path).unwrap();
            let output = fixture.run(&args, &path);
            assert_eq!(
                output.status.code(),
                Some(2),
                "{args:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(String::from_utf8_lossy(&output.stderr).contains("aliases an input"));
            assert_eq!(std::fs::read(&path).unwrap(), before);
        }
    }
}

#[cfg(unix)]
#[test]
fn graph_outputs_reject_hardlinks_and_symbolic_links() {
    let fixture = Fixture::new("links");
    let input = fixture.snapshot(Level::Symbol);
    let hardlink = fixture.0.join("alias.json");
    std::fs::hard_link(&input, &hardlink).unwrap();
    let symlink = fixture.0.join("link.json");
    std::os::unix::fs::symlink(&input, &symlink).unwrap();
    let before = std::fs::read(&input).unwrap();
    for alias in [hardlink, symlink] {
        for format in ["json", "mermaid"] {
            let output = fixture.run(
                &[
                    "graph",
                    "--graph",
                    input.to_str().unwrap(),
                    "--format",
                    format,
                ],
                &alias,
            );
            assert_eq!(output.status.code(), Some(2));
            assert_eq!(std::fs::read(&input).unwrap(), before);
        }
    }
}

#[test]
fn client_routes_preserve_wrapper_input_and_custom_source_paths() {
    let fixture = Fixture::new("wrappers");
    let wrappers = fixture.0.join("wrappers.json");
    let content = r#"{"format":"http-wrappers","version":1,"wrappers":[]}"#;
    std::fs::write(&wrappers, content).unwrap();
    let output = fixture.run(
        &[
            "routes",
            "--role",
            "client",
            "--wrappers",
            wrappers.to_str().unwrap(),
        ],
        &wrappers,
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("aliases an input"));
    assert_eq!(std::fs::read_to_string(&wrappers).unwrap(), content);
    let custom = fixture.0.join("source.txt");
    std::fs::write(&custom, "pub fn entry() {}\n").unwrap();
    std::fs::write(
        fixture.0.join("Cargo.toml"),
        "[package]\nname='safety_fixture'\nversion='0.1.0'\n[lib]\npath='source.txt'\n",
    )
    .unwrap();
    let before = std::fs::read(&custom).unwrap();
    for args in [
        vec!["graph"],
        vec!["schema"],
        vec!["routes", "--role", "server"],
    ] {
        assert_eq!(fixture.run(&args, &custom).status.code(), Some(2));
        assert_eq!(std::fs::read(&custom).unwrap(), before);
    }
}

#[test]
fn saved_graph_baseline_preserves_default_config_outside_snapshot_root() {
    let fixture = Fixture::new("baseline-config");
    let snapshot = fixture.snapshot(Level::Symbol);
    let directory = fixture.0.join("configuration");
    std::fs::create_dir_all(&directory).unwrap();
    for name in [".rustograph.yml", ".rustograph.yaml"] {
        let config = directory.join(name);
        let original = "components: {}\ndeps: {}\n";
        std::fs::write(&config, original).unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_rustograph"))
            .args(["rules", "--write-baseline", "--graph"])
            .arg(&snapshot)
            .arg("--dir")
            .arg(&directory)
            .arg("--baseline")
            .arg(&config)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stderr).contains("aliases an input"));
        assert_eq!(std::fs::read_to_string(&config).unwrap(), original);
        std::fs::remove_file(config).unwrap();
    }
}

#[test]
fn literal_include_inputs_are_protected_and_unresolved_paths_fail_closed() {
    let fixture = Fixture::new("include-inputs");
    std::fs::write(fixture.0.join("src/lib.rs"), "include!(\"../extra.inc\");\nconst TEXT: &str = include_str!(\"../data.txt\");\nconst BYTES: &[u8] = include_bytes!(\"../data.bin\");\n").unwrap();
    for (name, content) in [
        ("extra.inc", "include!(\"nested.inc\");"),
        ("nested.inc", "pub fn included() {}"),
        ("data.txt", "text source"),
        ("data.bin", "binary source"),
    ] {
        std::fs::write(fixture.0.join(name), content).unwrap();
    }
    for name in ["extra.inc", "nested.inc", "data.txt", "data.bin"] {
        let input = fixture.0.join(name);
        let before = std::fs::read(&input).unwrap();
        let output = fixture.run(&["graph"], &input);
        assert_eq!(output.status.code(), Some(2));
        assert_eq!(std::fs::read(&input).unwrap(), before);
    }
    std::fs::write(
        fixture.0.join("src/lib.rs"),
        "include!(concat!(env!(\"OUT_DIR\"), \"/generated.rs\"));\n",
    )
    .unwrap();
    let existing = fixture.0.join("report.json");
    std::fs::write(&existing, "previous report").unwrap();
    let output = fixture.run(&["graph"], &existing);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("unresolved include paths"));
    assert_eq!(
        std::fs::read_to_string(&existing).unwrap(),
        "previous report"
    );
    assert!(fixture
        .run(&["graph"], &fixture.0.join("new-report.json"))
        .status
        .success());
}

#[test]
fn macro_body_include_inputs_are_not_certified_safe_to_replace() {
    let fixture = Fixture::new("macro-include");
    std::fs::write(fixture.0.join("src/lib.rs"), "macro_rules! text { () => { include_str!(\"../asset.txt\") } } const TEXT: &str = text!();\n").unwrap();
    let asset = fixture.0.join("asset.txt");
    std::fs::write(&asset, "source asset").unwrap();
    let output = fixture.run(&["graph"], &asset);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("unresolved include paths"));
    assert_eq!(std::fs::read_to_string(asset).unwrap(), "source asset");
}

/// 선언된 모듈 파일을 읽거나 파싱하지 못해도 `schema`는 그 파일을 unparsed-sources로 세고
/// 문서를 낸다(그래프를 수확하지 않는 `routes --role server`도 같다) — 입력 보호 목록
/// 수집이 그 파일에서 실패하면 `--out`일 때만 종료 2였다. 그 파일은 보호하되 그 파일이
/// 선언한 하위 모듈·include 입력은 모르므로 기존 출력 교체만 보수적으로 거부하고, 새
/// 경로에는 표준 출력과 같은 문서를 쓴다. 심볼 그래프가 필요한 graph는 그대로 실패한다.
#[test]
fn unparsable_modules_refuse_only_replacing_existing_outputs() {
    let fixture = Fixture::new("unparsable-module");
    std::fs::write(
        fixture.0.join("src/lib.rs"),
        "pub mod broken;\npub fn entry() { let _q = \"SELECT id FROM users\"; }\n",
    )
    .unwrap();
    let broken = fixture.0.join("src/broken.rs");
    std::fs::write(&broken, "pub mod child;\npub fn oops( {\n").unwrap();
    std::fs::create_dir_all(fixture.0.join("src/broken")).unwrap();
    let child = fixture.0.join("src/broken/child.rs");
    std::fs::write(&child, "pub fn child() {}\n").unwrap();
    let existing = fixture.0.join("report.json");
    std::fs::write(&existing, "previous report").unwrap();
    let fresh = fixture.0.join("facts.json");
    let without_clock = |bytes: &[u8]| {
        let mut doc: Value = serde_json::from_slice(bytes).unwrap();
        doc.as_object_mut().unwrap().remove("generatedAt");
        doc
    };
    for args in [vec!["schema"], vec!["routes", "--role", "server"]] {
        let stdout = Command::new(env!("CARGO_BIN_EXE_rustograph"))
            .args(&args)
            .arg("--dir")
            .arg(&fixture.0)
            .output()
            .unwrap();
        assert_eq!(stdout.status.code(), Some(0), "{args:?}");
        let output = fixture.run(&args, &fresh);
        assert_eq!(
            output.status.code(),
            Some(0),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let written = without_clock(&std::fs::read(&fresh).unwrap());
        assert_eq!(written, without_clock(&stdout.stdout), "{args:?}");
        if args[0] == "schema" {
            let limitations = written["limitations"].as_array().unwrap();
            assert!(
                limitations
                    .iter()
                    .any(|l| l.as_str().unwrap().starts_with("unparsed-sources: 1 ")),
                "{limitations:?}"
            );
        }
        std::fs::remove_file(&fresh).unwrap();
        for target in [&existing, &broken, &child] {
            let before = std::fs::read(target).unwrap();
            let output = fixture.run(&args, target);
            assert_eq!(output.status.code(), Some(2), "{args:?} {target:?}");
            assert!(
                String::from_utf8_lossy(&output.stderr).contains("could not be read or parsed"),
                "{args:?} {target:?}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(std::fs::read(target).unwrap(), before);
        }
    }
    let output = fixture.run(&["graph"], &fresh);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("parse error"));
    assert!(!fresh.exists());
}

#[test]
fn staged_output_never_truncates_a_hardlinked_input() {
    let fixture = Fixture::new("hardlink-preservation");
    let input = fixture.0.join("input.json");
    let alias = fixture.0.join("output.json");
    std::fs::write(&input, "original input").unwrap();
    std::fs::hard_link(&input, &alias).unwrap();
    let result = export::write_file(&alias, b"complete output", std::slice::from_ref(&input));
    assert_eq!(std::fs::read_to_string(&input).unwrap(), "original input");
    if result.is_ok() {
        assert_eq!(std::fs::read_to_string(alias).unwrap(), "complete output");
    } else {
        assert_eq!(std::fs::read_to_string(alias).unwrap(), "original input");
    }
}

/// 정규 파일이 아닌 include! 대상은 열지 않고 해석 못 한 경로로 센다 — FIFO는 여는 순간
/// 쓰는 쪽을 영원히 기다리고 `/dev/zero`는 끝없이 읽혀 `--out`의 입력 보호 수집이 끝나지
/// 않았다. 기존 출력 교체는 보수적으로 거부하고(2) 새 경로에는 그대로 쓴다.
#[cfg(unix)]
#[test]
fn special_include_targets_fail_closed_without_reading_them() {
    let fixture = Fixture::new("special-include");
    let status = Command::new("mkfifo")
        .arg(fixture.0.join("pipe.inc"))
        .status()
        .expect("mkfifo must run");
    assert!(status.success());
    for target in ["../pipe.inc", "/dev/zero"] {
        std::fs::write(
            fixture.0.join("src/lib.rs"),
            format!("include!(\"{target}\");\npub fn entry() {{}}\n"),
        )
        .unwrap();
        let existing = fixture.0.join("report.json");
        std::fs::write(&existing, "previous report").unwrap();
        let output = fixture.run(&["graph"], &existing);
        assert_eq!(output.status.code(), Some(2), "{target}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("unresolved include paths"),
            "{target}"
        );
        assert_eq!(
            std::fs::read_to_string(&existing).unwrap(),
            "previous report"
        );
        let fresh = fixture.0.join("new-report.json");
        assert!(fixture.run(&["graph"], &fresh).status.success(), "{target}");
        std::fs::remove_file(fresh).unwrap();
    }
}
