//! 클라이언트 오라클 — fixture 시나리오를 진짜 reqwest 0.13.5·ureq 3.4.2·url 2.5.8로
//! 실행해 127.0.0.1 임시 포트의 기록 서버(HTTP 프록시로 지정)가 받은 method·경로를
//! rustograph `routes --role client` 사실과 대조한다.
//!
//! 판정: 시나리오의 요청마다 그 시나리오에 귀속된 정적 사실 중 method(또는
//! `methodDynamic`)와 경로(root는 전체, base는 세그먼트 경계 꼬리, `{}`는 비어 있지
//! 않은 세그먼트)와 authority가 맞는 것이 있으면 match, 정적 사실 없이 dynamic
//! 사실만 있으면 dynamic, 요청이 없으면(보내기 전 실패) 사실도 없어야 한다. 정적
//! 사실은 모두 어떤 요청과 맞아야 한다(정밀도). 외부 네트워크를 쓰지 않는다 —
//! 모든 http 요청이 프록시로 지정한 로컬 서버로 간다.
//!
//! 사용: oracle-client <routes-client.json> <recorded.json>

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

/// 서버가 받은 요청 하나.
#[derive(Clone, Debug)]
struct Seen {
    method: String,
    target: String,
    host: String,
}

type Log = Arc<Mutex<Vec<Seen>>>;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, facts_path, out_path] = args.as_slice() else {
        eprintln!("usage: oracle-client <routes-client.json> <recorded.json>");
        std::process::exit(2);
    };
    let doc: Value = serde_json::from_str(
        &std::fs::read_to_string(facts_path).expect("read the routes document"),
    )
    .expect("routes document is JSON");
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind 127.0.0.1");
    let port = listener.local_addr().expect("local addr").port();
    let server_log = log.clone();
    std::thread::spawn(move || serve(listener, server_log));
    // 모든 http 요청을 기록 서버로 보낸다(reqwest·ureq 모두 환경 프록시를 읽는다).
    let proxy = format!("http://127.0.0.1:{port}");
    for key in ["HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy"] {
        std::env::set_var(key, &proxy);
    }
    for key in ["NO_PROXY", "no_proxy"] {
        std::env::remove_var(key);
    }
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    let mut rows = Vec::new();
    let mut claimed = std::collections::BTreeSet::new();
    let mut failed = 0usize;
    for sc in scenarios() {
        log.lock().unwrap().clear();
        let error = match &sc.run {
            Run::Blocking(f) => f().err(),
            Run::Async(f) => rt.block_on(f()).err(),
        };
        // 서버 스레드가 기록을 마칠 때까지 잠깐 기다린다(응답 뒤 기록하지 않으므로 짧다).
        std::thread::sleep(std::time::Duration::from_millis(30));
        let seen = log.lock().unwrap().clone();
        let facts = facts_for(&doc, &sc.usrs);
        for f in &facts {
            claimed.insert(f.to_string());
        }
        let (result, requests) = judge(&seen, &facts);
        if result == "mismatch" {
            failed += 1;
        }
        rows.push(json!({
            "scenario": sc.name,
            "requests": requests,
            "facts": facts,
            "result": result,
            "error": error.is_some(),
        }));
    }
    let unclaimed: Vec<Value> = doc["facts"]
        .as_array()
        .into_iter()
        .flatten()
        .map(summary)
        .filter(|f| !claimed.contains(&f.to_string()))
        .collect();
    let count = |r: &str| rows.iter().filter(|x| x["result"] == r).count();
    let record = json!({
        "versions": {"reqwest": "0.13.5", "ureq": "3.4.2", "url": "2.5.8"},
        "totals": {
            "scenarios": rows.len(),
            "match": count("match"),
            "dynamic": count("dynamic"),
            "noRequest": count("no-request"),
            "mismatch": count("mismatch"),
        },
        "scenarios": rows,
        "unclaimedFacts": unclaimed,
    });
    let text = serde_json::to_string_pretty(&record).expect("serialize") + "\n";
    std::fs::write(out_path, text).expect("write the record");
    println!(
        "client oracle: {} scenarios — match {}, dynamic {}, no-request {}, mismatch {}, unclaimed facts {}",
        record["totals"]["scenarios"],
        count("match"),
        count("dynamic"),
        count("no-request"),
        failed,
        record["unclaimedFacts"].as_array().map_or(0, Vec::len)
    );
    if failed > 0 || !record["unclaimedFacts"].as_array().is_some_and(Vec::is_empty) {
        std::process::exit(1);
    }
}

// ── 기록 서버 ────────────────────────────────────────────────

/// 연결마다 요청 하나를 읽고 200으로 닫는다. ureq 3은 http 요청도 프록시에 CONNECT
/// 터널을 연다 — 터널을 수락하고 그 안의 평문 요청을 읽는다.
fn serve(listener: TcpListener, log: Log) {
    for stream in listener.incoming().flatten() {
        let log = log.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(&stream);
            let Some(mut seen) = read_request(&mut reader) else {
                return;
            };
            if seen.method == "CONNECT" {
                let mut w = &stream;
                if w.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").is_err() {
                    return;
                }
                let Some(inner) = read_request(&mut reader) else {
                    return;
                };
                seen = inner;
            }
            log.lock().unwrap().push(seen);
            let mut w = &stream;
            let _ = w.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
        });
    }
}

/// 요청 줄·헤더·본문을 읽는다.
fn read_request(reader: &mut BufReader<&TcpStream>) -> Option<Seen> {
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let mut host = String::new();
    let mut length = 0usize;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).ok()? == 0 || h == "\r\n" || h == "\n" {
            break;
        }
        let (k, v) = h.split_once(':')?;
        match k.trim().to_ascii_lowercase().as_str() {
            "host" => host = v.trim().to_ascii_lowercase(),
            "content-length" => length = v.trim().parse().unwrap_or(0),
            _ => {}
        }
    }
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).ok()?;
    Some(Seen {
        method,
        target,
        host,
    })
}

// ── 시나리오 ─────────────────────────────────────────────────

type BlockingFn = Box<dyn Fn() -> Result<(), String>>;
type AsyncFn = Box<dyn Fn() -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>>>>>;

enum Run {
    Blocking(BlockingFn),
    Async(AsyncFn),
}

/// 시나리오 하나 — 요청을 보내는 함수와 그 요청을 설명해야 하는 사실의 usr들.
struct Scenario {
    name: &'static str,
    usrs: Vec<String>,
    run: Run,
}

fn b<F>(name: &'static str, usrs: &[&str], f: F) -> Scenario
where
    F: Fn() -> client_app::Res + 'static,
{
    Scenario {
        name,
        usrs: usrs.iter().map(|u| format!("client_app::{u}")).collect(),
        run: Run::Blocking(Box::new(move || f().map_err(|e| e.to_string()))),
    }
}

fn scenarios() -> Vec<Scenario> {
    use client_app::{scenarios as s, ureq_calls as u, wrappers as w};
    let base = "http://api.example.com/prefix";
    vec![
        b("literal_query", &["scenarios::literal_query"], s::literal_query),
        Scenario {
            name: "async_user",
            usrs: vec!["client_app::scenarios::async_user".into()],
            run: Run::Async(Box::new(|| {
                Box::pin(async { s::async_user(7).await.map_err(|e| e.to_string()) })
            })),
        },
        b("concat_post", &["scenarios::concat_post"], s::concat_post),
        b("put_positional", &["scenarios::put_positional"], || s::put_positional(5)),
        b("delete_plus", &["scenarios::delete_plus"], || s::delete_plus("9")),
        b("patch_local", &["scenarios::patch_local"], s::patch_local),
        b("head_health", &["scenarios::head_health"], s::head_health),
        b("request_options", &["scenarios::request_options"], s::request_options),
        b("request_dynamic", &["scenarios::request_dynamic"], || {
            s::request_dynamic(reqwest::Method::PATCH)
        }),
        b("join_relative", &["scenarios::join_relative"], || s::join_relative(5)),
        b("join_replaces_last", &["scenarios::join_replaces_last"], s::join_replaces_last),
        b("join_absolute_path", &["scenarios::join_absolute_path"], s::join_absolute_path),
        b("dot_segments", &["scenarios::dot_segments"], s::dot_segments),
        b("double_slash", &["scenarios::double_slash"], s::double_slash),
        b("query_tail", &["scenarios::query_tail"], || s::query_tail(Some(2))),
        b("query_tail_empty", &["scenarios::query_tail"], || s::query_tail(None)),
        b("partial_segment", &["scenarios::partial_segment"], || s::partial_segment("report")),
        b("unknown_base_rooted", &["scenarios::unknown_base_rooted"], move || {
            s::unknown_base_rooted(base)
        }),
        b("unknown_base_glued", &["scenarios::unknown_base_glued"], move || {
            s::unknown_base_glued(base)
        }),
        b("relative_url", &["scenarios::relative_url"], s::relative_url),
        b("masked_token", &["scenarios::masked_token"], s::masked_token),
        b("non_ascii", &["scenarios::non_ascii"], s::non_ascii),
        b("userinfo_fragment", &["scenarios::userinfo_fragment"], s::userinfo_fragment),
        b("with_port", &["scenarios::with_port"], s::with_port),
        b("backslashes", &["scenarios::backslashes"], s::backslashes),
        b("builder_client", &["scenarios::builder_client"], s::builder_client),
        b("struct_field_base", &["api::ApiClient::list_items"], s::struct_field_base),
        b("struct_field_item", &["api::ApiClient::get_item"], s::struct_field_item),
        b("struct_field_plus", &["api::ApiClient::create_item"], s::struct_field_plus),
        b("struct_field_query", &["api::ApiClient::search"], s::struct_field_query),
        b("struct_param_base", &["api::RemoteClient::user"], move || {
            s::struct_param_base("http://api.example.com/remote")
        }),
        b("url_field", &["api::Catalog::products"], s::url_field),
        b("ureq_get", &["ureq_calls::ureq_get"], u::ureq_get),
        b("ureq_delete", &["ureq_calls::ureq_delete"], || u::ureq_delete(4)),
        b("ureq_dots", &["ureq_calls::ureq_dots"], u::ureq_dots),
        b("ureq_agent_head", &["ureq_calls::ureq_agent_head"], u::ureq_agent_head),
        b("ureq_post", &["ureq_calls::ureq_post"], u::ureq_post),
        b("wrapper_function", &["wrappers::wrapper_function"], w::wrapper_function),
        b("wrapper_method", &["wrappers::wrapper_method"], w::wrapper_method),
        b(
            "wrapper_constructor",
            &["wrappers::wrapper_constructor", "api::ApiClient::execute"],
            || w::wrapper_constructor(6),
        ),
        b("undeclared_sink", &["wrappers::raw_get"], w::undeclared_sink),
    ]
}

// ── 판정 ─────────────────────────────────────────────────────

/// 사실을 오라클이 비교하는 필드만 남긴 요약으로.
fn summary(f: &Value) -> Value {
    let mut out = json!({
        "channel": f["channel"],
        "dynamic": f["dynamic"],
        "pathAnchor": f["pathAnchor"],
        "usr": f.pointer("/symbol/usr").cloned().unwrap_or(Value::Null),
    });
    for key in [
        "method",
        "methodDynamic",
        "authority",
        "channelPrefix",
        "queryTailStripped",
        "maskedSegments",
    ] {
        if let Some(v) = f.get(key) {
            out[key] = v.clone();
        }
    }
    out
}

/// 시나리오 usr들에 귀속된 사실 요약.
fn facts_for(doc: &Value, usrs: &[String]) -> Vec<Value> {
    doc["facts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|f| {
            f.pointer("/symbol/usr")
                .and_then(Value::as_str)
                .is_some_and(|u| usrs.iter().any(|x| x == u))
        })
        .map(summary)
        .collect()
}

/// 요청들과 사실들을 대조한다. (결과, 요청 요약).
fn judge(seen: &[Seen], facts: &[Value]) -> (&'static str, Vec<Value>) {
    let mut requests = Vec::new();
    let statics: Vec<&Value> = facts.iter().filter(|f| f["dynamic"] == false).collect();
    let mut all_matched = true;
    let mut any_dynamic_only = false;
    let mut used = vec![false; statics.len()];
    for s in seen {
        let (authority, path) = split_target(&s.target, &s.host);
        let path = normalize(&path);
        let hit = statics
            .iter()
            .position(|f| fact_matches(f, &s.method, &path, &authority));
        match hit {
            Some(i) => used[i] = true,
            None => {
                let dynamic_ok = facts.iter().any(|f| {
                    f["dynamic"] == true
                        && method_ok(f, &s.method)
                        && f["channelPrefix"]
                            .as_str()
                            .is_none_or(|p| prefix_ok(f, p, &path))
                });
                if dynamic_ok && statics.is_empty() {
                    any_dynamic_only = true;
                } else {
                    all_matched = false;
                }
            }
        }
        requests.push(json!({"method": s.method, "path": path, "authority": authority}));
    }
    let precise = used.iter().all(|u| *u);
    let result = if seen.is_empty() {
        if facts.is_empty() {
            "no-request"
        } else {
            "mismatch"
        }
    } else if !all_matched || !precise {
        "mismatch"
    } else if any_dynamic_only {
        "dynamic"
    } else {
        "match"
    };
    (result, requests)
}

/// 프록시 요청(절대 형식)이면 authority와 경로를, 아니면 Host 헤더와 경로를.
fn split_target(target: &str, host: &str) -> (String, String) {
    let (authority, rest) = match target.strip_prefix("http://") {
        Some(r) => match r.find(['/', '?', '#']) {
            Some(i) => (r[..i].to_ascii_lowercase(), r[i..].to_string()),
            None => (r.to_ascii_lowercase(), "/".to_string()),
        },
        None => (host.to_string(), target.to_string()),
    };
    let path = rest.split(['?', '#']).next().unwrap_or("/").to_string();
    let path = if path.is_empty() { "/".into() } else { path };
    (authority, path)
}

/// 서버가 받은 경로를 정규 표기로(unreserved 디코드, 대문자 hex, 비 pchar 인코딩).
fn normalize(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    let unreserved = |c: u8| c.is_ascii_alphanumeric() || b"-._~".contains(&c);
    let pchar = |c: u8| unreserved(c) || b"!$&'()*+,;=:@/".contains(&c);
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'%'
            && i + 2 < bytes.len()
            && bytes[i + 1].is_ascii_hexdigit()
            && bytes[i + 2].is_ascii_hexdigit()
        {
            let hex = |b: u8| (b as char).to_digit(16).unwrap_or(0) as u8;
            let v = hex(bytes[i + 1]) * 16 + hex(bytes[i + 2]);
            if unreserved(v) {
                out.push(v as char);
            } else {
                out.push_str(&format!("%{v:02X}"));
            }
            i += 3;
            continue;
        }
        if pchar(c) {
            out.push(c as char);
        } else {
            out.push_str(&format!("%{c:02X}"));
        }
        i += 1;
    }
    out
}

fn method_ok(f: &Value, method: &str) -> bool {
    f["methodDynamic"] == true || f["method"] == method
}

/// 정적 사실이 요청을 설명하는가.
fn fact_matches(f: &Value, method: &str, path: &str, authority: &str) -> bool {
    if !method_ok(f, method) {
        return false;
    }
    if let Some(a) = f["authority"].as_str() {
        if a != authority {
            return false;
        }
    }
    let Some(t) = f["channel"].as_str() else {
        return false;
    };
    let tsegs: Vec<&str> = t[1..].split('/').collect();
    let psegs: Vec<&str> = path[1..].split('/').collect();
    let seg_ok = |t: &str, p: &str| if t == "{}" { !p.is_empty() } else { t == p };
    match f["pathAnchor"].as_str() {
        Some("root") => {
            tsegs.len() == psegs.len() && tsegs.iter().zip(&psegs).all(|(t, p)| seg_ok(t, p))
        }
        _ => {
            psegs.len() >= tsegs.len()
                && tsegs
                    .iter()
                    .zip(&psegs[psegs.len() - tsegs.len()..])
                    .all(|(t, p)| seg_ok(t, p))
        }
    }
}

/// dynamic 사실의 channelPrefix가 요청 경로와 맞는가(root는 앞, base는 어딘가).
fn prefix_ok(f: &Value, prefix: &str, path: &str) -> bool {
    let literal = prefix.split("{}").next().unwrap_or(prefix);
    match f["pathAnchor"].as_str() {
        Some("root") => path.starts_with(literal),
        _ => path.contains(literal),
    }
}
