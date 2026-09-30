//! isthmus `bridge-facts` v1 http 도메인의 서버 측 생산자(`rustograph routes
//! --role server`) — axum·actix-web 라우트 선언을 `route-decl` 사실로 낸다.
//!
//! 계약의 정본은 ../isthmus의 docs/GRAPH-EXCHANGE.md "HTTP 경계"다. 프레임워크
//! 규칙과 공식 소스 근거는 docs/HTTP-ROUTES.md에 있다. 흐름:
//!
//! 1. `cargo metadata` 해석 결과로 멤버 크레이트별 axum·actix-web 버전을 찾는다
//!    (Cargo.lock이 고정한 버전 = resolve의 패키지 버전).
//! 2. `impact`와 같은 syn 수확을 돌려 모듈 트리·AST·정점 집합을 얻는다 —
//!    핸들러 usr가 그래프 정점 ID와 같아야 `reach`로 이어진다.
//! 3. 프레임워크 추출기가 선언([`common::Decl`])과 공백([`common::Gap`])을 낸다.
//! 4. 이 모듈이 템플릿을 렌더링하고 usr·위치·스코프를 붙인 뒤 스스로 계약
//!    검사(`order`·스코프·템플릿 문법)를 통과한 문서만 낸다.

mod actix;
mod axum;
mod common;
mod pattern;
pub mod template;
pub mod validate;

use crate::cargo_meta;
use crate::source::schema::{rfc3339_utc_now, BridgeFactsTool, BridgeLocation, FactSymbol};
use common::{Anchor, Ctx, Decl, DeclPath, Gap, Handler, Output, ScopeSpec, Trailing};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use template::{constraints, render, template_problem, Seg};

/// 분석할 프레임워크다.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Framework {
    Axum,
    Actix,
}

impl Framework {
    /// `--framework` 값.
    pub fn parse(s: &str) -> Option<Framework> {
        match s {
            "axum" => Some(Framework::Axum),
            "actix" | "actix-web" => Some(Framework::Actix),
            _ => None,
        }
    }
}

/// `routes` 옵션.
#[derive(Debug, Default)]
pub struct RouteOptions {
    /// 워크스페이스에 두 프레임워크가 모두 있을 때 고른 하나.
    pub framework: Option<Framework>,
}

/// http 문서의 route-decl 사실이다. 키 순서는 계약 문서 나열 순서를 따른다.
#[derive(Serialize)]
pub struct RouteFact {
    pub kind: &'static str,
    pub method: String,
    pub channel: Option<String>,
    pub dynamic: bool,
    #[serde(rename = "pathAnchor")]
    pub path_anchor: &'static str,
    #[serde(rename = "trailingSlash", skip_serializing_if = "Option::is_none")]
    pub trailing_slash: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub narrowed: Option<bool>,
    #[serde(rename = "paramConstraints", skip_serializing_if = "Vec::is_empty")]
    pub param_constraints: Vec<ParamConstraint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<Order>,
    pub location: BridgeLocation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<FactSymbol>,
}

/// `paramConstraints` 항목.
#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct ParamConstraint {
    pub segment: usize,
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pattern: Option<String>,
}

/// registration-order 문서의 `order`.
#[derive(Serialize, Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Order {
    pub group: String,
    pub index: u64,
}

/// http `limitationScopes` 항목.
#[derive(Serialize, Debug, Clone)]
pub struct LimitationScope {
    #[serde(rename = "limitationIndex")]
    pub limitation_index: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub templates: Vec<String>,
    #[serde(rename = "templatePrefixes", skip_serializing_if = "Vec::is_empty")]
    pub template_prefixes: Vec<String>,
    #[serde(rename = "templateSuffixes", skip_serializing_if = "Vec::is_empty")]
    pub template_suffixes: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub methods: Vec<String>,
}

/// 문서의 `sourceSets`.
#[derive(Serialize, Debug)]
pub struct SourceSets {
    pub tests: &'static str,
}

/// isthmus bridge-facts v1 http 서버 문서다.
#[derive(Serialize)]
pub struct RoutesDocument {
    pub format: &'static str,
    pub version: u8,
    pub tool: BridgeFactsTool,
    #[serde(rename = "generatedAt")]
    pub generated_at: String,
    pub platform: &'static str,
    /// roles가 있는 http 문서는 사실 0건이어도 `http`다(계약의 http 예외).
    pub target: &'static str,
    pub project: String,
    pub roles: Vec<&'static str>,
    pub dispatch: &'static str,
    #[serde(rename = "sourceSets")]
    pub source_sets: SourceSets,
    pub facts: Vec<RouteFact>,
    pub limitations: Vec<String>,
    #[serde(rename = "limitationScopes", skip_serializing_if = "Vec::is_empty")]
    pub limitation_scopes: Vec<LimitationScope>,
}

/// 멤버 크레이트 하나의 프레임워크 사용.
struct Use {
    /// 수확 트리의 루트 모듈 이름(lib·bin 타깃 이름).
    roots: Vec<String>,
    framework: Framework,
    version: String,
}

/// 워크스페이스의 axum·actix-web 라우트를 http 서버 문서로 낸다.
///
/// 두 프레임워크가 모두 있는데 `--framework`가 없으면 사용법 오류다 — 문서
/// 하나는 dispatch 하나만 선언할 수 있다.
pub fn routes(
    dir: &Path,
    tool_version: &str,
    opts: &RouteOptions,
) -> Result<RoutesDocument, String> {
    let root = dir
        .canonicalize()
        .map_err(|e| format!("cannot resolve {}: {e}", dir.display()))?;
    let meta = cargo_meta::load(dir)?;
    let uses = framework_uses(&meta);
    let present: BTreeSet<Framework> = uses.iter().map(|u| u.framework).collect();
    let framework = match (opts.framework, present.len()) {
        (Some(f), _) => Some(f),
        (None, 0) => None,
        (None, 1) => present.iter().next().copied(),
        (None, _) => {
            return Err(
                "the workspace depends on both axum and actix-web; pass --framework axum or --framework actix"
                    .to_string(),
            )
        }
    };
    let mut out = Output::default();
    let dispatch = match framework {
        Some(Framework::Actix) => "registration-order",
        _ => "specificity",
    };
    match framework {
        None => out.gap(
            "route-coverage:",
            "no workspace member depends on axum or actix-web, so no routes were extracted"
                .to_string(),
        ),
        Some(f) => {
            let parts = crate::source::harvest_parts(dir, &meta)?;
            let ctx = Ctx::new(&parts, root.clone());
            for u in uses.iter().filter(|u| u.framework == f) {
                extract_use(&ctx, u, &mut out);
            }
            if !uses.iter().any(|u| u.framework == f) {
                out.gap(
                    "route-coverage:",
                    format!(
                        "--framework {f:?} was requested but no workspace member depends on it"
                    ),
                );
            }
            return assemble(&ctx, out, dispatch, tool_version, &root, &meta);
        }
    }
    finish(out, dispatch, tool_version, &root, &meta, Vec::new())
}

/// 멤버 하나의 루트들에서 추출한다.
fn extract_use(ctx: &Ctx, u: &Use, out: &mut Output) {
    match u.framework {
        Framework::Axum => {
            let version = match u.version.split('.').collect::<Vec<_>>().as_slice() {
                ["0", "7", ..] => axum::Version::V07,
                ["0", "8", ..] => axum::Version::V08,
                ["0", "6", ..] => {
                    version_gap(out, "axum", &u.version, "0.7 path syntax");
                    axum::Version::V07
                }
                _ => {
                    version_gap(out, "axum", &u.version, "0.8 path syntax");
                    axum::Version::V08
                }
            };
            for r in &u.roots {
                axum::extract(ctx, r, version, out);
            }
        }
        Framework::Actix => {
            if !u.version.starts_with("4.") {
                version_gap(out, "actix-web", &u.version, "actix-web 4 rules");
            }
            for r in &u.roots {
                actix::extract(ctx, r, out);
            }
        }
    }
}

/// 확인한 메이저 밖의 버전 — 규칙이 다를 수 있다는 서버 측 공백.
fn version_gap(out: &mut Output, name: &str, version: &str, assumed: &str) {
    out.gap(
        "route-framework-version-unknown:",
        format!(
            "{name} {version} is outside the verified versions; routes were read with {assumed}"
        ),
    );
}

/// 멤버 크레이트가 직접 의존하는 axum·actix-web과 그 해석 버전이다.
fn framework_uses(meta: &cargo_meta::Metadata) -> Vec<Use> {
    let mut out = Vec::new();
    for d in meta.dep_edges.iter().filter(|d| d.kind.is_empty()) {
        let (Some(&fi), Some(&ti)) = (meta.by_id.get(&d.from), meta.by_id.get(&d.to)) else {
            continue;
        };
        let (from, to) = (&meta.packages[fi], &meta.packages[ti]);
        if !from.workspace_member {
            continue;
        }
        let framework = match to.name.as_str() {
            "axum" => Framework::Axum,
            "actix_web" => Framework::Actix,
            _ => continue,
        };
        let roots: Vec<String> = from
            .targets
            .iter()
            .filter(|t| matches!(t.kind.as_str(), "lib" | "bin"))
            .map(|t| t.name.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        out.push(Use {
            roots,
            framework,
            version: to.version.clone(),
        });
    }
    out
}

/// 선언을 사실로, 공백을 한계로 조립하고 스스로 계약 검사를 한다.
fn assemble(
    ctx: &Ctx,
    out: Output,
    dispatch: &'static str,
    tool_version: &str,
    root: &Path,
    meta: &cargo_meta::Metadata,
) -> Result<RoutesDocument, String> {
    let mut facts = Vec::new();
    let mut gaps = out.gaps;
    let mut closures = 0usize;
    let mut unknown = 0usize;
    let mut unlocated = 0usize;
    // 같은 라우터가 두 번 서빙되거나 두 루트가 같은 함수를 부르면 같은 선언이
    // 거듭 나온다 — 한 번만 센다.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for d in &out.decls {
        if !seen.insert(decl_key(d)) {
            continue;
        }
        let Some(location) = ctx.locate(&d.loc) else {
            unlocated += 1;
            continue;
        };
        let symbol = match &d.handler {
            Handler::Usr(id) => Some(id.clone()),
            // 감싸는 정점을 찾은 클로저만 근사 usr 한계로 센다 — 못 찾으면 usr가 없는
            // 핸들러와 같다.
            Handler::Closure(l) => match ctx.owner_of(l) {
                Some(id) => {
                    closures += 1;
                    Some(id)
                }
                None => {
                    unknown += 1;
                    None
                }
            },
            Handler::Unknown => {
                unknown += 1;
                None
            }
        }
        .map(|id| FactSymbol {
            qualified_name: id.clone(),
            usr: id,
        });
        for fact in decl_facts(d, dispatch, &location, symbol.as_ref()) {
            facts.push(fact);
        }
    }
    if closures > 0 {
        gaps.push(Gap {
            prefix: "missing-route-usrs:",
            text: format!("{closures} route handler(s) are closures; their symbol.usr names the enclosing function, so forward reach from those routes over-approximates"),
            scope: None,
        });
    }
    if unknown > 0 {
        gaps.push(Gap {
            prefix: "missing-route-usrs:",
            text: format!("{unknown} route declaration(s) have a handler that is not a workspace function (a tower service, a redirect, or an unresolved path); they carry no symbol"),
            scope: None,
        });
    }
    if unlocated > 0 {
        gaps.push(Gap {
            prefix: "route-coverage:",
            text: format!(
                "{unlocated} route declaration(s) had no source location and were not emitted"
            ),
            scope: None,
        });
    }
    finish(
        Output {
            decls: Vec::new(),
            gaps,
        },
        dispatch,
        tool_version,
        root,
        meta,
        facts,
    )
}

/// 사실·한계를 정렬하고 문서를 만든 뒤 계약 검사를 한다.
fn finish(
    out: Output,
    dispatch: &'static str,
    tool_version: &str,
    root: &Path,
    meta: &cargo_meta::Metadata,
    mut facts: Vec<RouteFact>,
) -> Result<RoutesDocument, String> {
    facts.sort_by(fact_key);
    facts.dedup_by(|a, b| fact_key(a, b).is_eq());
    let (limitations, limitation_scopes) = limitations(out.gaps, &meta.limitations);
    let doc = RoutesDocument {
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
        roles: vec!["server"],
        dispatch,
        source_sets: SourceSets { tests: "excluded" },
        facts,
        limitations,
        limitation_scopes,
    };
    self_check(&doc)?;
    Ok(doc)
}

/// 선언 하나를 method·변형별 사실로 편다.
fn decl_facts(
    d: &Decl,
    dispatch: &str,
    location: &BridgeLocation,
    symbol: Option<&FactSymbol>,
) -> Vec<RouteFact> {
    let mut variants: Vec<(Option<String>, Vec<ParamConstraint>, Option<&'static str>)> =
        Vec::new();
    let dynamic = match &d.path {
        DeclPath::Template { segs, empty_tail } => {
            let slash = trailing_str(d.trailing);
            variants.push((Some(render(segs)), param_constraints(segs), slash));
            if let Some(tail) = empty_tail {
                // 빈 값 변형 — catch-all 자리를 빈 세그먼트로 채운 정적 템플릿이다.
                let mut empty = segs.clone();
                if let Some(last) = empty.last_mut() {
                    *last = Seg::Lit(String::new());
                }
                variants.push((
                    Some(render(&empty)),
                    param_constraints(&empty),
                    trailing_str(*tail),
                ));
            }
            false
        }
        DeclPath::Dynamic(text) => {
            variants.push((Some(text.clone()), Vec::new(), None));
            true
        }
    };
    let order = if dispatch == "registration-order" {
        d.order.as_ref().map(|(g, i)| Order {
            group: g.clone(),
            index: *i,
        })
    } else {
        None
    };
    let mut out = Vec::new();
    for method in &d.methods {
        for (channel, pc, slash) in &variants {
            out.push(RouteFact {
                kind: "route-decl",
                method: method.clone(),
                channel: channel.clone(),
                dynamic,
                path_anchor: match d.anchor {
                    Anchor::Root => "root",
                    Anchor::Base => "base",
                },
                trailing_slash: *slash,
                narrowed: d.narrowed.then_some(true),
                param_constraints: pc.clone(),
                order: order.clone(),
                location: location.clone(),
                symbol: symbol.cloned(),
            });
        }
    }
    out
}

/// 선언 중복 판정 키 — 위치·method·경로·앵커·순서·핸들러.
fn decl_key(d: &Decl) -> String {
    let path = match &d.path {
        DeclPath::Template { segs, .. } => render(segs),
        DeclPath::Dynamic(t) => format!("dyn:{t}"),
    };
    format!(
        "{}|{:?}|{:?}|{path}|{:?}|{:?}|{:?}",
        d.loc.file.display(),
        d.loc.span.byte_range(),
        d.methods,
        d.anchor,
        d.order,
        d.handler
    )
}

/// 끝 슬래시 판정의 계약 값.
fn trailing_str(t: Trailing) -> Option<&'static str> {
    match t {
        Trailing::Strict => Some("strict"),
        Trailing::Optional => Some("optional"),
        Trailing::Unknown => None,
    }
}

/// 세그먼트의 `paramConstraints`.
fn param_constraints(segs: &[Seg]) -> Vec<ParamConstraint> {
    constraints(segs)
        .into_iter()
        .map(|(segment, c)| ParamConstraint {
            segment,
            kind: c.kind(),
            pattern: match c {
                template::Constraint::Regex(p) => Some(p),
                _ => None,
            },
        })
        .collect()
}

/// 사실의 결정적 정렬 키.
fn fact_key(a: &RouteFact, b: &RouteFact) -> std::cmp::Ordering {
    let key = |f: &RouteFact| {
        (
            f.channel.clone(),
            f.method.clone(),
            f.path_anchor,
            f.order.clone(),
            f.location.path.clone(),
            f.location.line,
            f.location.column,
            f.symbol.as_ref().map(|s| s.usr.clone()),
            f.narrowed,
        )
    };
    key(a).cmp(&key(b))
}

/// 공백을 한계 문장과 스코프로 바꾼다. 같은 문장·스코프는 하나로 합친다. 스코프가
/// 계약 검사를 통과하지 못하면 스코프 없이(문서 전체 효과로) 남긴다.
fn limitations(gaps: Vec<Gap>, meta_limits: &[String]) -> (Vec<String>, Vec<LimitationScope>) {
    let mut seen: BTreeMap<(String, Option<ScopeSpec>), ()> = BTreeMap::new();
    let mut ordered: Vec<(String, Option<ScopeSpec>)> = Vec::new();
    for g in gaps {
        let text = format!("{} {}", g.prefix, g.text);
        let key = (text, g.scope);
        if seen.insert(key.clone(), ()).is_none() {
            ordered.push(key);
        }
    }
    ordered.sort();
    let mut lines: Vec<String> = Vec::new();
    let mut scopes: Vec<LimitationScope> = Vec::new();
    for (text, scope) in ordered {
        let index = lines.len();
        lines.push(text);
        if let Some(s) = scope {
            let entry = LimitationScope {
                limitation_index: index,
                templates: s.templates,
                template_prefixes: s.prefixes,
                template_suffixes: s.suffixes,
                methods: s.methods,
            };
            let value = serde_json::to_value(&entry).unwrap_or_default();
            if validate::scope_problem(&value).is_none() {
                scopes.push(entry);
            }
        }
    }
    // cargo metadata 한계(resolve 없음 등)는 버전 판정의 근거가 약해진다는 뜻이다.
    for l in meta_limits {
        lines.push(format!("route-framework-version-unknown: {l}"));
    }
    (lines, scopes)
}

/// 내기 전에 템플릿 문법·order 규칙을 소비자와 같은 검사로 확인한다. 실패는
/// 생산자 결함이므로 문서를 내지 않고 오류로 보고한다.
fn self_check(doc: &RoutesDocument) -> Result<(), String> {
    for (i, f) in doc.facts.iter().enumerate() {
        if f.dynamic {
            continue;
        }
        let Some(channel) = &f.channel else { continue };
        if let Some(problem) = template_problem(channel) {
            return Err(format!(
                "internal error: route template {channel:?} at fact {i} is not canonical ({problem}); please report this with the route source"
            ));
        }
    }
    let value = serde_json::to_value(doc).map_err(|e| format!("internal error: {e}"))?;
    if let Some(problem) = validate::order_problem(&value) {
        return Err(format!(
            "internal error: the routes document breaks the isthmus order contract ({problem}); please report this"
        ));
    }
    Ok(())
}
