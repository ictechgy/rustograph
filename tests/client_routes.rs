//! `rustograph routes --role client` — isthmus url-compose 벡터, 오라클 기록 대조, 추출 규칙.

use rustograph::source::routes::client::{client_routes, ClientOptions};
use rustograph::source::routes::compose::{
    compose_path, join, mask, parse_template, Join, Origin, Outcome, PathAnchor, Piece, UrlVal,
};
use rustograph::source::routes::template::render;
use rustograph::source::routes::wrappers::{self, ArgSpec, ArgValue, CallArg, MethodSpec};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn load(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("readable")).expect("json")
}

/// 워크스페이스의 클라이언트 문서(JSON).
fn doc_of(dir: &Path, wrappers_json: Option<&str>) -> Value {
    let wrappers = wrappers_json
        .map(|t| wrappers::parse(t).expect("wrappers parse"))
        .unwrap_or_default();
    let opts = ClientOptions {
        wrappers,
        service: None,
    };
    let d = client_routes(dir, "test", &opts).expect("client extraction");
    serde_json::to_value(&d).expect("serializes")
}

// ── 공유 벡터(url-compose) ─────────────────────────────────

fn url_compose_cases() -> Vec<Value> {
    let suite = load(&repo().join("conformance/url-compose.json"));
    suite["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .filter(|c| {
            c["appliesTo"].as_array().is_some_and(|a| {
                a.iter()
                    .any(|t| t == "producer" || t == "producer:rustograph")
            })
        })
        .cloned()
        .collect()
}

fn pieces_of(parts: &Value) -> Vec<Piece> {
    parts
        .as_array()
        .expect("parts")
        .iter()
        .map(|p| {
            if let Some(l) = p["literal"].as_str() {
                Piece::Lit(l.to_string())
            } else if p.get("queryTail").is_some() {
                Piece::QueryTail
            } else {
                Piece::Value(Origin::Unknown)
            }
        })
        .collect()
}

/// 벡터의 결합 이름 → Rust 결합. `dio-concat`은 같은 입력에서 결과가 같은 WHATWG
/// 문자열 연결로 실행한다(점 세그먼트·`//` 처리만 다르고 벡터 입력에는 없다).
fn join_of(name: &str) -> Join {
    match name {
        "rfc3986" => Join::WhatwgJoin,
        "slash-join" => Join::SlashJoin,
        "dio-concat" => Join::WhatwgConcat,
        other => panic!("unclassified join {other}"),
    }
}

fn check_outcome(id: &str, c: &Value, out: &Outcome) {
    let expect = &c["expect"];
    if c["expectDynamic"] == true {
        let Outcome::Dynamic {
            prefix, ambiguous, ..
        } = out
        else {
            panic!("{id}: expected dynamic, got {out:?}");
        };
        if let Some(p) = expect.get("channelPrefix") {
            assert_eq!(prefix.as_deref(), p.as_str(), "{id} channelPrefix");
        }
        if c["expectLimitation"] == "ambiguous-base-join:" {
            assert!(ambiguous, "{id}: ambiguous-base-join expected");
        }
        return;
    }
    let Outcome::Template(t) = out else {
        panic!("{id}: expected template, got {out:?}");
    };
    assert_eq!(
        Some(t.template.as_str()),
        expect["template"].as_str(),
        "{id}"
    );
    if let Some(a) = expect["pathAnchor"].as_str() {
        assert_eq!(t.anchor.as_str(), a, "{id} pathAnchor");
    }
    if let Some(a) = expect.get("authority") {
        assert_eq!(t.authority.as_deref(), a.as_str(), "{id} authority");
    }
    assert_eq!(
        t.query_tail_stripped,
        expect["queryTailStripped"] == true,
        "{id} queryTailStripped"
    );
    if let Some(m) = expect["maskedSegments"].as_u64() {
        assert_eq!(t.masked_segments as u64, m, "{id} maskedSegments");
    }
}

fn arg_spec(v: &Value) -> Option<ArgSpec> {
    v.as_object().map(|o| ArgSpec {
        index: o.get("index").and_then(Value::as_u64).map(|i| i as usize),
        label: o.get("label").and_then(Value::as_str).map(str::to_string),
    })
}

fn run_wrapper_method(id: &str, c: &Value) {
    let d = &c["input"]["declaration"];
    let spec = MethodSpec {
        method_arg: arg_spec(&d["methodArg"]),
        default_method: d["defaultMethod"].as_str().map(str::to_string),
        method_enum: d["methodEnum"]
            .as_object()
            .map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_string()))
                    .collect()
            })
            .unwrap_or_default(),
    };
    let args: Vec<CallArg> = c["input"]["call"]["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| CallArg {
            label: a["label"].as_str().map(str::to_string),
            value: if let Some(l) = a["value"]["literal"].as_str() {
                ArgValue::Literal(l.to_string())
            } else if let Some(e) = a["value"]["enumCase"].as_str() {
                ArgValue::EnumCase(e.to_string())
            } else {
                ArgValue::Other
            },
        })
        .collect();
    let method = wrappers::bind_method(&spec, &args);
    if c["expectDynamic"] == true {
        assert_eq!(method, None, "{id}");
    } else {
        assert_eq!(method.as_deref(), c["expect"]["method"].as_str(), "{id}");
    }
}

#[test]
fn url_compose_producer_cases_pass() {
    let cases = url_compose_cases();
    // 조용히 줄지 않게 — 벡터 재벤더링 때 수를 확인한다(producer 41, producer:kartograph 13 제외).
    assert_eq!(cases.len(), 41);
    let mut by_rule: BTreeMap<String, usize> = BTreeMap::new();
    for c in &cases {
        let id = c["id"].as_str().unwrap_or("");
        let input = &c["input"];
        let rule = c["ruleId"].as_str().unwrap_or("");
        *by_rule.entry(rule.to_string()).or_default() += 1;
        match rule {
            "compose.interpolation"
            | "compose.query-tail"
            | "compose.suffix"
            | "compose.normalize" => {
                let out = compose_path(PathAnchor::Root, &pieces_of(&input["parts"]), None, true);
                check_outcome(id, c, &out);
            }
            "compose.base-join" => {
                let path = [Piece::Lit(input["path"].as_str().unwrap().to_string())];
                let out = join(
                    join_of(input["join"].as_str().unwrap()),
                    input["base"].as_str(),
                    &path,
                );
                check_outcome(id, c, &out);
            }
            "compose.strip" => {
                let url = [Piece::Lit(input["url"].as_str().unwrap().to_string())];
                check_outcome(id, c, &UrlVal::parse(&url, true).outcome());
            }
            "compose.mask" => {
                let mut segs = parse_template(input["template"].as_str().unwrap());
                let n = mask(input["authority"].as_str(), &mut segs);
                assert_eq!(
                    Some(render(&segs).as_str()),
                    c["expect"]["template"].as_str(),
                    "{id}"
                );
                assert_eq!(
                    Some(n as u64),
                    c["expect"]["maskedSegments"].as_u64(),
                    "{id}"
                );
            }
            "wrapper.method" => run_wrapper_method(id, c),
            // 실제 스캐너로 확인한다 — `wrapper_location_is_the_call_start_line`.
            "wrapper.location" => {
                assert_eq!(c["expect"]["line"], input["callStartLine"], "{id}");
            }
            other => panic!("unclassified url-compose rule {other} ({id})"),
        }
    }
    assert_eq!(by_rule.get("compose.base-join"), Some(&7));
}

/// `wrapper.location`: 여러 줄 호출은 호출식이 시작하는 줄이다. 열은 UTF-8 바이트다.
#[test]
fn wrapper_location_is_the_call_start_line() {
    let src = r#"
pub fn send(method: &str, path: &str) {}
pub fn caller() {
    /* 한글 */ send(
        "GET",
        "/x",
    );
}
"#;
    let dir = temp_crate("location", &[], &[("lib.rs", src)]);
    let wrappers = r#"{"format":"http-wrappers","version":1,"wrappers":[{"language":"rust","kind":"function","owner":"app","name":"send","methodArg":{"index":0},"pathArg":{"index":1},"pathAnchor":"root"}]}"#;
    let doc = doc_of(&dir, Some(wrappers));
    let f = &doc["facts"][0];
    assert_eq!(f["location"]["line"], 4);
    // `    /* 한글 */ ` = 4 + 3 + 6(한글 UTF-8) + 4 = 17바이트 → 열 18.
    assert_eq!(f["location"]["column"], 18);
    assert_eq!(f["method"], "GET");
    assert_eq!(f["channel"], "/x");
    assert_eq!(f["pathAnchor"], "root");
}

// ── 오라클 기록 대조 ────────────────────────────────────────

/// 오라클 요약과 같은 필드만 남긴다.
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

fn fixture_doc() -> Value {
    let dir = repo().join("tests/fixture-client");
    let wrappers = std::fs::read_to_string(dir.join("http-wrappers.json")).unwrap();
    doc_of(&dir.join("app"), Some(&wrappers))
}

/// 지금 출력의 사실이 오라클이 실제 요청으로 확인한 사실과 정확히 같아야 한다.
/// 다시 기록하려면 experiments/client-oracle/run.sh.
#[test]
fn fixture_matches_client_oracle() {
    let rec = load(&repo().join("experiments/client-oracle/recorded/client.json"));
    let totals = &rec["totals"];
    assert_eq!(totals["mismatch"], 0, "the oracle recorded mismatches");
    assert!(rec["unclaimedFacts"].as_array().unwrap().is_empty());
    assert!(
        totals["match"].as_u64().unwrap() >= 30,
        "the oracle matched too little"
    );
    let recorded: BTreeSet<String> = rec["scenarios"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["facts"].as_array().unwrap().iter())
        .map(|f| f.to_string())
        .collect();
    let doc = fixture_doc();
    let now: BTreeSet<String> = doc["facts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| summary(f).to_string())
        .collect();
    assert_eq!(now, recorded, "re-run experiments/client-oracle/run.sh");
    // 요청이 없던 시나리오(보내기 전 실패)는 사실도 없다.
    for s in rec["scenarios"].as_array().unwrap() {
        if s["result"] == "no-request" {
            assert!(
                s["facts"].as_array().unwrap().is_empty(),
                "{}",
                s["scenario"]
            );
        }
    }
}

#[test]
fn fixture_document_shape_and_limitations() {
    let doc = fixture_doc();
    assert_eq!(doc["platform"], "rust");
    assert_eq!(doc["target"], "http");
    assert_eq!(doc["roles"], json!(["client"]));
    assert_eq!(doc["sourceSets"]["tests"], "excluded");
    assert!(doc.get("dispatch").is_none());
    let lims: Vec<&str> = doc["limitations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect();
    assert!(
        lims.iter()
            .any(|l| l.starts_with("ambiguous-base-join: 1 ")),
        "{lims:?}"
    );
    assert!(
        lims.iter()
            .any(|l| l.starts_with("http-wrapper-undeclared: 2 ")),
        "{lims:?}"
    );
    assert!(lims.iter().any(|l| l.contains("relative URL")), "{lims:?}");
    // 다른 언어 선언(swift)은 적용하지 않고 공백으로도 세지 않는다.
    assert!(!lims.iter().any(|l| l.contains("Network")), "{lims:?}");
    // dynamic 사실은 원문 식을 싣지 않는다.
    for f in doc["facts"].as_array().unwrap() {
        if f["dynamic"] == true {
            assert!(f["channel"].is_null());
        }
    }
    // 모든 usr가 reach 정점이다 — 같은 fixture의 그래프에서 확인한다.
    let graph = rustograph::source::load(
        &repo().join("tests/fixture-client/app"),
        &rustograph::source::Options {
            symbol_level: true,
            ..Default::default()
        },
    )
    .unwrap();
    let ids = graph.vertex_ids();
    for f in doc["facts"].as_array().unwrap() {
        let usr = f["symbol"]["usr"].as_str().unwrap();
        assert!(ids.contains(usr), "{usr} is not a graph vertex");
    }
}

// ── 추출 규칙(임시 크레이트) ────────────────────────────────

/// 임시 워크스페이스 — 스텁 의존성의 이름·버전과 소스 파일들.
fn temp_crate(tag: &str, deps: &[(&str, &str)], files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rg-client-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ws/app/src")).unwrap();
    let mut dep_lines = String::new();
    for (name, version) in deps {
        let stub = dir.join(format!("stub-{name}"));
        std::fs::create_dir_all(stub.join("src")).unwrap();
        std::fs::write(
            stub.join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"{version}\"\nedition = \"2021\"\n"),
        )
        .unwrap();
        std::fs::write(stub.join("src/lib.rs"), "").unwrap();
        dep_lines.push_str(&format!("{name} = {{ path = \"../../stub-{name}\" }}\n"));
    }
    std::fs::write(
        dir.join("ws/Cargo.toml"),
        "[workspace]\nmembers = [\"app\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("ws/app/Cargo.toml"),
        format!("[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{dep_lines}"),
    )
    .unwrap();
    for (path, text) in files {
        std::fs::write(dir.join("ws/app/src").join(path), text).unwrap();
    }
    dir.join("ws")
}

/// (usr 끝 이름, method 또는 *, channel 또는 DYN:prefix, anchor).
fn rows(doc: &Value) -> Vec<(String, String, String, String)> {
    doc["facts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            let usr = f["symbol"]["usr"].as_str().unwrap_or("-");
            let short = usr.rsplit("::").next().unwrap_or(usr).to_string();
            let channel = match f["channel"].as_str() {
                Some(c) => c.to_string(),
                None => format!("DYN:{}", f["channelPrefix"].as_str().unwrap_or("")),
            };
            (
                short,
                f["method"].as_str().unwrap_or("*").to_string(),
                channel,
                f["pathAnchor"].as_str().unwrap().to_string(),
            )
        })
        .collect()
}

fn has(
    rows: &[(String, String, String, String)],
    usr: &str,
    method: &str,
    channel: &str,
    anchor: &str,
) -> bool {
    rows.iter()
        .any(|r| r.0 == usr && r.1 == method && r.2 == channel && r.3 == anchor)
}

fn limitations(doc: &Value) -> Vec<String> {
    doc["limitations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn values_flow_through_locals_consts_and_types() {
    let src = r#"
use reqwest::Client;
use std::sync::{Arc, OnceLock};

const HOST: &str = "http://h.test";
const API: &str = concat!("http://h.test", "/api/", 2);
static SHARED: OnceLock<Client> = OnceLock::new();

pub struct Svc { http: Arc<Client>, root: String }

impl Svc {
    const BASE: &'static str = "http://h.test/svc";

    pub fn make() -> Svc { Svc { http: Arc::new(Client::new()), root: format!("{}/root", HOST) } }

    fn client(&self) -> &Client { &self.http }

    pub fn assoc_const(&self) { self.client().get(format!("{}/a", Self::BASE)); }
    pub fn field_root(&self) { self.http.get(self.root.clone() + "/x"); }
    pub fn named_args(&self, id: u32) { self.http.get(format!("{h}/n/{id}", h = HOST)); }
    pub fn debug_spec(&self, id: u32) { self.http.get(format!("{HOST}/d/{:?}", id)); }
    pub fn width_arg(&self, id: u32) { self.http.get(format!("{HOST}/w/{:>1$}", id, 5)); }
    pub fn concat_int(&self) { self.http.get(API); }
}

fn shared() -> &'static Client { SHARED.get_or_init(Client::new) }

pub fn once_lock() { shared().get(format!("{HOST}/once")); }
pub fn mutated() { let mut u = String::from(HOST); u.push_str("/m"); Client::new().get(u); }
pub fn shadowed() { let u = "http://h.test/one"; let u = format!("{u}/two"); Client::new().get(u); }
pub fn closure_param() { let c = Client::new(); let f = |u: &str| c.get(u); f("http://h.test/c"); }
pub fn loops(items: Vec<&str>) { let c = Client::new(); for it in items { c.delete(format!("{HOST}/l/{it}")); } }
pub fn matched(x: Option<&str>) {
    let c = Client::new();
    match x { Some(v) => { c.put(format!("{HOST}/p/{v}")); } None => {} }
    if let Some(v) = x { c.patch(format!("{HOST}/q/{v}")); }
    while let Some(v) = x { c.head(format!("{HOST}/r/{v}")); break; }
}
pub fn query_match(page: u8) {
    let q = match page { 0 => String::new(), n => format!("?p={n}") };
    Client::new().get(format!("{HOST}/qm{q}"));
}
pub fn no_else(page: u8) {
    let q = if page > 0 { format!("?p={page}") };
    Client::new().get(format!("{HOST}/ne{q}"));
}
pub fn nested() {
    fn inner() { reqwest::Client::new().get("http://h.test/inner"); }
    inner();
}
pub fn in_macro() { let c = Client::new(); let _ = vec![c.get("http://h.test/mac")]; }
pub fn request_new() { let _ = reqwest::Request::new(reqwest::Method::DELETE, "http://h.test/rn".parse().unwrap()); }
pub fn http_method() { Client::new().request(http::Method::TRACE, format!("{HOST}/t")); }

pub trait Api { fn ping(&self, c: &Client) { c.get("http://h.test/ping"); } }

#[cfg(test)]
mod tests { fn t() { reqwest::Client::new().get("http://h.test/test-only"); } }

#[test]
fn test_fn() { reqwest::Client::new().get("http://h.test/test-fn"); }
"#;
    let dir = temp_crate(
        "values",
        &[("reqwest", "0.13.5"), ("http", "1.3.1")],
        &[("lib.rs", src)],
    );
    let doc = doc_of(&dir, None);
    let r = rows(&doc);
    let expect = [
        ("assoc_const", "GET", "/svc/a", "root"),
        ("field_root", "GET", "/root/x", "root"),
        ("named_args", "GET", "/n/{}", "root"),
        ("debug_spec", "GET", "/d/{}", "root"),
        ("concat_int", "GET", "/api/2", "root"),
        ("once_lock", "GET", "/once", "root"),
        ("shadowed", "GET", "/one/two", "root"),
        ("closure_param", "GET", "DYN:", "base"),
        ("loops", "DELETE", "/l/{}", "root"),
        ("matched", "PUT", "/p/{}", "root"),
        ("matched", "PATCH", "/q/{}", "root"),
        ("matched", "HEAD", "/r/{}", "root"),
        ("query_match", "GET", "/qm", "root"),
        ("no_else", "GET", "DYN:/ne", "root"),
        ("mutated", "GET", "DYN:", "base"),
        // 중첩 함수는 정점이 아니다 — 감싸는 함수가 usr다.
        ("nested", "GET", "/inner", "root"),
        ("in_macro", "GET", "/mac", "root"),
        ("http_method", "TRACE", "/t", "root"),
        ("ping", "GET", "/ping", "root"),
    ];
    for (u, m, c, a) in expect {
        assert!(has(&r, u, m, c, a), "missing {u} {m} {c} {a} in {r:#?}");
    }
    // `{:>1$}`는 인자 순서를 바꾸는 서식이라 URL 전체를 모른다.
    assert!(has(&r, "width_arg", "GET", "DYN:", "base"), "{r:#?}");
    // Request::new의 URL은 `.parse()` 결과(Url)라 모르는 값이다 — 동사는 확정한다.
    assert!(has(&r, "request_new", "DELETE", "DYN:", "base"), "{r:#?}");
    assert!(
        !r.iter().any(|x| x.2.contains("test")),
        "test sources leaked: {r:#?}"
    );
}

#[test]
fn fields_resolve_only_from_constant_constructors() {
    let src = r#"
use reqwest::blocking::Client;

#[derive(Default)]
pub struct Defaulted { base: String, http: Client }
impl Defaulted { pub fn go(&self) { self.http.get(format!("{}/d", self.base)); } }

pub struct Two { base: String, http: Client }
impl Two {
    pub fn a() -> Two { Two { base: "http://one.test".into(), http: Client::new() } }
    pub fn b() -> Two { Two { base: "http://two.test".into(), http: Client::new() } }
    pub fn go(&self) { self.http.get(format!("{}/t", self.base)); }
}

pub struct Poisoned { base: String, http: Client }
impl Poisoned {
    pub fn new() -> Poisoned { Poisoned { base: "http://p.test".into(), http: Client::new() } }
    pub fn rebase(&mut self) { self.base = String::new(); }
    pub fn go(&self) { self.http.get(format!("{}/p", self.base)); }
}

pub struct Rest { base: String, http: Client }
impl Rest {
    pub fn new(other: Rest) -> Rest { Rest { http: Client::new(), ..other } }
    pub fn go(&self) { self.http.get(format!("{}/r", self.base)); }
}

pub struct Other { base: String }
pub fn via_instance(o: &Other, c: &Client) {
    let o2 = Other { base: "http://o.test/x".to_string() };
    c.get(format!("{}/i", o.base));
    c.get(format!("{}/j", o2.base));
}
"#;
    let dir = temp_crate("fields", &[("reqwest", "0.12.9")], &[("lib.rs", src)]);
    let doc = doc_of(&dir, None);
    let r = rows(&doc);
    // 네 구조체 모두 필드 값이 확정되지 않아 base 앵커다.
    let n = r
        .iter()
        .filter(|x| x.0 == "go" && x.3 == "base" && !x.2.starts_with("DYN"))
        .count();
    assert_eq!(n, 4, "{r:#?}");
    assert!(has(&r, "via_instance", "GET", "/x/i", "root"), "{r:#?}");
    assert!(has(&r, "via_instance", "GET", "/x/j", "root"), "{r:#?}");
    let base_refs: BTreeSet<&str> = doc["facts"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["baseRef"].as_str())
        .collect();
    assert!(base_refs.contains("app::Two::base"), "{base_refs:?}");
}

#[test]
fn ureq_versions_and_agents() {
    let src = r#"
pub fn free() { ureq::get("http://u.test/a/../b").call(); }
pub fn agent2() { let a = ureq::AgentBuilder::new().build(); a.put("http://u.test/p").call(); }
pub fn agent3() { let a = ureq::Agent::config_builder().build().new_agent(); a.options("http://u.test/o").call(); }
pub fn request2(m: &str) {
    ureq::request("PATCH", "http://u.test/rq").call();
    ureq::request("get", "http://u.test/lower").call();
    ureq::request(m, "http://u.test/param").call();
}
pub fn unproven(x: &Thing) { x.get("http://u.test/nope").call(); }
pub struct Thing;
"#;
    let v2 = doc_of(
        &temp_crate("ureq2", &[("ureq", "2.12.1")], &[("lib.rs", src)]),
        None,
    );
    let r2 = rows(&v2);
    // ureq 2는 url::Url이라 점 세그먼트를 지운다.
    assert!(has(&r2, "free", "GET", "/b", "root"), "{r2:#?}");
    assert!(has(&r2, "agent2", "PUT", "/p", "root"), "{r2:#?}");
    assert!(has(&r2, "request2", "PATCH", "/rq", "root"), "{r2:#?}");
    assert!(has(&r2, "request2", "*", "/lower", "root"), "{r2:#?}");
    assert!(has(&r2, "request2", "*", "/param", "root"), "{r2:#?}");
    let l2 = limitations(&v2);
    assert!(l2.iter().any(|l| l.contains("ureq 2.12.1")), "{l2:?}");
    assert!(l2.iter().any(|l| l.contains("not a proven")), "{l2:?}");
    assert!(
        l2.iter()
            .any(|l| l.starts_with("http-wrapper-undeclared: 1 ")),
        "{l2:?}"
    );
    let v3 = doc_of(
        &temp_crate("ureq3", &[("ureq", "3.4.2")], &[("lib.rs", src)]),
        None,
    );
    let r3 = rows(&v3);
    assert!(has(&r3, "free", "GET", "/a/../b", "root"), "{r3:#?}");
    assert!(has(&r3, "agent3", "OPTIONS", "/o", "root"), "{r3:#?}");
    assert!(!limitations(&v3).iter().any(|l| l.contains("ureq 3.4.2")));
}

#[test]
fn unmodelled_clients_and_versions_are_counted() {
    let src = r#"
pub async fn a() { let _ = surf::get("http://s.test/x").await; }
pub fn b() { let _c = hyper_util::client::legacy::Client::builder(); }
pub fn serve() { let _s = hyper::server::conn::http1::Builder::new(); }
pub fn old() { reqwest::get("http://r.test/old"); }
"#;
    let dir = temp_crate(
        "unmodelled",
        &[
            ("surf", "2.3.2"),
            ("hyper-util", "0.1.10"),
            ("hyper", "1.6.0"),
            ("reqwest", "0.11.27"),
        ],
        &[("lib.rs", src)],
    );
    let doc = doc_of(&dir, None);
    let l = limitations(&doc);
    assert!(l.iter().any(|x| x.contains("1 use(s) of surf")), "{l:?}");
    assert!(
        l.iter().any(|x| x.contains("1 use(s) of hyper_util")),
        "{l:?}"
    );
    // 서버 쪽 hyper 경로는 세지 않는다.
    assert!(
        !l.iter()
            .any(|x| x.contains("of hyper,") || x.contains("of hyper ")),
        "{l:?}"
    );
    assert!(l.iter().any(|x| x.contains("reqwest 0.11.27")), "{l:?}");
    assert!(has(&rows(&doc), "old", "GET", "/old", "root"));
}

#[test]
fn wrapper_declarations_resolve_or_report() {
    let src = r#"
pub enum M { Get, Put }
pub struct Api;
impl Api {
    pub fn call(&self, m: M, path: &str) {}
    pub fn raw(m: M, path: &str) {}
}
pub trait Send2 { fn send2(&self, path: &str); }
impl Send2 for Api { fn send2(&self, path: &str) {} }
pub struct Ep(pub M, pub &'static str);
pub fn go(api: &Api, id: u32, dynamic_verb: M) {
    api.call(M::Put, "/a");
    Api::call(api, M::Get, "/ufcs");
    Api::raw(M::Get, "https://Other.test/abs?x=1");
    api.send2(&format!("/t/{id}"));
    let _ = Ep(M::Put, "/tuple");
    api.call(dynamic_verb, "relative");
}
"#;
    let dir = temp_crate("wrappers", &[], &[("lib.rs", src)]);
    let wrappers = r#"{"format":"http-wrappers","version":1,"wrappers":[
      {"language":"rust","kind":"function","owner":"app::Api","name":"call","methodArg":{"index":0},"methodEnum":{"Get":"GET","Put":"PUT"},"pathArg":{"index":1},"pathAnchor":"base","service":"api"},
      {"language":"rust","kind":"function","owner":"app::Api","name":"raw","methodArg":{"index":0},"methodEnum":{"Get":"GET"},"pathArg":{"index":1},"pathAnchor":"root"},
      {"language":"rust","kind":"function","owner":"app::Api::<Send2>","name":"send2","pathArg":{"index":0},"defaultMethod":"POST","pathAnchor":"root"},
      {"language":"rust","kind":"constructor","owner":"app::Ep","name":"Ep","methodArg":{"index":0},"methodEnum":{"Put":"PUT"},"pathArg":{"index":1},"pathAnchor":"root"},
      {"language":"rust","kind":"function","owner":"app::Missing","name":"nope","pathArg":{"index":0},"defaultMethod":"GET","pathAnchor":"root"},
      {"language":"rust","kind":"constructor","owner":"app::Api","name":"new","pathArg":{"index":0},"defaultMethod":"GET","pathAnchor":"root"},
      {"language":"kotlin","kind":"function","owner":"x","name":"y","pathArg":{"index":0},"defaultMethod":"GET","pathAnchor":"root"}
    ]}"#;
    let doc = doc_of(&dir, Some(wrappers));
    let r = rows(&doc);
    assert!(has(&r, "go", "PUT", "/a", "base"), "{r:#?}");
    assert!(has(&r, "go", "GET", "/ufcs", "base"), "{r:#?}");
    assert!(has(&r, "go", "GET", "/abs", "root"), "{r:#?}");
    assert!(has(&r, "go", "POST", "/t/{}", "root"), "{r:#?}");
    assert!(has(&r, "go", "PUT", "/tuple", "root"), "{r:#?}");
    assert!(has(&r, "go", "*", "DYN:", "base"), "{r:#?}");
    let abs = doc["facts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["channel"] == "/abs")
        .unwrap();
    assert_eq!(abs["authority"], "other.test");
    assert_eq!(abs["queryTailStripped"], true);
    let svc = doc["facts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["service"] == "api")
        .count();
    assert_eq!(svc, 3);
    let l = limitations(&doc);
    assert!(
        l.iter()
            .any(|x| x.contains("wrappers[4] (app::Missing::nope) does not name")),
        "{l:?}"
    );
    assert!(
        l.iter()
            .any(|x| x.contains("wrappers[5] (app::Api::new) does not name")),
        "{l:?}"
    );
    assert!(
        l.iter().any(|x| x.starts_with("ambiguous-base-join: 1 ")),
        "{l:?}"
    );
    assert!(!l.iter().any(|x| x.contains("wrappers[6]")), "{l:?}");
    // 호출 0건 선언.
    let zero = r#"{"format":"http-wrappers","version":1,"wrappers":[{"language":"rust","kind":"function","owner":"app","name":"go","pathArg":{"index":0},"defaultMethod":"GET","pathAnchor":"root"}]}"#;
    let l0 = limitations(&doc_of(&dir, Some(zero)));
    assert!(
        l0.iter()
            .any(|x| x.contains("wrappers[0] (app::go) has no calls")),
        "{l0:?}"
    );
}

#[test]
fn service_conflict_is_a_usage_error() {
    let dir = temp_crate("service", &[], &[("lib.rs", "pub fn f(p: &str) {}\n")]);
    let wrappers = wrappers::parse(r#"{"format":"http-wrappers","version":1,"wrappers":[{"language":"rust","kind":"function","owner":"app","name":"f","pathArg":{"index":0},"defaultMethod":"GET","pathAnchor":"root","service":"a"}]}"#).unwrap();
    let opts = ClientOptions {
        wrappers,
        service: Some("b".into()),
    };
    let err = client_routes(&dir, "test", &opts).err().expect("conflict");
    assert!(err.contains("service"), "{err}");
    let ok = ClientOptions {
        wrappers: Vec::new(),
        service: Some("b".into()),
    };
    let doc = serde_json::to_value(client_routes(&dir, "test", &ok).unwrap()).unwrap();
    assert_eq!(doc["service"], "b");
    assert!(doc["facts"].as_array().unwrap().is_empty());
    assert_eq!(doc["target"], "http");
}

/// 상수가 상수를 가리키는 긴 사슬도 깊이 상한 안에서 끝난다 — 스택을 넘기지 않고
/// 모르는 값(baseRef)으로 낮추며, 짧은 사슬은 그대로 푼다.
#[test]
fn deep_const_chains_degrade_instead_of_overflowing() {
    let mut src = String::from("const C0: &str = \"http://h.test/x\";\n");
    for i in 1..3000 {
        src.push_str(&format!("const C{i}: &str = C{};\n", i - 1));
    }
    src.push_str(
        "pub fn deep() { reqwest::get(C2999); }\npub fn shallow() { reqwest::get(C3); }\n",
    );
    let dir = temp_crate("deepconst", &[("reqwest", "0.13.5")], &[("lib.rs", &src)]);
    let r = rows(&doc_of(&dir, None));
    assert!(has(&r, "deep", "GET", "DYN:", "base"), "{r:#?}");
    assert!(has(&r, "shallow", "GET", "/x", "root"), "{r:#?}");
}
