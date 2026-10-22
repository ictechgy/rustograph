//! 실제 Git 변경을 두 소스 수확에 연결하는 회귀 — 파일 이름과 호출 방향을 고정한다.
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);
struct Repo(PathBuf);
impl Repo {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rustograph-change-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(path.join("src")).unwrap();
        git(&path, &["init", "-q"]);
        git(&path, &["config", "user.email", "fixture@example.invalid"]);
        git(&path, &["config", "user.name", "Fixture"]);
        std::fs::write(
            path.join("Cargo.toml"),
            "[package]\nname='patch_fixture'\nversion='0.1.0'\nedition='2021'\n",
        )
        .unwrap();
        std::fs::write(path.join(".gitignore"), "/target/\n").unwrap();
        std::fs::write(
            path.join("src/lib.rs"),
            "pub mod target; pub mod caller; pub mod entry;\n",
        )
        .unwrap();
        std::fs::write(path.join("src/target.rs"), "pub fn removed() {}\n").unwrap();
        std::fs::write(
            path.join("src/caller.rs"),
            "pub fn caller() { crate::target::removed(); }\n",
        )
        .unwrap();
        std::fs::write(
            path.join("src/entry.rs"),
            "pub fn entry() {}\n#[test] fn connected() { entry(); }\n",
        )
        .unwrap();
        let lock = Command::new("cargo")
            .args(["generate-lockfile", "--offline", "--manifest-path"])
            .arg(path.join("Cargo.toml"))
            .output()
            .unwrap();
        assert!(
            lock.status.success(),
            "{}",
            String::from_utf8_lossy(&lock.stderr)
        );
        git(&path, &["add", "."]);
        git(&path, &["commit", "-qm", "before"]);
        Self(path)
    }
    fn base(&self) -> String {
        git(&self.0, &["rev-parse", "HEAD"]).trim().to_owned()
    }
    fn report(&self, base: &str, extra: &[&str]) -> Value {
        let mut args = vec![
            "change-impact",
            "--dir",
            self.0.to_str().unwrap(),
            "--base",
            base,
        ];
        args.extend_from_slice(extra);
        let result = Command::new(env!("CARGO_BIN_EXE_rustograph"))
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        serde_json::from_slice(&result.stdout).unwrap()
    }
}
impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn git(dir: &Path, args: &[&str]) -> String {
    let result = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap()
}
fn has(rows: &Value, id: &str) -> bool {
    rows.as_array().unwrap().iter().any(|r| r["id"] == id)
}

#[test]
fn deletion_keeps_historical_callers_without_hybrid_revision_paths() {
    let repo = Repo::new();
    let base = repo.base();
    std::fs::write(repo.0.join("src/target.rs"), "// deleted declaration\n").unwrap();
    std::fs::write(repo.0.join("src/caller.rs"), "pub fn caller() {}\n").unwrap();
    std::fs::write(
        repo.0.join("src/entry.rs"),
        "pub fn entry() { crate::caller::caller(); }\n#[test] fn connected() { entry(); }\n",
    )
    .unwrap();
    git(&repo.0, &["add", "."]);
    git(&repo.0, &["commit", "-qm", "after"]);
    let report = repo.report(&base, &[]);
    let old = report["before"]["impacts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "patch_fixture::target::removed")
        .unwrap();
    assert!(has(&old["impacted"], "patch_fixture::caller::caller"));
    assert!(!has(&old["impacted"], "patch_fixture::entry::entry"));
    assert!(!has(
        &report["after"]["seeds"],
        "patch_fixture::target::removed"
    ));
    assert_eq!(report["mode"], "git-change");
    assert_eq!(report["state"], "incomplete");
    assert_eq!(report["before"]["provenance"]["revision"], base);
}

#[test]
fn rename_and_dirty_body_changes_record_both_paths_and_content() {
    let repo = Repo::new();
    let base = repo.base();
    git(&repo.0, &["mv", "src/target.rs", "src/renamed.rs"]);
    std::fs::write(
        repo.0.join("src/lib.rs"),
        "pub mod renamed; pub mod caller; pub mod entry;\n",
    )
    .unwrap();
    std::fs::write(
        repo.0.join("src/caller.rs"),
        "pub fn caller() { crate::renamed::removed(); }\n",
    )
    .unwrap();
    let first = repo.report(&base, &["--working-tree"]);
    assert!(
        has(&first["before"]["seeds"], "patch_fixture::target::removed"),
        "{first:#}"
    );
    assert!(has(
        &first["after"]["seeds"],
        "patch_fixture::renamed::removed"
    ));
    std::fs::write(
        repo.0.join("src/renamed.rs"),
        "pub fn removed() { let _x = 2; }\n",
    )
    .unwrap();
    let second = repo.report(&base, &["--working-tree"]);
    assert_ne!(
        first["after"]["provenance"]["content_digest"],
        second["after"]["provenance"]["content_digest"]
    );
    assert_eq!(second["after"]["provenance"]["freshness"], "captured");
}

#[test]
fn untracked_and_configuration_changes_are_explicit_candidates() {
    let repo = Repo::new();
    let base = repo.base();
    std::fs::write(repo.0.join("settings.json"), "{\"callback\":\"entry\"}").unwrap();
    let report = repo.report(&base, &["--working-tree"]);
    assert!(report["changes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["new_path"] == "settings.json"));
    assert!(report["limitations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s.as_str().unwrap().contains("non-Rust")));
    assert_eq!(report["state"], "incomplete");
}

#[test]
fn staged_deletion_does_not_restore_head_source_in_captured_working_tree() {
    let repo = Repo::new();
    let base = repo.base();
    git(&repo.0, &["rm", "src/target.rs"]);
    // Cargo 변경으로 전체 선언을 드러내되 사라진 모듈 선언은 그대로 남긴다.
    let manifest = repo.0.join("Cargo.toml");
    let mut text = std::fs::read_to_string(&manifest).unwrap();
    text.push_str("description='staged deletion regression'\n");
    std::fs::write(manifest, text).unwrap();
    let report = repo.report(&base, &["--working-tree"]);
    assert!(!has(
        &report["after"]["seeds"],
        "patch_fixture::target::removed"
    ));
    assert_eq!(report["after"]["provenance"]["freshness"], "captured");
    assert_eq!(report["state"], "incomplete");
    assert!(report["after"]["limitations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|s| s.as_str().unwrap().contains("could not be resolved")));
    let old = report["before"]["impacts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"] == "patch_fixture::target::removed")
        .unwrap();
    assert!(has(&old["impacted"], "patch_fixture::caller::caller"));
    assert!(!repo.0.join("src/target.rs").exists());
}

#[test]
fn staged_rename_removes_old_path_even_when_the_old_module_is_still_declared() {
    let repo = Repo::new();
    let base = repo.base();
    git(&repo.0, &["mv", "src/target.rs", "src/renamed.rs"]);
    std::fs::write(
        repo.0.join("src/lib.rs"),
        "pub mod target; pub mod renamed; pub mod caller; pub mod entry;\n",
    )
    .unwrap();
    let manifest = repo.0.join("Cargo.toml");
    let mut text = std::fs::read_to_string(&manifest).unwrap();
    text.push_str("description='staged rename regression'\n");
    std::fs::write(manifest, text).unwrap();
    let report = repo.report(&base, &["--working-tree"]);
    assert!(!has(
        &report["after"]["seeds"],
        "patch_fixture::target::removed"
    ));
    assert!(has(
        &report["after"]["seeds"],
        "patch_fixture::renamed::removed"
    ));
    assert_eq!(report["after"]["provenance"]["freshness"], "captured");
    assert_eq!(report["state"], "incomplete");
    assert!(!repo.0.join("src/target.rs").exists());
}

#[test]
fn native_mcp_and_cli_use_the_same_patch_contract() {
    let repo = Repo::new();
    let base = repo.base();
    std::fs::write(
        repo.0.join("src/target.rs"),
        "pub fn removed() { let _value = 42; }\n",
    )
    .unwrap();
    let cli = repo.report(&base, &["--working-tree", "--depth", "1", "--max", "1"]);
    let mut process = Command::new(env!("CARGO_BIN_EXE_rustograph"))
        .args(["mcp", "--dir"])
        .arg(&repo.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let request = serde_json::json!({"jsonrpc":"2.0", "id":1, "method":"tools/call", "params": {
        "name":"rustograph_change_impact", "arguments":{"base":base, "working_tree":true,"depth":1,"max":1}
    }});
    writeln!(process.stdin.take().unwrap(), "{request}").unwrap();
    let output = process.wait_with_output().unwrap();
    assert!(output.status.success());
    let rpc: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_ne!(rpc["result"]["isError"], true, "{rpc}");
    let mcp: Value =
        serde_json::from_str(rpc["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(cli["before"], mcp["before"]);
    assert_eq!(cli["after"], mcp["after"]);
    assert_eq!(cli["changes"], mcp["changes"]);
}

#[test]
fn trait_callback_candidates_compile_and_preserve_uncertainty() {
    let repo = Repo::new();
    let base = repo.base();
    std::fs::write(repo.0.join("src/target.rs"), "pub trait Action { fn run(&self); }\npub struct Service;\nimpl Action for Service { fn run(&self) {} }\npub fn removed() {}\npub fn callback(f: fn()) { f(); }\n").unwrap();
    std::fs::write(repo.0.join("src/caller.rs"), "use crate::target::{Action, Service};\npub fn caller() { Service.run(); crate::target::callback(crate::target::removed); }\n#[test] fn behavior() { caller(); }\n").unwrap();
    let checked = Command::new("cargo")
        .args(["test", "--offline", "--quiet", "--manifest-path"])
        .arg(repo.0.join("Cargo.toml"))
        .output()
        .unwrap();
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    let report = repo.report(&base, &["--working-tree", "--depth", "0"]);
    assert!(report["after"]["review_roots"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r == "patch_fixture::caller::behavior"));
    assert!(report["limitations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r.as_str().unwrap().contains("callbacks")));
    assert_eq!(report["state"], "incomplete");
}

#[test]
fn invalid_revision_and_conflicting_inputs_fail_instead_of_empty_success() {
    let repo = Repo::new();
    for flags in [
        vec!["--base", "does-not-exist"],
        vec!["--base", "HEAD", "--head", "HEAD", "--working-tree"],
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_rustograph"))
            .args(["change-impact", "--dir"])
            .arg(&repo.0)
            .args(flags)
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(result.stdout.is_empty());
    }
}
