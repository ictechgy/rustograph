//! `rustograph routes --role server` — isthmus 공유 벡터, 오라클 기록 대조, 추출 규칙.

use rustograph::source::routes::{self, template, validate, Framework, RouteOptions};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// fixture 하나의 routes 문서(JSON).
fn doc_of(dir: &Path, framework: Option<Framework>) -> Value {
    let d = routes::routes(dir, "test", &RouteOptions { framework }).expect("routes extraction");
    serde_json::to_value(&d).expect("serializes")
}

fn load(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("readable")).expect("json")
}

// ── 공유 벡터 ──────────────────────────────────────────────

/// 실행하는 생산자 ruleId.
const APPLICABLE: &[&str] = &[
    "template.grammar",
    "template.normalize",
    "dispatch.validate",
    "scope.validate",
];

/// 건너뛰는 ruleId 접두사 — 새 ruleId가 오면 테스트가 판단을 요구한다.
const SKIPPED: &[&str] = &[
    "match.",
    "dispatch.match",
    "dispatch.shadow",
    "scope.applies",
    "framework.openapi.",
    "framework.spring.",
    // 클라이언트 조립 규칙 — tests/client_routes.rs가 실행한다.
    "compose.",
    "wrapper.",
];

fn cases() -> Vec<Value> {
    let mut out = Vec::new();
    let mut names: Vec<PathBuf> = std::fs::read_dir(repo().join("conformance"))
        .expect("conformance dir")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    names.sort();
    for p in names {
        let suite = load(&p);
        out.extend(suite["cases"].as_array().expect("cases").iter().cloned());
    }
    out
}

fn applicable() -> Vec<Value> {
    cases()
        .into_iter()
        .filter(|c| {
            APPLICABLE.contains(&c["ruleId"].as_str().unwrap_or(""))
                && c["appliesTo"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|t| t == "producer"))
        })
        .collect()
}

#[test]
fn vendored_vectors_match_sums_and_lock() {
    let dir = repo().join("conformance");
    let sums: std::collections::BTreeMap<String, String> =
        std::fs::read_to_string(dir.join("SHA256SUMS"))
            .expect("sums")
            .lines()
            .filter(|l| !l.is_empty())
            .map(|l| {
                let (h, n) = l.split_once("  ").expect("sha256sum format");
                (n.to_string(), h.to_string())
            })
            .collect();
    let lock = load(&repo().join("conformance.lock"));
    let locked: BTreeSet<String> = lock["files"]
        .as_object()
        .expect("files")
        .keys()
        .cloned()
        .collect();
    let mut vectors: Vec<String> = std::fs::read_dir(&dir)
        .expect("dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".json"))
        .collect();
    vectors.sort();
    assert_eq!(sums.keys().cloned().collect::<Vec<_>>(), vectors);
    assert_eq!(locked.into_iter().collect::<Vec<_>>(), vectors);
    assert_eq!(lock["commit"].as_str().map(str::len), Some(40));
    for n in &vectors {
        let bytes = std::fs::read(dir.join(n)).expect("vector");
        let digest = rustograph::traversal::sha256::hex_digest(&bytes);
        assert_eq!(digest, sums[n], "{n}: re-vendor it from isthmus");
        assert_eq!(
            Some(digest.as_str()),
            lock["files"][n].as_str(),
            "{n}: lock"
        );
    }
}

#[test]
fn every_case_is_applicable_or_classified() {
    let unknown: Vec<String> = cases()
        .iter()
        .filter(|c| {
            let rule = c["ruleId"].as_str().unwrap_or("");
            !APPLICABLE.contains(&rule) && !SKIPPED.iter().any(|p| rule.starts_with(p))
        })
        .map(|c| c["id"].to_string())
        .collect();
    assert!(unknown.is_empty(), "classify new vector cases: {unknown:?}");
}

#[test]
fn producer_cases_pass() {
    let cases = applicable();
    // 조용히 줄지 않게 — 벡터 재벤더링 때 수를 확인한다.
    assert_eq!(cases.len(), 26 + 7 + 18 + 9);
    for c in &cases {
        let id = c["id"].as_str().unwrap_or("");
        let input = &c["input"];
        let expect = &c["expect"];
        match c["ruleId"].as_str().unwrap_or("") {
            "template.grammar" => {
                let problem = template::template_problem(input["template"].as_str().unwrap());
                assert_eq!(problem.is_none(), expect["valid"] == true, "{id}");
                if let Some(reason) = expect["reason"].as_str() {
                    assert_eq!(problem, Some(reason), "{id}");
                }
            }
            "template.normalize" => assert_eq!(
                template::normalize_uri_path(input["path"].as_str().unwrap()),
                expect["template"].as_str().unwrap(),
                "{id}"
            ),
            "dispatch.validate" => {
                // isthmus 참조 실행기와 같게 빠진 위치·kind를 채운다.
                let mut doc = input["document"].clone();
                let facts = doc["facts"].as_array_mut().unwrap();
                for (i, f) in facts.iter_mut().enumerate() {
                    let line = f.pointer("/location/line").cloned().unwrap_or(json!(i + 1));
                    let column = f.pointer("/location/column").cloned().unwrap_or(json!(1));
                    f["location"] = json!({"path": "shop/urls.py", "line": line, "column": column});
                }
                let valid = validate::order_problem(&doc).is_none();
                assert_eq!(valid, expect["valid"] == true, "{id}");
            }
            "scope.validate" => {
                let mut scope = input["scope"].clone();
                scope["limitationIndex"] = json!(0);
                assert_eq!(
                    validate::scope_problem(&scope).is_none(),
                    expect["valid"] == true,
                    "{id}"
                );
            }
            other => panic!("unexpected rule {other}"),
        }
    }
}

// ── 오라클 기록 대조 ────────────────────────────────────────

/// 오라클 기록과 같은 모양의 사실 키.
fn fact_key(f: &Value) -> Value {
    let mut k = json!({
        "method": f["method"],
        "channel": f["channel"],
        "pathAnchor": f["pathAnchor"],
    });
    if let Some(t) = f.get("trailingSlash") {
        k["trailingSlash"] = t.clone();
    }
    if let Some(o) = f.get("order") {
        k["order"] = json!([o["group"], o["index"]]);
    }
    if let Some(u) = f.pointer("/symbol/usr") {
        k["usr"] = u.clone();
    }
    if f.get("narrowed").is_some() {
        k["narrowed"] = json!(true);
    }
    if let Some(pc) = f["paramConstraints"].as_array() {
        k["paramConstraints"] = pc
            .iter()
            .map(|c| {
                json!([
                    c["segment"],
                    c["kind"],
                    c.get("pattern").cloned().unwrap_or(Value::Null)
                ])
            })
            .collect();
    }
    k
}

/// 지금 출력의 정적 사실이 오라클이 실제 요청으로 확인한 사실(과 요청할 수 없는
/// base 사실)과 정확히 같아야 한다. 다시 기록하려면 experiments/routes-oracle/run_all.sh.
fn check_fixture(name: &str) {
    let doc = doc_of(&repo().join("tests/fixture-routes").join(name), None);
    assert!(validate::order_problem(&doc).is_none());
    let now: BTreeSet<String> = doc["facts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["dynamic"] == false)
        .map(|f| fact_key(f).to_string())
        .collect();
    let rec = load(&repo().join(format!("experiments/routes-oracle/recorded/{name}.json")));
    for t in ["precision", "recall", "negatives", "trailingSlash"] {
        assert_eq!(rec[t]["passed"], rec[t]["total"], "{name} {t} is not 100%");
        assert!(
            rec[t]["total"].as_u64().unwrap() > 0,
            "{name} {t} probed nothing"
        );
    }
    assert!(rec["failedFacts"].as_array().unwrap().is_empty());
    let mut recorded: BTreeSet<String> = rec["verifiedFacts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.to_string())
        .collect();
    recorded.extend(
        rec["unprobed"]
            .as_array()
            .unwrap()
            .iter()
            .map(|u| u[0].to_string()),
    );
    // 키 순서를 정규화한다(serde_json Map은 정렬된 맵이라 다시 쓰면 같은 바이트).
    let normalize = |s: &BTreeSet<String>| -> BTreeSet<String> {
        s.iter()
            .map(|x| serde_json::from_str::<Value>(x).unwrap().to_string())
            .collect()
    };
    assert_eq!(
        normalize(&now),
        normalize(&recorded),
        "{name}: re-run the oracle"
    );
}

#[test]
fn axum08_matches_oracle() {
    check_fixture("axum08");
}

#[test]
fn axum07_matches_oracle() {
    check_fixture("axum07");
}

#[test]
fn actix_matches_oracle() {
    check_fixture("actix");
}

// ── 추출 규칙(임시 크레이트) ────────────────────────────────

/// 임시 워크스페이스 — 스텁 의존성의 이름·버전과 소스 파일들.
fn temp_crate(tag: &str, deps: &[(&str, &str)], files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rg-routes-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("app/src")).unwrap();
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
        dep_lines.push_str(&format!("{name} = {{ path = \"../stub-{name}\" }}\n"));
    }
    std::fs::write(
        dir.join("Cargo.toml"),
        "[workspace]\nmembers = [\"app\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("app/Cargo.toml"),
        format!("[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[dependencies]\n{dep_lines}"),
    )
    .unwrap();
    for (path, text) in files {
        std::fs::write(dir.join("app/src").join(path), text).unwrap();
    }
    dir
}

fn facts(doc: &Value) -> Vec<(String, String, Option<String>)> {
    doc["facts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            (
                f["method"].as_str().unwrap().to_string(),
                f["channel"].as_str().unwrap_or("").to_string(),
                f["trailingSlash"].as_str().map(str::to_string),
            )
        })
        .collect()
}

fn limitations(doc: &Value) -> Vec<String> {
    doc["limitations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap().to_string())
        .collect()
}

/// NormalizePathLayer는 Router 바깥을 감쌀 때만 라우팅 전에 동작한다 — Router::layer
/// 인자로만 쓰면 끝 슬래시는 엄격하다(axum layer.md:57-62).
#[test]
fn axum_normalize_path_only_counts_outside_the_router() {
    let src_outer = r#"
use axum::{routing::get, Router, ServiceExt};
use tower_http::normalize_path::NormalizePathLayer;
use tower::Layer;
async fn h() {}
pub fn app() -> Router { Router::new().route("/items", get(h)).route("/dir/", get(h)) }
pub async fn run() {
    let svc = NormalizePathLayer::trim_trailing_slash().layer(app());
    axum::serve(listener(), ServiceExt::<axum::extract::Request>::into_make_service(svc)).await;
    axum::serve(listener(), app()).await;
}
"#;
    let dir = temp_crate("norm", &[("axum", "0.8.9")], &[("lib.rs", src_outer)]);
    let d = doc_of(&dir, None);
    let f = facts(&d);
    assert!(
        f.contains(&("GET".into(), "/items".into(), Some("optional".into()))),
        "{f:?}"
    );
    assert!(
        f.contains(&("GET".into(), "/dir/".into(), Some("strict".into()))),
        "{f:?}"
    );
    let src_inner = r#"
use axum::{routing::get, Router};
use tower_http::normalize_path::NormalizePathLayer;
async fn h() {}
pub fn app() -> Router {
    Router::new().route("/items", get(h)).layer(NormalizePathLayer::trim_trailing_slash())
}
pub async fn run() { axum::serve(listener(), app()).await; }
"#;
    std::fs::write(dir.join("app/src/lib.rs"), src_inner).unwrap();
    let d = doc_of(&dir, None);
    assert!(facts(&d).contains(&("GET".into(), "/items".into(), Some("strict".into()))));
    // MethodRouter::layer도 라우팅 뒤다. 변수로 넘긴 레이어는 모양을 못 읽어 효과 없음.
    let src_method = r#"
use axum::{routing::get, Router};
use tower_http::normalize_path::NormalizePathLayer;
async fn h() {}
pub fn app() -> Router {
    let norm = NormalizePathLayer::trim_trailing_slash();
    Router::new()
        .route("/items", get(h).layer(NormalizePathLayer::trim_trailing_slash()))
        .route("/other", get(h))
        .layer(norm)
}
pub async fn run() { axum::serve(listener(), app()).await; }
"#;
    std::fs::write(dir.join("app/src/lib.rs"), src_method).unwrap();
    let d = doc_of(&dir, None);
    let f = facts(&d);
    assert!(
        f.contains(&("GET".into(), "/items".into(), Some("strict".into()))),
        "{f:?}"
    );
    assert!(
        f.contains(&("GET".into(), "/other".into(), Some("strict".into()))),
        "{f:?}"
    );
    // ServiceBuilder로 감싼 서비스는 라우팅 전이다.
    let src_builder = r#"
use axum::{routing::get, Router};
use tower_http::normalize_path::NormalizePathLayer;
async fn h() {}
pub fn app() -> Router { Router::new().route("/items", get(h)) }
pub async fn run() {
    let svc = tower::ServiceBuilder::new().layer(NormalizePathLayer::trim_trailing_slash()).service(app());
    serve_somehow(svc);
    axum::serve(listener(), app()).await;
}
"#;
    std::fs::write(dir.join("app/src/lib.rs"), src_builder).unwrap();
    let d = doc_of(&dir, None);
    assert!(facts(&d).contains(&("GET".into(), "/items".into(), Some("optional".into()))));
    let _ = std::fs::remove_dir_all(&dir);
}

/// 평가하지 못한 nest·동적 경로는 한계와 dynamic 사실로 남는다(조용히 버리지 않는다).
#[test]
fn axum_unknown_parts_are_limitations() {
    let src = r#"
use axum::{routing::get, Router};
async fn h() {}
pub fn app(prefix: &str) -> Router {
    Router::new()
        .route(prefix, get(h))
        .nest("/ext", external::router())
        .route("/ok", get(h))
}
pub async fn run() { axum::serve(listener(), app("/x")).await; }
"#;
    let dir = temp_crate("unknown", &[("axum", "0.8.9")], &[("lib.rs", src)]);
    let d = doc_of(&dir, None);
    let _ = std::fs::remove_dir_all(&dir);
    let dynamic = d["facts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["dynamic"] == true)
        .count();
    assert_eq!(dynamic, 1);
    let lims = limitations(&d);
    assert!(
        lims.iter()
            .any(|l| l.starts_with("route-coverage: a nested router")),
        "{lims:?}"
    );
    let scope = d["limitationScopes"].as_array().unwrap();
    assert!(
        scope
            .iter()
            .any(|s| s["templatePrefixes"] == json!(["/ext"])),
        "{scope:?}"
    );
}

/// 두 프레임워크가 모두 있으면 추측하지 않고 사용법 오류, 없으면 사실 0건과 한계.
#[test]
fn framework_choice_is_explicit() {
    let dir = temp_crate(
        "both",
        &[("axum", "0.8.9"), ("actix-web", "4.15.0")],
        &[("lib.rs", "")],
    );
    let err = routes::routes(&dir, "test", &RouteOptions::default())
        .err()
        .expect("usage error");
    assert!(err.contains("--framework"), "{err}");
    let d = doc_of(&dir, Some(Framework::Actix));
    assert_eq!(d["dispatch"], "registration-order");
    let _ = std::fs::remove_dir_all(&dir);
    let none = temp_crate("none", &[], &[("lib.rs", "pub fn f() {}")]);
    let d = doc_of(&none, None);
    let _ = std::fs::remove_dir_all(&none);
    assert_eq!(d["target"], "http");
    assert!(d["facts"].as_array().unwrap().is_empty());
    assert!(limitations(&d)[0].starts_with("route-coverage: no workspace member"));
}

/// actix 설정 클로저·설정 함수 위임·알 수 없는 서비스·App 기본 서비스.
#[test]
fn actix_configure_and_unknown_services() {
    let src = r#"
use actix_web::{web, App};
async fn h() -> &'static str { "" }
fn nested(cfg: &mut web::ServiceConfig) { cfg.route("/deep", web::put().to(h)); }
fn outer(cfg: &mut web::ServiceConfig) { nested(cfg); }
pub fn build() {
    let _ = App::new()
        .configure(|cfg| { cfg.service(web::resource("/c").to(h)); })
        .service(web::scope("/s").configure(outer).service(other::thing()))
        .default_service(web::to(h));
}
"#;
    let dir = temp_crate("actixcfg", &[("actix-web", "4.15.0")], &[("lib.rs", src)]);
    let d = doc_of(&dir, None);
    let _ = std::fs::remove_dir_all(&dir);
    let f = facts(&d);
    assert!(
        f.contains(&("ANY".into(), "/c".into(), Some("strict".into()))),
        "{f:?}"
    );
    assert!(
        f.contains(&("PUT".into(), "/s/deep".into(), Some("strict".into()))),
        "{f:?}"
    );
    let orders: Vec<u64> = d["facts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["order"]["index"].as_u64().unwrap())
        .collect();
    assert_eq!(orders.len(), 2);
    let lims = limitations(&d);
    assert!(
        lims.iter().any(|l| l.contains("App default service")),
        "{lims:?}"
    );
    assert!(
        lims.iter().any(|l| l.contains("could not evaluate")),
        "{lims:?}"
    );
    let scope = d["limitationScopes"].as_array().unwrap();
    assert!(
        scope.iter().any(|s| s["templatePrefixes"] == json!(["/s"])),
        "{scope:?}"
    );
}

/// 사실의 (method, channel, trailingSlash, narrowed, order index, usr) 요약.
fn rows(doc: &Value) -> Vec<String> {
    doc["facts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| {
            format!(
                "{} {} {} {}{}{}",
                f["method"].as_str().unwrap(),
                f["channel"].as_str().unwrap_or("null"),
                f["pathAnchor"].as_str().unwrap(),
                f["trailingSlash"].as_str().unwrap_or("-"),
                if f.get("narrowed").is_some() {
                    " narrowed"
                } else {
                    ""
                },
                f.pointer("/symbol/usr")
                    .and_then(Value::as_str)
                    .map(|u| format!(" {u}"))
                    .unwrap_or_default()
            )
        })
        .collect()
}

/// actix 가드·method 생성자·리소스 메서드·기본 서비스·리다이렉트·파일 서비스·
/// 정규화 모드·동적 스코프·매크로 옵션.
#[test]
fn actix_guards_services_and_normalize_modes() {
    let src = r#"
use actix_web::{get, guard, route, web, App, http::Method};
use actix_web::middleware::{NormalizePath, TrailingSlash};
async fn h() -> &'static str { "" }
#[route("/prop", method = "PROPFIND")]
async fn custom() -> &'static str { "" }
#[get("/g", guard = "is_admin")]
async fn guarded() -> &'static str { "" }
fn scope_of() -> actix_web::Scope { web::scope("/fn").route("/x", web::get().to(h)) }
pub struct Server;
impl Server { pub fn app() { let _ = App::new(); } }
pub fn build(dynamic: &str) {
    let _ = App::new()
        .wrap(NormalizePath::new(TrailingSlash::Always))
        .service(web::resource("/any-or").guard(guard::Any(guard::Get()).or(guard::Post())).to(h))
        .service(web::resource("/m").guard(guard::Method(Method::PUT)).to(h))
        .service(web::resource("/hdr").guard(guard::Header("x", "y")).to(h))
        .service(web::resource("/both").guard(guard::All(guard::Get()).and(guard::Header("a", "b"))).to(h))
        .route("/patch", web::method(Method::PATCH).to(h))
        .route("/routed", web::route().method(Method::DELETE).to(h))
        .service(web::resource("/res").get(h).post(h).default_service(web::to(h)))
        .service(web::redirect("/old", "/new"))
        .service(actix_files::Files::new("/static", "./static"))
        .service(web::resource("/tail/{rest:.*}").to(h))
        .service(web::resource("/two/{a}.{b}").to(h))
        .service((guarded, custom))
        .service(web::scope(dynamic).route("/under", web::get().to(h)))
        .service(web::scope("/merge").wrap(NormalizePath::new(TrailingSlash::MergeOnly)).route("/k", web::get().to(h)).default_service(web::to(h)))
        .service(scope_of());
    let _ = App::new().service(web::resource("/second").to(h));
}
"#;
    let dir = temp_crate("actixmix", &[("actix-web", "4.15.0")], &[("lib.rs", src)]);
    let d = doc_of(&dir, None);
    let _ = std::fs::remove_dir_all(&dir);
    let r = rows(&d);
    let has = |s: &str| r.iter().any(|x| x == s);
    for want in [
        "GET /any-or root strict app::h",
        "POST /any-or root strict app::h",
        "PUT /m root strict app::h",
        "ANY /hdr root strict narrowed app::h",
        "GET /both root strict narrowed app::h",
        "PATCH /patch root strict app::h",
        "DELETE /routed root strict app::h",
        "GET /res root strict app::h",
        "POST /res root strict app::h",
        "ANY /res root strict app::h",
        "ANY /old root strict",
        "ANY /tail/{**} root - app::h",
        "ANY /tail/ root optional app::h",
        "GET /g root strict narrowed app::guarded",
        "GET /under base strict app::h",
        "GET /merge/k root strict app::h",
        "GET /fn/x root strict app::h",
        "ANY /second root strict app::h",
    ] {
        assert!(has(want), "missing {want:?} in {r:#?}");
    }
    let dynamic = d["facts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["dynamic"] == true)
        .count();
    assert_eq!(dynamic, 1, "two params in one segment is dynamic");
    let groups: BTreeSet<&str> = d["facts"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f.pointer("/order/group").and_then(Value::as_str))
        .collect();
    assert_eq!(
        groups.len(),
        2,
        "each App is its own order group: {groups:?}"
    );
    let lims = limitations(&d);
    for want in [
        "route-coverage: route macro on app::custom uses method PROPFIND",
        "framework-provided-routes: an actix-files service",
        "route-coverage: route pattern \"/two/{a}.{b}\"",
        "route-coverage: a scope default service",
        "unresolved-route-prefix:",
        "route-dispatch-order-unknown:",
        "route-coverage: an App built inside a method",
    ] {
        assert!(
            lims.iter().any(|l| l.starts_with(want)),
            "missing {want:?} in {lims:#?}"
        );
    }
    let scopes = d["limitationScopes"].as_array().unwrap();
    assert!(scopes
        .iter()
        .any(|s| s["templatePrefixes"] == json!(["/static"])
            && s["methods"] == json!(["GET", "HEAD"])));
}

/// axum 메서드 라우터 변형·서비스·CONNECT·연관 함수 핸들러·동적 nest 접두사.
#[test]
fn axum_method_router_variants() {
    let src = r#"
use axum::routing::{any_service, connect, get, get_service, on, MethodFilter, MethodRouter};
use axum::Router;
async fn h() {}
pub struct Api;
impl Api { pub async fn list() {} pub fn routes() -> Router { Router::new() } }
fn methods() -> MethodRouter { MethodRouter::new().get(h).merge(axum::routing::post(h)) }
pub fn app(p: &str) -> Router {
    Router::new()
        .route("/assoc", get(Api::list))
        .route("/unknown-filter", on(some_filter(), h))
        .route("/svc", get_service(svc()).fallback_service(svc()))
        .route("/anysvc", any_service(svc()))
        .route("/conn", connect(h))
        .route("/merged", methods())
        .route("/mf", get(h).on(MethodFilter::DELETE, h).fallback(h))
        .nest(p, Router::new().route("/tail", get(h)))
        .route("/bad/{a}{b}", get(h))
        .route("/pre{*rest}", get(h))
}
pub async fn run() { axum::serve(listener(), app("/x")).await; }
"#;
    let dir = temp_crate("axmix", &[("axum", "0.8.9")], &[("lib.rs", src)]);
    let d = doc_of(&dir, None);
    let _ = std::fs::remove_dir_all(&dir);
    let r = rows(&d);
    let has = |s: &str| r.iter().any(|x| x == s);
    for want in [
        "GET /assoc root strict app::Api::list",
        "ANY /unknown-filter root strict app::h",
        "GET /svc root strict",
        "ANY /svc root strict",
        "ANY /anysvc root strict",
        "GET /merged root strict app::h",
        "POST /merged root strict app::h",
        "DELETE /mf root strict app::h",
        "ANY /mf root strict app::h",
        "GET /tail base strict app::h",
    ] {
        assert!(has(want), "missing {want:?} in {r:#?}");
    }
    let lims = limitations(&d);
    for want in [
        "route-coverage: a CONNECT route",
        "route-coverage: route path \"/bad/{a}{b}\"",
        "route-coverage: route path \"/pre{*rest}\"",
        "unresolved-route-prefix: 1 route declaration(s) sit under a nest prefix",
        "route-coverage: a Router built inside a method",
        "missing-route-usrs:",
    ] {
        assert!(
            lims.iter().any(|l| l.starts_with(want)),
            "missing {want:?} in {lims:#?}"
        );
    }
    let dynamic = d["facts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["dynamic"] == true)
        .count();
    assert_eq!(
        dynamic, 1,
        "catch-all after static text is a dynamic declaration"
    );
}

/// 버전 밖 axum은 가까운 문법으로 읽고 한계를 낸다.
#[test]
fn axum_unverified_version_is_a_limitation() {
    let src = r#"
use axum::{routing::get, Router};
async fn h() {}
pub fn app() -> Router { Router::new().route("/items/:id", get(h)) }
pub async fn run() { axum::serve(listener(), app()).await; }
"#;
    let dir = temp_crate("ax06", &[("axum", "0.6.20")], &[("lib.rs", src)]);
    let d = doc_of(&dir, None);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(rows(&d).contains(&"GET /items/{} root strict app::h".to_string()));
    assert!(limitations(&d)[0].starts_with("route-framework-version-unknown: axum 0.6.20"));
}

/// 루트에서 닿지 않은 라우터끼리의 nest — 안쪽 라우터가 따로 한 번 더 나오지 않는다
/// (이름 순서와 무관).
#[test]
fn unrooted_inner_router_is_not_emitted_twice() {
    let src = r#"
use axum::{routing::get, Router};
async fn h() {}
pub fn a_inner() -> Router { Router::new().route("/leaf", get(h)) }
pub fn b_outer() -> Router { Router::new().nest("/mid", a_inner()) }
"#;
    let dir = temp_crate("unrooted", &[("axum", "0.8.9")], &[("lib.rs", src)]);
    let d = doc_of(&dir, None);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        rows(&d),
        vec!["GET /mid/leaf base strict app::h".to_string()]
    );
}

/// actix App을 지역 변수로 키우는 재대입은 따라가고, 함수에 넘긴 App은 한계로 센다.
#[test]
fn actix_app_through_variables_and_helpers() {
    let src = r#"
use actix_web::{web, App};
async fn h() -> &'static str { "" }
fn extend<T>(app: T) -> T { app }
pub fn build() {
    let mut app = App::new().route("/a", web::get().to(h));
    app = app.route("/b", web::post().to(h));
    let _ = extend(App::new().route("/c", web::get().to(h)));
    let _ = app;
}
"#;
    let dir = temp_crate("actixvar", &[("actix-web", "4.15.0")], &[("lib.rs", src)]);
    let d = doc_of(&dir, None);
    let _ = std::fs::remove_dir_all(&dir);
    let r = rows(&d);
    for want in [
        "GET /a root strict app::h",
        "POST /b root strict app::h",
        "GET /c root strict app::h",
    ] {
        assert!(r.iter().any(|x| x == want), "missing {want:?} in {r:#?}");
    }
    // /a·/b는 한 App(같은 group), /c는 다른 App이다.
    let group_of = |ch: &str| {
        d["facts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["channel"] == ch)
            .and_then(|f| f.pointer("/order/group").cloned())
    };
    assert_eq!(group_of("/a"), group_of("/b"));
    assert_ne!(group_of("/a"), group_of("/c"));
    assert!(limitations(&d)
        .iter()
        .any(|l| l.starts_with("route-coverage: an App passed to a function")));
}
