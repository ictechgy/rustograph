//! 저장 스냅샷으로 CLI와 MCP의 방향·상한 계약을 실제 바이너리에서 대조한다.

use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

struct Snapshot(PathBuf);

impl Snapshot {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "rustograph-analysis-{}-{name}.json",
            std::process::id()
        ));
        let vertices: Vec<_> = ["root", "shared", "sibling", "caller", "entry"]
            .into_iter()
            .map(|id| json!({"id": id, "kind": "fn", "crate": "app", "module": "app"}))
            .collect();
        let doc = json!({
            "version": 1, "tool": "rustograph", "level": "symbol", "root": ".",
            "roots": ["entry"], "vertices": vertices,
            "edges": [
                {"from": "root", "to": "shared", "kind": "call"},
                {"from": "sibling", "to": "shared", "kind": "call"},
                {"from": "caller", "to": "root", "kind": "call"},
                {"from": "caller", "to": "root", "kind": "references"},
                {"from": "entry", "to": "caller", "kind": "call"}
            ]
        });
        std::fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();
        Self(path)
    }

    fn cli(&self, args: &[&str]) -> Value {
        let output = Command::new(env!("CARGO_BIN_EXE_rustograph"))
            .args(args)
            .arg("--graph")
            .arg(&self.0)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        std::fs::remove_file(&self.0).unwrap();
    }
}

#[test]
fn cli_impact_reports_only_reverse_reachability() {
    let snapshot = Snapshot::new("cli-direction");
    let report = snapshot.cli(&["impact", "root", "--depth", "10", "--max", "2"]);
    assert_eq!(
        report["impacted"],
        json!([
            {"id": "caller", "edges": ["call", "references"], "direction": "in", "depth": 1},
            {"id": "entry", "edges": ["call"], "direction": "in", "depth": 2}
        ])
    );
    assert_eq!(report["truncated"], false);
}

#[test]
fn cli_query_zero_and_exact_caps_are_truthful() {
    let snapshot = Snapshot::new("cli-limits");
    for (max, expected_count, truncated) in [("0", 0, true), ("1", 1, true), ("2", 2, false)] {
        let report = snapshot.cli(&["query", "root", "--depth", "1", "--max", max]);
        assert_eq!(
            report["neighbors"].as_array().unwrap().len(),
            expected_count
        );
        assert_eq!(report["truncated"], truncated);
    }
}

#[test]
fn mcp_and_cli_share_directional_results_and_caps() {
    let snapshot = Snapshot::new("mcp-direction");
    let mut child = Command::new(env!("CARGO_BIN_EXE_rustograph"))
        .args(["mcp", "--graph"])
        .arg(&snapshot.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let request = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "rustograph_impact", "arguments": {"id": "root", "depth": 10, "max": 2}}
    });
    writeln!(child.stdin.take().unwrap(), "{request}").unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    let report: Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        report,
        snapshot.cli(&["impact", "root", "--depth", "10", "--max", "2"])
    );
}
