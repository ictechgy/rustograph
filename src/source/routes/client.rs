//! isthmus `bridge-facts` v1 http 도메인의 호출 측 생산자(`rustograph routes --role
//! client`) — reqwest·ureq 요청과 선언된 래퍼 호출을 `route-call` 사실로 낸다.
//!
//! 흐름:
//!
//! 1. `cargo metadata`로 멤버 크레이트별 reqwest·ureq(과 모델링하지 않는 클라이언트)
//!    의존과 버전을 찾는다 — ureq는 메이저에 따라 URL 해석기가 다르다.
//! 2. `impact`와 같은 syn 수확으로 모듈 트리·AST·정점 집합을 얻는다 — `symbol.usr`가
//!    `reach`/`impact`의 정점 ID와 같아야 isthmus `trace`가 호출부에서 이어 간다.
//! 3. 스캐너가 두 번 돈다(필드 값 수집 → 호출 사실). URL 조립은 `compose`가,
//!    래퍼 동사 바인딩은 `wrappers`가 한다.
//! 4. 사실을 정렬·검사하고 호출 측 공백을 닫힌 접두사 목록의 한계로 낸다.

mod index;
mod scan;

use super::common::Ctx;
use super::compose::{Outcome, PathAnchor};
use super::template::template_problem;
use super::wrappers::Wrapper;
use crate::cargo_meta;
use crate::source::schema::{rfc3339_utc_now, BridgeFactsTool, BridgeLocation, FactSymbol};
use index::Index;
use scan::{scan_crate, CallSite, Collected, Libs, Mode, Shared};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::Path;

/// 이름으로 사용을 세는, 모델링하지 않는 HTTP 클라이언트 패키지(cargo_meta가 `-`를
/// `_`로 정규화한 이름).
const UNMODELLED: &[&str] = &[
    "hyper",
    "hyper_util",
    "surf",
    "awc",
    "isahc",
    "attohttpc",
    "minreq",
    "curl",
    "ehttp",
    "gloo_net",
    "reqwasm",
    "http_client",
    "reqwest_middleware",
];

/// `routes --role client` 옵션.
#[derive(Debug, Default)]
pub struct ClientOptions {
    /// `http-wrappers` v1 선언(모든 언어 항목 — `rust`만 적용한다).
    pub wrappers: Vec<Wrapper>,
    /// 문서 수준 `service`.
    pub service: Option<String>,
}

/// route-call 사실 하나. 키 순서는 서버 사실과 같은 계약 나열 순서다.
#[derive(Serialize)]
pub struct CallFact {
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(rename = "methodDynamic", skip_serializing_if = "Option::is_none")]
    pub method_dynamic: Option<bool>,
    /// dynamic이면 null — 원문 식은 URL의 userinfo·query를 담을 수 있어 싣지 않는다.
    pub channel: Option<String>,
    pub dynamic: bool,
    #[serde(rename = "pathAnchor")]
    pub path_anchor: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authority: Option<String>,
    #[serde(rename = "baseRef", skip_serializing_if = "Option::is_none")]
    pub base_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    #[serde(rename = "queryTailStripped", skip_serializing_if = "Option::is_none")]
    pub query_tail_stripped: Option<bool>,
    #[serde(rename = "channelPrefix", skip_serializing_if = "Option::is_none")]
    pub channel_prefix: Option<String>,
    #[serde(rename = "maskedSegments", skip_serializing_if = "Option::is_none")]
    pub masked_segments: Option<usize>,
    pub location: BridgeLocation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<FactSymbol>,
}

/// 문서의 `sourceSets`.
#[derive(Serialize, Debug)]
pub struct ClientSourceSets {
    pub tests: &'static str,
}

/// isthmus bridge-facts v1 http 클라이언트 문서다.
#[derive(Serialize)]
pub struct ClientDocument {
    pub format: &'static str,
    pub version: u8,
    pub tool: BridgeFactsTool,
    #[serde(rename = "generatedAt")]
    pub generated_at: String,
    pub platform: &'static str,
    /// roles가 있는 http 문서는 사실 0건이어도 `http`다(스캔했으나 호출 없음).
    pub target: &'static str,
    pub project: String,
    pub roles: Vec<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    #[serde(rename = "sourceSets")]
    pub source_sets: ClientSourceSets,
    pub facts: Vec<CallFact>,
    pub limitations: Vec<String>,
}

/// 멤버 크레이트 하나 — 수확 루트와 의존 라이브러리.
struct Member {
    name: String,
    roots: Vec<String>,
    libs: Libs,
    /// (패키지, 버전) — 확인한 버전 밖이면 한계로 센다.
    versions: Vec<(String, String)>,
}

/// 워크스페이스의 HTTP 요청 호출을 http 클라이언트 문서로 낸다.
pub fn client_routes(
    dir: &Path,
    tool_version: &str,
    opts: &ClientOptions,
) -> Result<ClientDocument, String> {
    let root = dir
        .canonicalize()
        .map_err(|e| format!("cannot resolve {}: {e}", dir.display()))?;
    let meta = cargo_meta::load(dir)?;
    let members = members(&meta);
    let parts = crate::source::harvest_parts(dir, &meta)?;
    let ctx = Ctx::new(&parts, root.clone());
    let krates: Vec<String> = members.iter().flat_map(|m| m.roots.clone()).collect();
    let index = Index::build(&ctx, &krates);
    let wrappers: Vec<Wrapper> = opts
        .wrappers
        .iter()
        .filter(|w| w.language == "rust")
        .cloned()
        .collect();
    check_services(&wrappers, opts.service.as_deref())?;
    let sh = Shared::new(&ctx, &index, &wrappers);
    for mode in [Mode::Collect, Mode::Emit] {
        for m in &members {
            for r in &m.roots {
                scan_crate(&sh, r, &m.libs, mode);
            }
        }
        if mode == Mode::Collect {
            sh.finish_collect();
        }
    }
    let collected = std::mem::take(&mut *sh.out.borrow_mut());
    let mut limitations = coverage(&members, &collected);
    limitations.extend(wrapper_gaps(&sh, &wrappers, &collected));
    let (facts, missing) = facts(&ctx, &collected.calls, &mut limitations)?;
    if missing > 0 {
        limitations.push(format!("missing-route-usrs: {missing} route call(s) have no enclosing workspace function or method; they carry no symbol"));
    }
    for l in &meta.limitations {
        limitations.push(format!("route-call-coverage: {l}"));
    }
    limitations.sort();
    limitations.dedup();
    Ok(ClientDocument {
        format: "bridge-facts",
        version: 1,
        tool: BridgeFactsTool {
            name: "rustograph",
            version: tool_version.to_string(),
        },
        generated_at: rfc3339_utc_now(),
        platform: "rust",
        target: "http",
        project: root.display().to_string(),
        roles: vec!["client"],
        service: opts.service.clone(),
        source_sets: ClientSourceSets { tests: "excluded" },
        facts,
        limitations,
    })
}

/// 래퍼 선언의 service가 문서 service와 다르면 isthmus가 문서를 거부한다 — 먼저 막는다.
fn check_services(wrappers: &[Wrapper], service: Option<&str>) -> Result<(), String> {
    let Some(doc) = service else { return Ok(()) };
    for w in wrappers {
        if let Some(s) = &w.service {
            if s != doc {
                return Err(format!(
                    "wrappers[{}] declares service {s:?} but --service is {doc:?}; isthmus rejects a fact whose service differs from the document's — drop --service or align the declaration",
                    w.position
                ));
            }
        }
    }
    Ok(())
}

/// 워크스페이스 멤버와 그 HTTP 클라이언트 의존.
fn members(meta: &cargo_meta::Metadata) -> Vec<Member> {
    let mut out = Vec::new();
    for (id, &pi) in &meta.by_id {
        let pkg = &meta.packages[pi];
        if !pkg.workspace_member {
            continue;
        }
        let roots: Vec<String> = pkg
            .targets
            .iter()
            .filter(|t| matches!(t.kind.as_str(), "lib" | "bin"))
            .map(|t| t.name.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut libs = Libs::default();
        let mut versions = Vec::new();
        for d in meta
            .dep_edges
            .iter()
            .filter(|d| &d.from == id && d.kind.is_empty())
        {
            let Some(&ti) = meta.by_id.get(&d.to) else {
                continue;
            };
            let to = &meta.packages[ti];
            match to.name.as_str() {
                "reqwest" => {
                    libs.reqwest = Some(d.lib_name.clone());
                    versions.push((to.name.clone(), to.version.clone()));
                }
                "ureq" => {
                    // 메이저를 읽지 못하면 최신(3.x, http::Uri) 규칙으로 읽고 버전 한계로 센다.
                    let major = to
                        .version
                        .split('.')
                        .next()
                        .and_then(|m| m.parse().ok())
                        .unwrap_or(3);
                    libs.ureq = Some((d.lib_name.clone(), major));
                    versions.push((to.name.clone(), to.version.clone()));
                }
                name if UNMODELLED.contains(&name) => libs.unmodelled.push(d.lib_name.clone()),
                _ => {}
            }
        }
        out.push(Member {
            name: pkg.name.clone(),
            roots,
            libs,
            versions,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// 호출 측 커버리지 한계(`route-call-coverage:`).
fn coverage(members: &[Member], c: &Collected) -> Vec<String> {
    let mut out = Vec::new();
    for m in members {
        for (name, version) in &m.versions {
            let verified = match name.as_str() {
                "reqwest" => version.starts_with("0.13.") || version.starts_with("0.12."),
                _ => version.starts_with("3."),
            };
            if !verified {
                out.push(format!(
                    "route-call-coverage: {} depends on {name} {version}, outside the versions checked against a live server (reqwest 0.12/0.13, ureq 3); its URLs were read with the same rules",
                    m.name
                ));
            }
        }
    }
    for (lib, n) in &c.unmodelled {
        out.push(format!(
            "route-call-coverage: {n} use(s) of {lib}, an HTTP client rustograph does not model; their requests are not reported"
        ));
    }
    if c.unproven > 0 {
        out.push(format!(
            "route-call-coverage: {} request-shaped call(s) are sent from a receiver that is not a proven reqwest::Client or ureq::Agent; they are not reported",
            c.unproven
        ));
    }
    if c.unrequestable > 0 {
        out.push(format!(
            "route-call-coverage: {} call(s) pass a relative URL with no base or a non-http(s) URL, which the client rejects before sending; they are not reported",
            c.unrequestable
        ));
    }
    let ambiguous = c
        .calls
        .iter()
        .filter(|s| {
            matches!(
                s.outcome,
                Outcome::Dynamic {
                    ambiguous: true,
                    ..
                }
            )
        })
        .count();
    if ambiguous > 0 {
        out.push(format!("ambiguous-base-join: {ambiguous} call(s) glue a relative path to a base URL whose path is unknown; they are dynamic"));
    }
    if c.undeclared > 0 {
        out.push(format!("http-wrapper-undeclared: {} dynamic call(s) build their URL or verb from a parameter of an undeclared function; declare it in an http-wrappers file to resolve its callers", c.undeclared));
    }
    out
}

/// 선언된 래퍼의 공백 — 정점에 닿지 않거나 호출이 0건.
fn wrapper_gaps(sh: &Shared, wrappers: &[Wrapper], c: &Collected) -> Vec<String> {
    let mut out = Vec::new();
    for (i, w) in wrappers.iter().enumerate() {
        let calls = c.wrapper_calls.get(&i).copied().unwrap_or(0);
        let target = format!("{}::{}", w.owner, w.name);
        if !sh.wrapper_resolved(i) {
            out.push(format!("http-wrapper-unresolved: wrappers[{}] ({target}) does not name a rustograph function, method or struct", w.position));
        } else if calls == 0 {
            out.push(format!(
                "http-wrapper-unresolved: wrappers[{}] ({target}) has no calls",
                w.position
            ));
        }
    }
    out
}

/// 호출을 사실로 바꾸고 정렬·중복 제거·문법 검사를 한다. (사실, usr 없는 수).
fn facts(
    ctx: &Ctx,
    calls: &[CallSite],
    limitations: &mut Vec<String>,
) -> Result<(Vec<CallFact>, usize), String> {
    let mut out = Vec::new();
    let mut missing = 0;
    let mut unlocated = 0;
    for c in calls {
        let Some(location) = ctx.locate_utf8(&c.loc) else {
            unlocated += 1;
            continue;
        };
        let symbol = ctx.owner_of(&c.loc).map(|id| FactSymbol {
            qualified_name: id.clone(),
            usr: id,
        });
        if symbol.is_none() {
            missing += 1;
        }
        out.push(to_fact(c, location, symbol)?);
    }
    if unlocated > 0 {
        limitations.push(format!(
            "route-call-coverage: {unlocated} route call(s) had no source location and were not emitted"
        ));
    }
    out.sort_by_key(fact_key);
    out.dedup_by(|a, b| fact_key(a) == fact_key(b));
    Ok((out, missing))
}

/// 호출 하나를 계약 사실로 쓴다. 정적 템플릿이 문법을 어기면 생산자 결함이다.
fn to_fact(
    c: &CallSite,
    location: BridgeLocation,
    symbol: Option<FactSymbol>,
) -> Result<CallFact, String> {
    let mut fact = CallFact {
        kind: "route-call",
        method: c.method.clone(),
        method_dynamic: c.method.is_none().then_some(true),
        channel: None,
        dynamic: true,
        path_anchor: PathAnchor::Base.as_str(),
        authority: None,
        base_ref: c.base_ref.clone(),
        service: c.service.clone(),
        query_tail_stripped: None,
        channel_prefix: None,
        masked_segments: None,
        location,
        symbol,
    };
    match &c.outcome {
        Outcome::Template(t) => {
            if let Some(problem) = template_problem(&t.template) {
                return Err(format!("internal error: route-call template {:?} is not canonical ({problem}); please report this with the call source", t.template));
            }
            fact.channel = Some(t.template.clone());
            fact.dynamic = false;
            fact.path_anchor = t.anchor.as_str();
            fact.authority = t.authority.clone();
            fact.query_tail_stripped = t.query_tail_stripped.then_some(true);
            fact.masked_segments = (t.masked_segments > 0).then_some(t.masked_segments);
        }
        Outcome::Dynamic {
            prefix,
            anchor,
            masked_segments,
            ..
        } => {
            fact.path_anchor = anchor.as_str();
            fact.channel_prefix = prefix.clone();
            fact.masked_segments =
                (*masked_segments > 0 && prefix.is_some()).then_some(*masked_segments);
        }
        Outcome::Unrequestable => {
            return Err("internal error: an unrequestable call reached fact assembly".into())
        }
    }
    Ok(fact)
}

/// 사실의 결정적 정렬 키.
fn fact_key(f: &CallFact) -> (String, u32, u32, Option<String>, Option<String>, bool) {
    (
        f.location.path.clone(),
        f.location.line,
        f.location.column,
        f.method.clone(),
        f.channel.clone(),
        f.dynamic,
    )
}
