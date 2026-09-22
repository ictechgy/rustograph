//! MCP stdio 서버 — 코딩 에이전트가 프로세스를 띄워 그래프를 되묻는 통로다.
//!
//! 전송은 개행 구분 JSON-RPC 2.0(MCP stdio 전송 규약)이고, 문서는 기동 시
//! 한 번 수확해 모든 도구 호출이 같은 스냅샷 위에서 답한다 — 호출마다
//! 재수확하면 같은 세션 안에서 그래프가 흔들린다.
//! 반환 형식은 CLI의 JSON 출력과 같다 — 같은 계약을 두 통로가 공유한다.

use crate::cli_args::{self, Args};
use crate::graph::{Document, Level};
use crate::{analysis, config, export, rules};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

/// 들어오는 JSON-RPC 메시지. `id`가 없으면 알림이다 — 응답을 돌려보내지 않는다.
#[derive(Deserialize)]
struct Request {
    #[serde(default)]
    id: Option<serde_json::Value>,
    method: String,
    #[serde(default)]
    params: serde_json::Value,
}

/// JSON-RPC 응답. result와 error는 상호 배타적이다.
#[derive(Serialize)]
struct Response {
    jsonrpc: &'static str,
    id: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

/// JSON-RPC 오류 객체.
#[derive(Serialize)]
struct RpcError {
    code: i32,
    message: String,
}

/// 수확된 문서 하나를 서빙하는 서버 상태.
pub struct Server {
    doc: Document,
    /// rules 도구가 읽을 설정 파일 — 없으면 rules 호출이 isError로 답한다.
    cfg_path: Option<PathBuf>,
    /// 핸드셰이크로 합의한 프로토콜 버전.
    protocol: String,
}

/// `rustograph mcp` — 플래그를 해석해 문서를 한 번 얻고 EOF까지 서빙한다.
/// stdin은 파라미터로 받아 테스트에서 파이프 없이 서버를 구동한다.
pub(crate) fn cmd(
    a: &Args,
    stdin: &mut dyn BufRead,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<i32, String> {
    let doc = cli_args::document_for(a, true)?;
    let dir = PathBuf::from(a.get("dir").unwrap_or("."));
    let cfg_path = match a.get("config") {
        Some(f) => Some(PathBuf::from(f)),
        None => find_config(&dir),
    };
    let srv = Server {
        doc,
        cfg_path,
        protocol: "2024-11-05".to_string(),
    };
    Ok(srv.serve(stdin, out, err))
}

/// 디렉터리에서 규칙 파일 위치만 찾는다 — 내용 파싱은 호출 시점에 한다.
fn find_config(dir: &Path) -> Option<PathBuf> {
    for name in [".rustograph.yml", ".rustograph.yaml"] {
        let p = dir.join(name);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

impl Server {
    /// EOF까지 한 줄씩 JSON-RPC를 처리한다.
    fn serve(&self, stdin: &mut dyn BufRead, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
        let mut protocol = self.protocol.clone();
        for line in stdin.lines() {
            let line = match line {
                Ok(l) => l,
                Err(e) => {
                    let _ = writeln!(err, "mcp: reading stdin: {e}");
                    return 2;
                }
            };
            if line.trim().is_empty() {
                continue;
            }
            let Some(resp) = self.handle(&line, &mut protocol) else {
                continue;
            };
            match serde_json::to_string(&resp) {
                Ok(s) => {
                    let _ = writeln!(out, "{s}");
                    let _ = out.flush();
                }
                Err(e) => {
                    let _ = writeln!(err, "mcp: encoding response: {e}");
                }
            }
        }
        0
    }

    /// 메시지 하나를 디스패치한다. 알림(id 없음)은 None을 돌려 응답을 생략한다.
    fn handle(&self, line: &str, protocol: &mut String) -> Option<Response> {
        let req: Request = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => {
                return Some(Response {
                    jsonrpc: "2.0",
                    id: serde_json::Value::Null,
                    result: None,
                    error: Some(RpcError {
                        code: -32700,
                        message: format!("parse error: {e}"),
                    }),
                });
            }
        };
        let Some(id) = req.id else {
            return None; // 알림에는 절대 응답하지 않는다.
        };
        let reply = |result: Result<serde_json::Value, RpcError>| -> Option<Response> {
            match result {
                Ok(r) => Some(Response {
                    jsonrpc: "2.0",
                    id,
                    result: Some(r),
                    error: None,
                }),
                Err(e) => Some(Response {
                    jsonrpc: "2.0",
                    id,
                    result: None,
                    error: Some(e),
                }),
            }
        };
        match req.method.as_str() {
            "initialize" => {
                if let Some(p) = req.params.get("protocolVersion").and_then(|v| v.as_str()) {
                    *protocol = p.to_string();
                }
                reply(Ok(serde_json::json!({
                    "protocolVersion": protocol,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "rustograph", "version": env!("CARGO_PKG_VERSION")},
                })))
            }
            "ping" => reply(Ok(serde_json::json!({}))),
            "tools/list" => reply(Ok(serde_json::json!({"tools": tool_list()}))),
            "tools/call" => {
                let name = req
                    .params
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let args = req
                    .params
                    .get("arguments")
                    .cloned()
                    .unwrap_or(serde_json::Value::Object(Default::default()));
                // 도구 안의 실패는 프로토콜 오류가 아니라 isError 콘텐츠다 —
                // 호출은 성립했으므로 클라이언트에게 결과물로 돌려준다.
                reply(Ok(self.call_tool(name, &args)))
            }
            other => reply(Err(RpcError {
                code: -32601,
                message: format!("method not found: {other}"),
            })),
        }
    }

    /// 도구 하나를 실행해 MCP 도구 결과를 만든다.
    fn call_tool(&self, name: &str, args: &serde_json::Value) -> serde_json::Value {
        match self.run_tool(name, args) {
            Ok(text) => serde_json::json!({
                "content": [{"type": "text", "text": text}],
            }),
            Err(e) => serde_json::json!({
                "content": [{"type": "text", "text": e}],
                "isError": true,
            }),
        }
    }

    /// 도구별로 분석을 실행하고 JSON 문자열을 돌려준다.
    fn run_tool(&self, name: &str, args: &serde_json::Value) -> Result<String, String> {
        match name {
            "rustograph_summary" => Ok(export::to_json(&serde_json::json!({
                "version": self.doc.version,
                "level": self.doc.level,
                "root": self.doc.root,
                "workspace": self.doc.workspace,
                "roots": self.doc.roots,
                "vertices": self.doc.vertices.len(),
                "edges": self.doc.edges.len(),
                "limitations": self.doc.limitations,
            }))),
            "rustograph_query" => {
                let id = arg_str(args, "id").ok_or("query needs an \"id\" argument")?;
                let depth = arg_usize(args, "depth").unwrap_or(1);
                let max = arg_usize(args, "max").unwrap_or(200);
                Ok(export::to_json(&analysis::query(&self.doc, id, depth, max)))
            }
            "rustograph_impact" => {
                let id = arg_str(args, "id").ok_or("impact needs an \"id\" argument")?;
                // depth 0은 "전체 전이 클로저" — 내부에서는 무제한으로 바꾸되
                // 보고에는 요청값을 싣는다(usize::MAX는 계약이 아니다).
                let requested = arg_usize(args, "depth").unwrap_or(0);
                let effective = if requested == 0 {
                    usize::MAX
                } else {
                    requested
                };
                let max = arg_usize(args, "max").unwrap_or(200);
                let mut r = analysis::impact(&self.doc, id, effective, max);
                r.depth = requested;
                Ok(export::to_json(&r))
            }
            "rustograph_cycles" => {
                let level = match arg_str(args, "level") {
                    Some(l) => Level::parse(l)
                        .ok_or_else(|| format!("unknown level {l} (crate|module|type|symbol)"))?,
                    None => self.doc.level,
                };
                let view = self.doc.view(level);
                Ok(export::to_json(&analysis::cycles(&view)))
            }
            "rustograph_dead" => {
                // 문서 루트에 retainPublic·추가 루트를 얹은 사본 위에서 판정한다 —
                // 도구 호출이 서빙하는 스냅샷 자체를 흔들면 안 된다.
                let ids: std::collections::BTreeSet<String> = self
                    .doc
                    .vertex_ids()
                    .iter()
                    .map(|s| s.to_string())
                    .collect();
                let mut roots: std::collections::BTreeSet<String> =
                    self.doc.roots.iter().cloned().collect();
                let mut unknown_roots = Vec::new();
                if arg_bool(args, "retainPublic") {
                    for v in &self.doc.vertices {
                        if v.exported {
                            roots.insert(v.id.clone());
                        }
                    }
                }
                if let Some(extra) = args.get("roots").and_then(|v| v.as_array()) {
                    for r in extra.iter().filter_map(|v| v.as_str()) {
                        if ids.contains(r) {
                            roots.insert(r.to_string());
                        } else {
                            unknown_roots.push(r.to_string());
                        }
                    }
                }
                let mut d = self.doc.clone();
                d.roots = roots.into_iter().collect();
                let rep = analysis::dead(&d);
                let mut out = serde_json::to_value(&rep).expect("report must serialize");
                if !unknown_roots.is_empty() {
                    out["unknownRoots"] = serde_json::json!(unknown_roots);
                }
                Ok(export::to_json(&out))
            }
            "rustograph_rules" => {
                let Some(cfg_path) = &self.cfg_path else {
                    return Err("no .rustograph.yml found — pass --config".to_string());
                };
                let src = std::fs::read_to_string(cfg_path)
                    .map_err(|e| format!("cannot read {}: {e}", cfg_path.display()))?;
                let cfg: config::Config = serde_yml::from_str(&src)
                    .map_err(|e| format!("bad {}: {e}", cfg_path.display()))?;
                Ok(export::to_json(&rules::check(&self.doc, &cfg)))
            }
            other => Err(format!("unknown tool {other:?} — see tools/list")),
        }
    }
}

/// 도구 목록과 입력 스키마 — 계열의 이름 규칙(`<tool>_<query>`)을 따른다.
fn tool_list() -> serde_json::Value {
    serde_json::json!([
        {
            "name": "rustograph_summary",
            "description": "Document metadata: level, root, vertex/edge counts, measured limitations",
            "inputSchema": {"type": "object", "properties": {}},
        },
        {
            "name": "rustograph_query",
            "description": "Bidirectional neighbors of a vertex: what it depends on and what depends on it",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "vertex ID, e.g. mycrate::mod::func"},
                    "depth": {"type": "integer", "description": "neighbor depth (default 1)"},
                    "max": {"type": "integer", "description": "max neighbors (default 200)"},
                },
                "required": ["id"],
            },
        },
        {
            "name": "rustograph_impact",
            "description": "Reverse transitive closure: what breaks if this vertex changes",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "id": {"type": "string", "description": "vertex ID"},
                    "depth": {"type": "integer", "description": "max depth; 0 = full closure"},
                    "max": {"type": "integer", "description": "max dependers (default 200)"},
                },
                "required": ["id"],
            },
        },
        {
            "name": "rustograph_cycles",
            "description": "Dependency cycles at a level (default: the document's level)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "level": {"type": "string", "description": "crate|module|type|symbol"},
                },
            },
        },
        {
            "name": "rustograph_dead",
            "description": "Symbols unreachable from retention roots — graph facts, not delete verdicts",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "retainPublic": {"type": "boolean", "description": "retain all exported symbols (use for libraries)"},
                    "roots": {"type": "array", "items": {"type": "string"}, "description": "extra retention root vertex IDs"},
                },
            },
        },
        {
            "name": "rustograph_rules",
            "description": "Check layer rules from .rustograph.yml; returns violations and unmapped vertices",
            "inputSchema": {"type": "object", "properties": {}},
        },
    ])
}

/// 문자열 인자 하나를 꺼낸다.
fn arg_str<'a>(args: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(|v| v.as_str())
}

/// 정수 인자 하나를 꺼낸다.
fn arg_usize(args: &serde_json::Value, key: &str) -> Option<usize> {
    args.get(key).and_then(|v| v.as_u64()).map(|n| n as usize)
}

/// 불리언 인자 하나를 꺼낸다(없으면 false).
fn arg_bool(args: &serde_json::Value, key: &str) -> bool {
    args.get(key).and_then(|v| v.as_bool()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{document, Edge, EdgeKind, Kind, Vertex};

    fn v(id: &str, kind: Kind, exported: bool) -> Vertex {
        Vertex {
            id: id.to_string(),
            kind,
            krate: "c".to_string(),
            module: "c".to_string(),
            position: None,
            exported,
            generated: false,
            cfg: None,
            unsafe_: false,
        }
    }

    /// a->b 호출, 사용 안 되는 d — 최소 문서.
    fn test_server() -> Server {
        Server {
            doc: document(
                Level::Symbol,
                ".".into(),
                None,
                vec!["c::a".into()],
                vec![
                    v("c::a", Kind::Fn, true),
                    v("c::b", Kind::Fn, true),
                    v("c::d", Kind::Fn, false),
                ],
                vec![Edge::new("c::a".into(), "c::b".into(), EdgeKind::Call)],
                vec![],
            ),
            cfg_path: None,
            protocol: "2024-11-05".to_string(),
        }
    }

    /// NDJSON 요청을 밀어 넣고 응답 줄들을 JSON 값으로 돌려준다.
    fn session(srv: &Server, lines: &[&str]) -> Vec<serde_json::Value> {
        let input = lines.join("\n") + "\n";
        let mut in_: &[u8] = input.as_bytes();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = srv.serve(&mut in_, &mut out, &mut err);
        assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&err));
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    #[test]
    fn handshake_and_tools_list() {
        let res = session(
            &test_server(),
            &[
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            ],
        );
        // 알림은 응답을 만들지 않는다.
        assert_eq!(res.len(), 2);
        assert_eq!(res[0]["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(res[1]["result"]["tools"].as_array().unwrap().len(), 6);
    }

    #[test]
    fn tool_call_answers_over_the_graph() {
        let res = session(
            &test_server(),
            &[
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"rustograph_query","arguments":{"id":"c::b","depth":1}}}"#,
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"rustograph_dead","arguments":{}}}"#,
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"rustograph_impact","arguments":{"id":"c::b"}}}"#,
            ],
        );
        let text = |r: &serde_json::Value| {
            r["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .to_string()
        };
        // query: b의 incoming 이웃에 a.
        assert!(text(&res[0]).contains("\"c::a\""));
        // dead: d는 unreachable로 보고된다.
        assert!(text(&res[1]).contains("c::d"));
        // impact: b를 바꾸면 a가 깨진다.
        assert!(text(&res[2]).contains("c::a"));
    }

    #[test]
    fn tool_errors_are_content_not_protocol() {
        let res = session(
            &test_server(),
            &[
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"nope","arguments":{}}}"#,
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"rustograph_query","arguments":{}}}"#,
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"rustograph_rules","arguments":{}}}"#,
            ],
        );
        // 없는 도구·빠진 인자·없는 설정 파일 모두 isError 결과다.
        for r in &res {
            assert_eq!(r["result"]["isError"], true, "{r}");
        }
    }

    #[test]
    fn unknown_method_and_parse_error() {
        let res = session(
            &test_server(),
            &[
                r#"{"jsonrpc":"2.0","id":1,"method":"bogus/method"}"#,
                "not json at all",
                r#"{"jsonrpc":"2.0","method":"bogus/notify"}"#,
            ],
        );
        // 알림은 침묵 — 응답은 두 개뿐.
        assert_eq!(res.len(), 2);
        assert_eq!(res[0]["error"]["code"], -32601);
        assert_eq!(res[1]["error"]["code"], -32700);
    }

    #[test]
    fn cycles_tool_respects_level_arg() {
        let res = session(
            &test_server(),
            &[
                r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"rustograph_cycles","arguments":{"level":"module"}}}"#,
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"rustograph_cycles","arguments":{"level":"bogus"}}}"#,
                r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"rustograph_summary","arguments":{}}}"#,
            ],
        );
        assert_eq!(res[0]["result"]["isError"], serde_json::Value::Null);
        let text = res[0]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("\"cycles\""));
        assert_eq!(res[1]["result"]["isError"], true);
        // summary는 카운트와 limitation을 싣는다.
        let sum = res[2]["result"]["content"][0]["text"].as_str().unwrap();
        assert!(sum.contains("\"vertices\""));
    }
}
