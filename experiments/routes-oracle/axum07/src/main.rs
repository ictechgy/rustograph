//! axum 0.7.9 오라클 — fixture `axum07_app::app()`에 `oneshot`으로 요청한다.

use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use oracle_common::{evaluate, io_paths, load_facts, plan, write};
use tower::ServiceExt;

/// 기대 디스패치 — 0.7 문법(`:id`·`*rest`·접두사 파라미터·이름에 흡수된 접미사).
const EXPECTED: &[(&str, &str, &str)] = &[
    ("GET", "/items/5", "axum07_app::items_show"),
    ("PATCH", "/items/5", "axum07_app::items_patch"),
    ("GET", "/files/a/b", "axum07_app::files"),
    ("GET", "/user_42", "axum07_app::user_prefixed"),
    ("GET", "/f/report.json", "axum07_app::json_named"),
    ("GET", "/f/report", "axum07_app::json_named"),
    ("GET", "/api", "axum07_app::api_root"),
    ("POST", "/api/echo", "axum07_app::api_echo"),
    ("GET", "/orders//lines", "axum07_app::order_lines"),
    ("GET", "/x_/y", "axum07_app::x_prefixed"),
    ("GET", "/x_1/y", "axum07_app::x_prefixed"),
];

/// 닿지 않아야 하는 요청.
const NEGATIVE: &[(&str, &str)] = &[
    ("GET", "/files"),
    ("GET", "/files/"),
    ("GET", "/user_"),
    ("GET", "/api/"),
    ("GET", "/items/5/"),
    ("POST", "/items/5"),
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
        let resp = axum07_app::app().oneshot(req).await.expect("infallible");
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
        "axum07",
        "axum 0.7.9 (matchit 0.7.3)",
        &dispatch,
        &facts,
        results,
    );
    write(&rec, &output)
}
