//! MCP stdio 서버 — 코딩 에이전트가 프로세스를 띄워 그래프를 되묻는 통로다.
//!
//! 전송은 개행 구분 JSON-RPC 2.0(MCP stdio 전송 규약)이고, 문서는 기동 시
//! 한 번 수확해 모든 도구 호출이 같은 스냅샷 위에서 답한다 — 호출마다
//! 재수확하면 같은 세션 안에서 그래프가 흔들린다.
//! 반환 형식은 CLI의 JSON 출력과 같다 — 같은 계약을 두 통로가 공유한다.

use crate::cli_args::{self, Args};
use crate::graph::{Document, Level};
use crate::{analysis, config, deps, export, rules};
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

/// 요청 한 줄의 최대 바이트 수(줄 끝 `\n`·`\r\n` 제외) — 1 MiB. 계열 MCP 서버(schemagraph·cartograph의
/// 1 MiB 요청 상한)와 같다. 이 서버의 요청은 정점 ID와 정수 몇 개라 수백 바이트면 된다 — 상한이 없으면
/// 개행 없는 입력 하나가 서버 메모리를 끝없이 쌓는다.
const MAX_REQUEST_LINE_BYTES: usize = 1024 * 1024;

#[cfg(test)]
thread_local! {
    /// 요청 줄 버퍼가 담았던 가장 긴 내용(바이트) — 상한을 넘는 줄도 버퍼가 상한에 묶이는지
    /// 단위 테스트가 본다(테스트 스레드마다 따로).
    static LINE_BUFFER_PEAK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// stdin에서 읽은 요청 줄 하나.
enum RequestLine {
    /// 줄 끝을 뗀 줄 내용(UTF-8).
    Text(String),
    /// 상한을 넘은 줄 — 내용을 쌓지 않고 다음 개행(또는 EOF)까지 읽어 버렸다.
    Oversized,
}

/// 다음 요청 줄을 읽는다 — EOF면 None. 줄 내용은 [`MAX_REQUEST_LINE_BYTES`]까지만 버퍼에 쌓고, 넘으면
/// 나머지는 읽어 버리기만 해 메모리가 상한에 묶인다. 줄 끝·오류 처리는 예전 `BufRead::lines`와 같다:
/// `\n`을 떼고 그 앞 `\r`도 떼며, 개행 없이 EOF로 끝난 마지막 줄도 한 줄이고, UTF-8이 아닌 줄은 읽기 오류다.
fn read_request_line(stdin: &mut dyn BufRead) -> std::io::Result<Option<RequestLine>> {
    let mut line_bytes = Vec::new();
    let (mut read_any, mut oversized) = (false, false);
    loop {
        let chunk = match stdin.fill_buf() {
            Ok(chunk) => chunk,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if chunk.is_empty() {
            // EOF — 읽은 것이 없으면 입력 끝, 있으면 개행 없는 마지막 줄이다.
            if !read_any {
                return Ok(None);
            }
            return end_request_line(line_bytes, oversized, false).map(Some);
        }
        read_any = true;
        let newline_at = chunk.iter().position(|&b| b == b'\n');
        let content = &chunk[..newline_at.unwrap_or(chunk.len())];
        // `\r\n`의 `\r`은 줄 내용이 아니다 — 상한보다 1바이트 더 쌓아 두고 줄 끝에서 판정한다.
        if !oversized && line_bytes.len() + content.len() <= MAX_REQUEST_LINE_BYTES + 1 {
            line_bytes.extend_from_slice(content);
        } else {
            oversized = true;
            line_bytes = Vec::new();
        }
        #[cfg(test)]
        LINE_BUFFER_PEAK.with(|peak| peak.set(peak.get().max(line_bytes.len())));
        let consumed = newline_at.map_or(chunk.len(), |at| at + 1);
        stdin.consume(consumed);
        if newline_at.is_some() {
            return end_request_line(line_bytes, oversized, true).map(Some);
        }
    }
}

/// 다 읽은 줄 하나를 판정한다 — 개행으로 끝난 줄은 끝의 `\r`을 떼고, 그래도 상한을 넘으면 Oversized다.
fn end_request_line(
    mut line_bytes: Vec<u8>,
    oversized: bool,
    newline: bool,
) -> std::io::Result<RequestLine> {
    if newline && line_bytes.last() == Some(&b'\r') {
        line_bytes.pop();
    }
    if oversized || line_bytes.len() > MAX_REQUEST_LINE_BYTES {
        return Ok(RequestLine::Oversized);
    }
    // 예전 `lines()`가 내던 것과 같은 오류다 — 세션은 이 오류에서 끝난다(종료 2).
    String::from_utf8(line_bytes)
        .map(RequestLine::Text)
        .map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "stream did not contain valid UTF-8",
            )
        })
}

/// 상한을 넘은 요청 줄에 대한 답 — 줄을 파싱하지 않았으니 id를 알 수 없어 null이다(JSON-RPC 2.0이
/// Invalid Request에 정한 대로). 서버는 이 답을 쓰고 다음 줄을 계속 읽는다.
fn oversized_request_error() -> Response {
    Response {
        jsonrpc: "2.0",
        id: serde_json::Value::Null,
        result: None,
        error: Some(RpcError {
            code: -32600,
            message: format!(
                "request line exceeds the 1 MiB limit ({MAX_REQUEST_LINE_BYTES} bytes) and was \
                 discarded; send each request as one shorter line"
            ),
        }),
    }
}

/// 응답 하나를 한 줄 JSON으로 쓰고 비운다 — 직렬화 실패는 stderr에 남긴다.
fn write_response(resp: &Response, out: &mut dyn Write, err: &mut dyn Write) {
    match serde_json::to_string(resp) {
        Ok(s) => {
            let _ = writeln!(out, "{s}");
            let _ = out.flush();
        }
        Err(e) => {
            let _ = writeln!(err, "mcp: encoding response: {e}");
        }
    }
}

/// 수확된 문서 하나를 서빙하는 서버 상태.
pub struct Server<'d> {
    doc: &'d Document,
    /// 질의 색인 — 정점 ID 집합·이웃·경로 인접·도달 인접을 처음 쓰는 도구 호출이 한 번
    /// 만들고 이후 호출이 재사용한다. 스냅샷이 불변이라 세션 내내 유효하다 — 호출마다
    /// 다시 만들면 답이 이웃 몇 개여도 호출당 전 간선 비용을 치른다.
    index: analysis::QueryIndex<'d>,
    /// deps 도구가 cargo metadata를 읽을 워크스페이스 디렉터리.
    dir: PathBuf,
    /// 문서를 `--graph` 저장 스냅샷에서 읽었는가 — 그러면 deps 도구는 `dir` 워크스페이스가 스냅샷이
    /// 기록한 워크스페이스와 같을 때만 답한다. 수확한 세션은 문서와 deps가 같은 `dir`을 본다.
    snapshot: bool,
    /// deps 보고서 — 첫 호출 시 한 번 만든다. 매 요청마다 syn 수확을
    /// 새로 하면 Box::leak된 AST가 서버 수명 동안 계속 쌓이고, 시작
    /// 시 무조건 수확하면 deps를 안 쓰는 세션도 수확 비용을 치른다.
    deps_report: std::sync::OnceLock<Result<String, String>>,
    /// 레벨별 cycles 보고서 — 레벨마다 첫 호출 시 한 번 만든다(칸 번호는 Level 선언
    /// 순서). 같은 스냅샷의 같은 레벨은 답이 같다 — 호출마다 투영·SCC를 다시 하면
    /// 문서 크기만큼 든다.
    cycles: [std::sync::OnceLock<Result<String, String>>; 4],
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
    // `--root`는 서빙하는 문서의 보존 루트를 바꾼다(summary·dead 도구가 쓴다).
    cli_args::reject_unused(a, &[cli_args::DOCUMENT_FLAGS, &["config", "root"]], 0)?;
    let doc = cli_args::document_for(a, true)?;
    let dir = PathBuf::from(a.get("dir").unwrap_or("."));
    let cfg_path = match a.get("config") {
        Some(f) => Some(PathBuf::from(f)),
        None => find_config(&dir),
    };
    let mut srv = Server::new(&doc, dir, cfg_path);
    srv.snapshot = a.get("graph").is_some();
    Ok(srv.serve(stdin, out, err))
}

/// 스냅샷 세션의 deps 도구가 스냅샷의 워크스페이스를 알 수 없을 때의 답 — 원인과 해결 방향.
const UNRECORDED_WORKSPACE: &str = "the --graph snapshot does not record the absolute cargo \
     workspace it was harvested from, so rustograph_deps cannot tell whether the --dir workspace is \
     the same one; regenerate the snapshot with `rustograph graph --level symbol --out FILE`, or \
     start the server without --graph";

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

impl<'d> Server<'d> {
    /// 문서 하나를 서빙할 서버 — 색인·보고서 캐시는 비어 있고, 처음 쓰는 도구 호출이 채운다.
    fn new(doc: &'d Document, dir: PathBuf, cfg_path: Option<PathBuf>) -> Self {
        Server {
            doc,
            index: analysis::QueryIndex::new(doc),
            dir,
            snapshot: false,
            deps_report: std::sync::OnceLock::new(),
            cycles: Default::default(),
            cfg_path,
            protocol: "2024-11-05".to_string(),
        }
    }

    /// EOF까지 한 줄씩 JSON-RPC를 처리한다. [`MAX_REQUEST_LINE_BYTES`]를 넘는 줄은 쌓지 않고
    /// 오류로 답한 뒤 다음 줄로 넘어간다.
    fn serve(&self, stdin: &mut dyn BufRead, out: &mut dyn Write, err: &mut dyn Write) -> i32 {
        let mut protocol = self.protocol.clone();
        loop {
            let line = match read_request_line(stdin) {
                Ok(Some(RequestLine::Text(l))) => l,
                Ok(Some(RequestLine::Oversized)) => {
                    write_response(&oversized_request_error(), out, err);
                    continue;
                }
                Ok(None) => return 0,
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
            write_response(&resp, out, err);
        }
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
        if matches!(
            name,
            "rustograph_query"
                | "rustograph_impact"
                | "rustograph_paths"
                | "rustograph_dead"
                | "rustograph_rules"
        ) {
            self.doc.require_level(Level::Symbol)?;
        }
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
                let id = self.require_id(id)?;
                // depth 0은 CLI `--depth 0`과 같은 해석 함수로 "깊이 제한 없음"이다 — 보고에는 요청값을 싣는다.
                let depth = arg_usize(args, "depth")?.unwrap_or(1);
                let max = arg_usize(args, "max")?.unwrap_or(200);
                let mut r = self.index.query(&id, cli_args::walk_depth(depth), max);
                r.depth = depth;
                Ok(export::to_json(&r))
            }
            "rustograph_impact" => {
                let id = arg_str(args, "id").ok_or("impact needs an \"id\" argument")?;
                let id = self.require_id(id)?;
                // 기본 깊이는 0(깊이 제한 없음)이다 — query와 같은 해석 함수를 지나고 보고에는 요청값을 싣는다.
                let depth = arg_usize(args, "depth")?.unwrap_or(0);
                let max = arg_usize(args, "max")?.unwrap_or(200);
                let mut r = self.index.impact(&id, cli_args::walk_depth(depth), max);
                r.depth = depth;
                Ok(export::to_json(&r))
            }
            "rustograph_paths" => {
                let from = arg_str(args, "from").ok_or("paths needs \"from\" and \"to\"")?;
                let to = arg_str(args, "to").ok_or("paths needs \"from\" and \"to\"")?;
                let from = self.require_id(from)?;
                let to = self.require_id(to)?;
                let max = arg_usize(args, "max")?.unwrap_or(10);
                let budget = arg_usize(args, "budget")?.unwrap_or(50_000);
                Ok(export::to_json(&self.index.paths(&from, &to, max, budget)))
            }
            "rustograph_search" => {
                let q = arg_str(args, "q").ok_or("search needs a \"q\" argument")?;
                let max = arg_usize(args, "max")?.unwrap_or(20);
                Ok(export::to_json(&analysis::search(self.doc, q, max)))
            }
            // 첫 호출에 한 번 만들어 메모한다(오류도 세션 내내 같다).
            "rustograph_deps" => self.deps_report.get_or_init(|| self.deps_json()).clone(),
            "rustograph_cycles" => {
                let level = match arg_str(args, "level") {
                    Some(l) => Level::parse(l)
                        .ok_or_else(|| format!("unknown level {l} (crate|module|type|symbol)"))?,
                    None => self.doc.level,
                };
                self.cycles_report(level)
            }
            "rustograph_dead" => {
                // 문서 루트에 retainPublic·추가 루트를 얹은 루트 목록으로 판정한다 —
                // 도구 호출이 서빙하는 스냅샷 자체를 흔들면 안 되고, 루트만 바꾼
                // 문서 사본도 만들지 않는다.
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
                        if self.index.has_vertex(r) {
                            roots.insert(r.to_string());
                        } else {
                            unknown_roots.push(r.to_string());
                        }
                    }
                }
                let roots: Vec<String> = roots.into_iter().collect();
                let rep = self.index.dead(&roots)?;
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
                // 설정은 호출마다 다시 읽는다(세션 중 바뀔 수 있다) — 명시 --config여도 정규
                // 파일만 연다. 한 번만 읽히는 파이프는 두 번째 호출에서 멈추거나 빈 설정을 읽는다.
                let cfg = config::load_file(cfg_path, config::FileKinds::RegularOnly)?;
                let mut rep = rules::check(self.doc, &cfg);
                // 설정에 선언된 기준선을 적용한다 — CLI rules와 같은 계약.
                // NotFound만 도입 전으로 본다 — 권한 오류나 깨진 파일을
                // 조용히 넘기면 억제가 안 먹힌 채로 지나간다. 장치·FIFO는
                // 열지 않고 상한까지만 읽는다 — 서버가 멈추면 안 된다.
                if let Some(b) = &cfg.baseline {
                    let path = cfg_path.parent().unwrap_or(self.dir.as_path()).join(b);
                    match config::read_bounded_text(
                        &path,
                        config::MAX_FILE_BYTES,
                        config::FileKinds::RegularOnly,
                    ) {
                        Ok(src) => rules::apply_baseline(&mut rep, &rules::Baseline::parse(&src)),
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => {
                            return Err(format!("cannot read baseline {}: {e}", path.display()))
                        }
                    }
                }
                Ok(export::to_json(&rep))
            }
            other => Err(format!("unknown tool {other:?} — see tools/list")),
        }
    }

    /// 정점 ID를 정확히 해석한다 — 부분 문자열을 조용히 맞추지 않고
    /// 후보를 담은 isError로 돌려준다. 에이전트는 rustograph_search로
    /// 후보를 찾은 뒤 정확한 ID로 다시 호출해야 한다.
    fn require_id(&self, id: &str) -> Result<String, String> {
        self.index.resolve_id(id).map_err(|cands| {
            if cands.is_empty() {
                format!("vertex {id} not found")
            } else {
                format!("vertex {id} not found — candidates: {}", cands.join(", "))
            }
        })
    }

    /// deps 보고서(JSON) — `dir`을 자체 syn 수확으로 만든다. 서빙하는 문서는 외부 정점이 없거나
    /// 필터됐을 수 있어 사용 증거로 못 쓴다. 스냅샷 세션에서는 `dir` 워크스페이스가 스냅샷이 기록한
    /// `workspace`(cargo가 준 절대 경로)와 같은 디렉터리일 때만 답한다 — 다르면 같은 세션의 summary·
    /// query와 다른 저장소를 말하는 답이 되는데 에이전트는 구분할 수 없다. 기록이 없거나 상대 경로면
    /// 같음을 확인할 수 없어 오류다. 스냅샷이 고른 경로에서는 cargo를 실행하지 않는다.
    fn deps_json(&self) -> Result<String, String> {
        if !self.snapshot {
            return deps::report(&self.dir).map(|r| export::to_json(&r));
        }
        let workspace = self
            .doc
            .workspace
            .as_deref()
            .map(Path::new)
            .filter(|w| w.is_absolute())
            .ok_or(UNRECORDED_WORKSPACE)?;
        match deps::report_in_workspace(&self.dir, workspace)? {
            Ok(report) => Ok(export::to_json(&report)),
            Err(dir_workspace) => Err(format!(
                "rustograph_deps harvests the --dir workspace {}, which is not the workspace the \
                 --graph snapshot was harvested from (rustograph_summary shows it); start the \
                 server with --dir set to the snapshot's workspace, or without --graph",
                dir_workspace.display()
            )),
        }
    }

    /// 레벨 하나의 cycles 보고서(JSON) — 첫 호출에 만들어 두고 같은 레벨의 다음 호출은
    /// 그대로 돌려준다. 문서 레벨이면 투영할 것이 없다 — view()는 같은 레벨에서 문서를
    /// 통째로 복제한다.
    fn cycles_report(&self, level: Level) -> Result<String, String> {
        // 칸 번호는 Level 선언 순서다(crate·module·type·symbol).
        self.cycles[level as usize]
            .get_or_init(|| {
                let rep = if level == self.doc.level {
                    analysis::cycles(self.doc)
                } else {
                    analysis::cycles(&self.doc.view(level)?)
                };
                Ok(export::to_json(&rep))
            })
            .clone()
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
                    "depth": {"type": "integer", "description": "neighbor depth (default 1); 0 = no depth limit, bounded by max"},
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
                    "depth": {"type": "integer", "description": "max depth (default 0); 0 = no depth limit, the full closure bounded by max"},
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
            "name": "rustograph_paths",
            "description": "Shortest-first dependency paths between two vertices (bounded BFS)",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "from": {"type": "string", "description": "start vertex ID"},
                    "to": {"type": "string", "description": "goal vertex ID"},
                    "max": {"type": "integer", "description": "max paths (default 10)"},
                    "budget": {"type": "integer", "description": "max vertex expansions (default 50000)"},
                },
                "required": ["from", "to"],
            },
        },
        {
            "name": "rustograph_search",
            "description": "Find vertices by ID — exact first, then suffix, then substring. Use before query/impact when unsure of an ID",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "q": {"type": "string", "description": "query string"},
                    "max": {"type": "integer", "description": "max hits (default 20)"},
                },
                "required": ["q"],
            },
        },
        {
            "name": "rustograph_deps",
            "description": "Cargo dependency report: unused declared deps and duplicate versions, from the tool's own harvest of the --dir workspace (with --graph, only when that is the workspace the snapshot was harvested from)",
            "inputSchema": {"type": "object", "properties": {}},
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

/// 정수 인자 하나를 꺼낸다 — 없으면 None, 있으면 반드시 부호 없는
/// 정수여야 한다. 문자열이나 음수를 조용히 버리면 호출자가 준 한계가
/// 무시된다 — 잘못된 값은 도구 오류다.
fn arg_usize(args: &serde_json::Value, key: &str) -> Result<Option<usize>, String> {
    match args.get(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(v) => {
            let n = v
                .as_u64()
                .ok_or_else(|| format!("invalid {key} {v} — expected a non-negative integer"))?;
            // 32비트에서 u64→usize `as` 캐스트는 값을 깎는다 — 한계
            // 인자가 조용히 0이 되면 안 된다.
            usize::try_from(n)
                .map(Some)
                .map_err(|_| format!("invalid {key} {v} — value exceeds usize range"))
        }
    }
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
    fn test_doc() -> Document {
        document(
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
        )
    }

    /// 문서 하나를 서빙하는 시험 서버.
    fn server_for(doc: &Document) -> Server<'_> {
        Server::new(doc, PathBuf::from("."), None)
    }

    /// NDJSON 요청을 밀어 넣고 응답 줄들을 JSON 값으로 돌려준다.
    fn session(srv: &Server<'_>, lines: &[&str]) -> Vec<serde_json::Value> {
        let input = lines.join("\n") + "\n";
        let mut in_: &[u8] = input.as_bytes();
        served(srv, &mut in_)
    }

    /// 입력 스트림 하나를 EOF까지 서빙하고 응답 줄들을 JSON 값으로 돌려준다 — 종료 코드는 0이어야 한다.
    fn served(srv: &Server<'_>, input: &mut dyn BufRead) -> Vec<serde_json::Value> {
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = srv.serve(input, &mut out, &mut err);
        assert_eq!(code, 0, "stderr: {}", String::from_utf8_lossy(&err));
        String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    /// 요청 줄 상한(1 MiB) — 시험이 상수를 거치지 않고 값을 고정한다.
    const LINE_LIMIT: usize = 1024 * 1024;

    /// 줄 내용이 정확히 `len`바이트인 ping 요청 — `pad` 문자열 길이로 맞춘다.
    fn ping_of_len(id: u32, len: usize) -> String {
        let bare = format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"ping","pad":""}}"#);
        let pad = "x".repeat(len - bare.len());
        format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"ping","pad":"{pad}"}}"#)
    }

    /// 상한을 넘은 줄의 답 — id를 알 수 없어 null인 -32600(Invalid Request)이고 원인·해결이 담긴다.
    fn assert_oversized_answer(r: &serde_json::Value) {
        assert_eq!(r["id"], serde_json::Value::Null, "{r}");
        assert_eq!(r["error"]["code"], -32600, "{r}");
        let message = r["error"]["message"].as_str().unwrap();
        assert!(message.contains("1 MiB"), "{message}");
        assert_eq!(r.get("result"), None, "{r}");
    }

    /// 1 MiB를 넘는 요청 줄은 처리하지 않는다 — -32600 오류 하나로 답하고 그 줄을 다음 개행까지
    /// 버린 뒤 다음 요청을 정상으로 처리한다. 개행 없이 EOF로 끝나는 거대 줄도 같다. 예전 서버는
    /// 줄 길이 상한 없이 `lines()`로 읽어 3 MiB 줄도 그대로 처리했다 — 개행 없는 입력 하나가
    /// 메모리를 끝없이 쌓을 수 있었다.
    #[test]
    fn request_lines_over_one_mebibyte_are_refused_and_skipped() {
        let doc = test_doc();
        let srv = server_for(&doc);
        let huge = ping_of_len(1, 3 * LINE_LIMIT);
        let next = r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#;
        let res = session(&srv, &[&huge, next]);
        assert_eq!(res.len(), 2, "{res:?}");
        assert_oversized_answer(&res[0]);
        assert_eq!(res[1]["id"], 2);
        assert_eq!(res[1]["result"], serde_json::json!({}));
        // 개행 없이 끝나는 마지막 줄 — 답한 뒤 정상 종료(0)한다.
        let input = format!("{next}\n{huge}");
        let res = served(&srv, &mut input.as_bytes());
        assert_eq!(res.len(), 2, "{res:?}");
        assert_eq!(res[0]["id"], 2);
        assert_oversized_answer(&res[1]);
    }

    /// 거대 줄은 버퍼에 쌓이지 않는다 — 개행 없는 64 MiB 입력을 보내도 줄 버퍼가 담는 내용은
    /// 상한(+ `\r` 한 바이트)을 넘지 않고, 그 뒤 줄은 정상으로 처리된다.
    #[test]
    fn oversized_request_lines_never_hold_more_than_the_limit() {
        use std::io::Read;
        let doc = test_doc();
        let srv = server_for(&doc);
        let flood = std::io::repeat(b'x').take(64 * LINE_LIMIT as u64);
        let rest: &[u8] = b"\n{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n";
        let mut reader = std::io::BufReader::new(flood.chain(rest));
        LINE_BUFFER_PEAK.with(|peak| peak.set(0));
        let res = served(&srv, &mut reader);
        let peak = LINE_BUFFER_PEAK.with(std::cell::Cell::get);
        assert!(peak <= LINE_LIMIT + 1, "line buffer held {peak} bytes");
        assert_eq!(res.len(), 2, "{res:?}");
        assert_oversized_answer(&res[0]);
        assert_eq!(res[1]["id"], 2);
        assert_eq!(MAX_REQUEST_LINE_BYTES, LINE_LIMIT);
    }

    /// 상한은 줄 끝(`\n`·`\r\n`)을 뺀 내용에 적용된다 — 정확히 1 MiB인 줄은 `\n`이든 `\r\n`이든
    /// 처리하고, 1바이트 더 긴 줄만 거부한다. 읽기 덩어리 경계(작은 버퍼)에 걸쳐도 같다.
    #[test]
    fn request_line_limit_counts_content_without_the_line_ending() {
        let doc = test_doc();
        let srv = server_for(&doc);
        let input = format!(
            "{}\n{}\r\n{}\n{}\r\n",
            ping_of_len(1, LINE_LIMIT),
            ping_of_len(2, LINE_LIMIT),
            ping_of_len(3, LINE_LIMIT + 1),
            ping_of_len(4, LINE_LIMIT + 1),
        );
        for capacity in [1, 7, 8192, 3 * LINE_LIMIT] {
            let mut reader = std::io::BufReader::with_capacity(capacity, input.as_bytes());
            let res = served(&srv, &mut reader);
            assert_eq!(res.len(), 4, "capacity {capacity}: {res:?}");
            assert_eq!(res[0]["id"], 1, "capacity {capacity}");
            assert_eq!(res[0]["result"], serde_json::json!({}));
            assert_eq!(res[1]["id"], 2, "capacity {capacity}");
            assert_eq!(res[1]["result"], serde_json::json!({}));
            assert_oversized_answer(&res[2]);
            assert_oversized_answer(&res[3]);
        }
    }

    /// UTF-8이 아닌 줄은 예전 `lines()`처럼 읽기 오류다 — 앞 요청의 답은 남기고 같은 문구로 종료 2.
    #[test]
    fn request_lines_that_are_not_utf8_end_the_session_as_before() {
        let doc = test_doc();
        let srv = server_for(&doc);
        let input: &[u8] = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\n\xff\xfe\n\
                             {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n";
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = srv.serve(&mut &input[..], &mut out, &mut err);
        assert_eq!(code, 2);
        assert_eq!(out, b"{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{}}\n");
        assert_eq!(
            String::from_utf8(err).unwrap(),
            "mcp: reading stdin: stream did not contain valid UTF-8\n"
        );
    }

    /// 줄 끝 처리는 예전 `lines()`와 같다 — `\r\n`·빈 줄·공백 줄을 그대로 다루고, 개행 없는 마지막
    /// 줄도 한 요청이다. 작은 읽기 버퍼로 `\r`과 `\n`이 다른 덩어리에 걸려도 같다.
    #[test]
    fn request_line_endings_match_buffered_lines() {
        let doc = test_doc();
        let srv = server_for(&doc);
        let input = "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"ping\"}\r\n\n  \r\n\r\n\
                     {\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"ping\"}\n\
                     {\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"ping\"}";
        for capacity in [1, 2, 3, 64] {
            let mut reader = std::io::BufReader::with_capacity(capacity, input.as_bytes());
            let ids: Vec<_> = served(&srv, &mut reader)
                .iter()
                .map(|r| r["id"].clone())
                .collect();
            assert_eq!(ids, [1, 2, 3], "capacity {capacity}");
        }
    }

    #[test]
    fn handshake_and_tools_list() {
        let res = session(
            &server_for(&test_doc()),
            &[
                r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            ],
        );
        // 알림은 응답을 만들지 않는다.
        assert_eq!(res.len(), 2);
        assert_eq!(res[0]["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(res[1]["result"]["tools"].as_array().unwrap().len(), 9);
    }

    #[test]
    fn tool_call_answers_over_the_graph() {
        let res = session(
            &server_for(&test_doc()),
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
            &server_for(&test_doc()),
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

    /// rules 도구 호출 하나를 보내고 결과 객체를 돌려준다.
    fn call_rules(srv: &Server<'_>) -> serde_json::Value {
        let call = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"rustograph_rules","arguments":{}}}"#;
        session(srv, &[call]).remove(0)["result"].take()
    }

    /// rules 도구도 CLI와 같은 config::load_file로 설정을 읽는다 — 올바른 설정은
    /// 보고서, YAML 형식 오류는 경로를 담은 isError다.
    #[test]
    fn rules_tool_reads_config_and_reports_bad_yaml() {
        let dir = std::env::temp_dir().join(format!("rg-mcp-rules-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("rules.yml");
        std::fs::write(&good, "components:\n  all: [\"c/**\"]\ndeps:\n  all: []\n").unwrap();
        let bad = dir.join("bad.yml");
        std::fs::write(&bad, "components: [unclosed\n").unwrap();
        let doc = test_doc();
        let mut srv = server_for(&doc);
        srv.cfg_path = Some(good);
        let result = call_rules(&srv);
        assert_eq!(result["isError"], serde_json::Value::Null, "{result}");
        assert!(result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("\"violations\""));
        srv.cfg_path = Some(bad.clone());
        let result = call_rules(&srv);
        assert_eq!(result["isError"], true);
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            text.starts_with(&format!("bad {}: ", bad.display())),
            "{text}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    /// rules 도구도 설정·기준선을 상한 읽기로 연다 — 장치를 가리키는 baseline과 FIFO
    /// 설정 파일은 서버를 멈추거나 메모리를 다 쓰지 않고 isError로 답한다.
    #[cfg(unix)]
    #[test]
    fn rules_tool_refuses_special_config_and_baseline_files() {
        let dir = std::env::temp_dir().join(format!("rg-mcp-special-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let fifo = dir.join("pipe.yml");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo must run");
        assert!(made.success());
        let device_baseline = dir.join("rules.yml");
        std::fs::write(
            &device_baseline,
            "components:\n  all: [\"c/**\"]\nbaseline: /dev/zero\n",
        )
        .unwrap();
        let doc = test_doc();
        let mut srv = server_for(&doc);
        for (cfg, cause) in [
            (
                device_baseline,
                "cannot read baseline /dev/zero: ".to_string(),
            ),
            (fifo.clone(), format!("cannot read {}: ", fifo.display())),
        ] {
            srv.cfg_path = Some(cfg);
            let result = call_rules(&srv);
            assert_eq!(result["isError"], true, "{result}");
            let text = result["content"][0]["text"].as_str().unwrap();
            assert!(
                text.starts_with(&cause) && text.contains("not a regular file"),
                "{text}"
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 정점 30개 사슬에 갈래·순환·추정 간선·공개 정점을 섞은 문서 — 도구마다 비지 않은 답이 있다.
    fn branching_doc() -> Document {
        let id = |i: usize| format!("c::f{i:02}");
        let vertices = (0..30).map(|i| v(&id(i), Kind::Fn, i % 7 == 0)).collect();
        let mut edges: Vec<Edge> = (0..29)
            .map(|i| Edge::new(id(i), id(i + 1), EdgeKind::Call))
            .collect();
        edges.extend(
            (0..25)
                .step_by(5)
                .map(|i| Edge::new(id(i), id(i + 3), EdgeKind::References)),
        );
        edges.push(Edge::new(id(12), id(4), EdgeKind::Call));
        edges.push(Edge::maybe(id(20), id(27), EdgeKind::Call));
        edges.push(Edge::new(id(3), id(8), EdgeKind::Contains));
        // f20 다음 사슬은 루트에서 끊긴다 — dead가 보고할 정점이 생긴다.
        edges.retain(|e| !(e.from == id(19) && e.to == id(20)));
        document(
            Level::Symbol,
            ".".into(),
            None,
            vec![id(0)],
            vertices,
            edges,
            vec![],
        )
    }

    /// tools/call 요청 한 줄.
    fn call_line(n: usize, name: &str, arguments: serde_json::Value) -> String {
        serde_json::json!({
            "jsonrpc": "2.0", "id": n, "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        })
        .to_string()
    }

    /// 같은 세션에서 query·impact·paths·dead·cycles를 인자를 바꿔 가며 세 번씩 부르는 요청과,
    /// 각 요청에 기대하는 답(CLI가 내는 분석 결과 JSON) — 서버 색인을 거치지 않고 만든다.
    fn repeated_calls(doc: &Document) -> (Vec<String>, Vec<String>) {
        let mut lines = Vec::new();
        let mut expected = Vec::new();
        let mut push = |name: &str, arguments: serde_json::Value, answer: String| {
            lines.push(call_line(lines.len() + 1, name, arguments));
            expected.push(answer);
        };
        for (id, depth) in [("c::f05", 1), ("c::f12", 3), ("c::f27", 2)] {
            let args = serde_json::json!({"id": id, "depth": depth});
            push(
                "rustograph_query",
                args,
                export::to_json(&analysis::query(doc, id, depth, 200)),
            );
        }
        for (id, depth, max) in [("c::f08", 0, 200), ("c::f04", 2, 3), ("c::f29", 0, 1)] {
            let mut r = analysis::impact(doc, id, if depth == 0 { usize::MAX } else { depth }, max);
            r.depth = depth;
            let args = serde_json::json!({"id": id, "depth": depth, "max": max});
            push("rustograph_impact", args, export::to_json(&r));
        }
        for (from, to, max) in [
            ("c::f00", "c::f13", 10),
            ("c::f02", "c::f09", 2),
            ("c::f20", "c::f00", 10),
        ] {
            let args = serde_json::json!({"from": from, "to": to, "max": max});
            push(
                "rustograph_paths",
                args,
                export::to_json(&analysis::paths(doc, from, to, max, 50_000)),
            );
        }
        let dead_answer = |roots: Vec<String>, unknown: &[&str]| {
            let mut d = doc.clone();
            d.roots = roots;
            let mut out = serde_json::to_value(analysis::dead(&d).unwrap()).unwrap();
            if !unknown.is_empty() {
                out["unknownRoots"] = serde_json::json!(unknown);
            }
            export::to_json(&out)
        };
        let public: std::collections::BTreeSet<String> = doc
            .vertices
            .iter()
            .filter(|v| v.exported)
            .map(|v| v.id.clone())
            .chain(doc.roots.iter().cloned())
            .collect();
        push(
            "rustograph_dead",
            serde_json::json!({}),
            dead_answer(doc.roots.clone(), &[]),
        );
        push(
            "rustograph_dead",
            serde_json::json!({"retainPublic": true}),
            dead_answer(public.into_iter().collect(), &[]),
        );
        push(
            "rustograph_dead",
            serde_json::json!({"roots": ["c::f22", "c::nope"]}),
            dead_answer(vec!["c::f00".into(), "c::f22".into()], &["c::nope"]),
        );
        for level in [None, Some("module"), None] {
            let args = level.map_or(serde_json::json!({}), |l| serde_json::json!({"level": l}));
            let view = doc
                .view(level.and_then(Level::parse).unwrap_or(doc.level))
                .unwrap();
            push(
                "rustograph_cycles",
                args,
                export::to_json(&analysis::cycles(&view)),
            );
        }
        (lines, expected)
    }

    /// 한 세션의 도구 호출은 불변 스냅샷 하나를 묻는다 — 호출마다 정점 ID 집합·이웃 색인·
    /// 경로 인접·도달 인접을 다시 만들거나 문서를 복제하면 답이 이웃 몇 개여도 호출당
    /// O(E log V)다(정점 8만·간선 40만 합성 문서에서 query 호출당 0.59초). 같은 레벨의
    /// cycles도 다시 계산할 이유가 없다. 색인은 종류마다 한 번, cycles는 레벨마다 한 번이다.
    #[test]
    fn repeated_calls_build_each_index_once() {
        let doc = branching_doc();
        let (lines, _) = repeated_calls(&doc);
        let srv = server_for(&doc);
        let firm = |d: &Document| {
            d.edges
                .iter()
                .filter(|e| e.kind.is_dependency() && !e.tentative)
                .count()
        };
        let cycle_bound = firm(&doc) + firm(&doc.view(Level::Module).unwrap());
        let (indexed, classified) = (analysis::indexed(), analysis::classified_edges());
        let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
        let res = session(&srv, &lines);
        let indexed = analysis::indexed() - indexed;
        let classified = analysis::classified_edges() - classified;
        assert_eq!(res.len(), lines.len());
        // 정점 ID 집합 한 번 + 간선 색인 넷(양방향 이웃·역방향 이웃·경로 인접·도달 인접) 한 번씩.
        let bound = doc.vertices.len() + 4 * doc.edges.len();
        assert!(indexed <= bound, "{indexed} indexing visits, bound {bound}");
        assert!(
            classified <= cycle_bound,
            "{classified} cycle edge visits, bound {cycle_bound}"
        );
    }

    /// 색인·보고서를 재사용해도 답은 호출마다 CLI가 같은 인자로 내는 분석 결과와 같다 —
    /// 앞 호출의 상태가 다음 호출에 새지 않는다.
    #[test]
    fn repeated_calls_match_one_shot_analysis() {
        let doc = branching_doc();
        let (lines, expected) = repeated_calls(&doc);
        let srv = server_for(&doc);
        let lines: Vec<&str> = lines.iter().map(String::as_str).collect();
        let res = session(&srv, &lines);
        assert_eq!(res.len(), expected.len());
        for (i, (r, want)) in res.iter().zip(&expected).enumerate() {
            assert_eq!(
                r["result"]["isError"],
                serde_json::Value::Null,
                "call {i}: {r}"
            );
            let text = r["result"]["content"][0]["text"].as_str().unwrap();
            assert_eq!(text, want, "call {i}: {}", lines[i]);
        }
        // 같은 줄을 한 번 더 보내도 같은 답이다.
        assert_eq!(session(&srv, &lines), res);
    }

    /// `rustograph mcp`도 쓰지 않는 플래그·위치 인자를 수확 전에 거부한다(CLI와 같은 공용 검사) —
    /// `--root`(서빙하는 문서의 보존 루트)와 공유 플래그는 받는다.
    #[test]
    fn mcp_command_rejects_flags_it_does_not_use() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture");
        let dir = fixture.to_str().unwrap();
        let start = |argv: &[&str]| {
            let argv: Vec<String> = argv.iter().map(|s| s.to_string()).collect();
            let parsed = cli_args::parse(&argv).unwrap();
            let mut stdin: &[u8] = b"";
            cmd(&parsed, &mut stdin, &mut Vec::new(), &mut Vec::new())
        };
        for (argv, unused) in [
            (&["mcp", "--out", "x.json", "--dir", dir][..], "--out"),
            (&["mcp", "--level", "module", "--dir", dir][..], "--level"),
            (&["mcp", "--strict", "--dir", dir][..], "--strict"),
            (&["mcp", "stray", "--dir", dir][..], "\"stray\""),
        ] {
            let e = start(argv).unwrap_err();
            assert!(e.starts_with("mcp does not") && e.contains(unused), "{e}");
        }
        let kept = ["mcp", "--root", "fixture_core::entry", "--retain-public"];
        assert_eq!(start(&[&kept[..], &["--dir", dir]].concat()), Ok(0));
    }

    #[test]
    fn unknown_method_and_parse_error() {
        let res = session(
            &server_for(&test_doc()),
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
            &server_for(&test_doc()),
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
