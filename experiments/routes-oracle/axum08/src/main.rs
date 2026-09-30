//! axum 0.8.9 오라클 — fixture `axum_app::app()`에 `tower::ServiceExt::oneshot`으로 요청한다.

use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use oracle_common::{evaluate, io_paths, load_facts, plan, write};
use tower::ServiceExt;

/// 사람이 적은 기대 디스패치(재현율의 분모) — 구체성·nest·merge·catch-all·HEAD.
const EXPECTED: &[(&str, &str, &str)] = &[
    ("GET", "/", "axum_app::handlers::root"),
    ("GET", "/health", "axum_app::handlers::health"),
    ("HEAD", "/health", "axum_app::handlers::health_head"),
    ("GET", "/api/items", "axum_app::handlers::items::list"),
    ("POST", "/api/items", "axum_app::handlers::items::create"),
    ("GET", "/api/items/42", "axum_app::handlers::items::show"),
    ("PUT", "/api/items/42", "axum_app::handlers::items::update"),
    (
        "DELETE",
        "/api/items/42",
        "axum_app::handlers::items::remove",
    ),
    (
        "GET",
        "/api/items/special",
        "axum_app::handlers::items::special",
    ),
    (
        "GET",
        "/api/items/42/tags/red",
        "axum_app::handlers::items::tag",
    ),
    (
        "GET",
        "/api/files/a/b/c.txt",
        "axum_app::handlers::files::serve",
    ),
    ("GET", "/api/files/x", "axum_app::handlers::files::serve"),
    ("GET", "/api/v2/status", "axum_app::handlers::status"),
    (
        "PATCH",
        "/api/users/7",
        "axum_app::handlers::users::any_method",
    ),
    ("GET", "/api/search", "axum_app::handlers::search"),
    ("POST", "/api/search", "axum_app::handlers::search"),
    ("GET", "/api/closure", "axum_app::routes::api"),
    (
        "POST",
        "/api/v/status",
        "axum_app::handlers::status_literal",
    ),
    (
        "GET",
        "/api/items//tags/red",
        "axum_app::handlers::items::tag",
    ),
    ("GET", "/api/tag_x", "axum_app::handlers::tag_prefixed"),
    ("GET", "/api/trailing/", "axum_app::handlers::trailing"),
    ("GET", "/admin/stats", "axum_app::handlers::admin_stats"),
    ("POST", "/admin/reset", "axum_app::handlers::admin_reset"),
];

/// 어느 핸들러에도 닿지 않아야 하는 요청(404·405·fallback).
const NEGATIVE: &[(&str, &str)] = &[
    ("GET", "/api/items/"),
    ("GET", "/api/files"),
    ("GET", "/api/files/"),
    ("DELETE", "/api/search"),
    ("GET", "/api/trailing"),
    ("GET", "/health/"),
    ("POST", "/admin/stats"),
    ("GET", "/orphan"),
    ("GET", "/api/lit/braces"),
    ("GET", "/api/lit/%7Bbraces%7D"),
    ("GET", "/api/v/status"),
    ("GET", "/api/tag_"),
];

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let (input, output) = io_paths();
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(input).expect("routes document"))
            .expect("json");
    let (dispatch, facts) = load_facts(&doc);
    let probes = plan(&facts, EXPECTED, NEGATIVE);
    let mut results = Vec::new();
    for p in probes {
        let req = Request::builder()
            .method(p.method.as_str())
            .uri(p.path.as_str())
            .body(Body::empty())
            .expect("request");
        let resp = axum_app::app().oneshot(req).await.expect("infallible");
        let status = resp.status().as_u16();
        // HEAD 응답은 본문이 지워진다 — 핸들러가 헤더로 밝혔으면 그 값을 쓴다.
        let marker = resp
            .headers()
            .get("x-handler")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let bytes = resp.into_body().collect().await.expect("body").to_bytes();
        let body = marker.unwrap_or_else(|| String::from_utf8_lossy(&bytes).into_owned());
        results.push((p, status, body));
    }
    let rec = evaluate(
        "axum08",
        "axum 0.8.9 (matchit 0.8.4)",
        &dispatch,
        &facts,
        results,
    );
    write(&rec, &output)
}
