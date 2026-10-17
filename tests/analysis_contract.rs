//! 저장 스냅샷으로 CLI와 MCP의 방향·상한 계약을 실제 바이너리에서 대조한다.

use serde_json::{json, Value};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

struct Snapshot(PathBuf);

impl Snapshot {
    /// 문서 하나를 테스트별 임시 파일로 쓴다.
    fn write(name: &str, doc: &Value) -> Self {
        let path = std::env::temp_dir().join(format!(
            "rustograph-analysis-{}-{name}.json",
            std::process::id()
        ));
        std::fs::write(&path, serde_json::to_vec(doc).unwrap()).unwrap();
        Self(path)
    }

    /// 정점 n개의 확정 call 고리 — DFS 깊이가 n인 SCC 하나다.
    fn ring(name: &str, n: usize) -> Self {
        let id = |i: usize| format!("app::f{i}");
        let vertices: Vec<_> = (0..n)
            .map(|i| json!({"id": id(i), "kind": "fn", "crate": "app", "module": "app"}))
            .collect();
        let edges: Vec<_> = (0..n)
            .map(|i| json!({"from": id(i), "to": id((i + 1) % n), "kind": "call"}))
            .collect();
        let doc = json!({
            "version": 1, "tool": "rustograph", "level": "symbol", "root": ".",
            "roots": [id(0)], "vertices": vertices, "edges": edges
        });
        Self::write(name, &doc)
    }

    fn new(name: &str) -> Self {
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
        Self::write(name, &doc)
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

/// 재귀 Tarjan은 고리 수만 정점에서 메인 스레드 스택을 넘겨 MCP 서버째 abort시켰다
/// (종료 코드 134 — 같은 세션의 다른 도구도 함께 사라진다). cycles에 답하고 다음
/// 요청(ping)에도 답해야 한다.
#[test]
fn mcp_cycles_answers_on_a_deep_ring() {
    let n = 50_000;
    let snapshot = Snapshot::ring("mcp-deep-ring", n);
    let mut child = Command::new(env!("CARGO_BIN_EXE_rustograph"))
        .args(["mcp", "--graph"])
        .arg(&snapshot.0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let cycles = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "rustograph_cycles", "arguments": {}}
    });
    let ping = json!({"jsonrpc": "2.0", "id": 2, "method": "ping"});
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{cycles}\n{ping}").unwrap();
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let responses: Vec<Value> = output
        .stdout
        .split(|&b| b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect();
    assert_eq!(responses.len(), 2);
    let report: Value = serde_json::from_str(
        responses[0]["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(report["cycles"].as_array().unwrap().len(), 1);
    assert_eq!(report["cycles"][0]["members"].as_array().unwrap().len(), n);
    assert_eq!(responses[1]["id"], 2);
    assert_eq!(responses[1]["result"], json!({}));
}
