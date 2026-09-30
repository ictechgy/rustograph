//! 합성 axum 0.7 서버 — 0.7 경로 문법과 서비스 등록을 덮는다.

use axum::body::Body;
use axum::response::Response;
use axum::routing::{get, post};
use axum::Router;
use std::convert::Infallible;

/// 서빙되는 라우터.
pub fn app() -> Router {
    let mut router = Router::new()
        .route("/items/:id", get(items_show).patch(items_patch))
        .route("/files/*path", get(files))
        .route("/user_:id", get(user_prefixed))
        .route("/f/:name.json", get(json_named))
        .route("/orders/:id/lines", get(order_lines))
        .route("/x_:id/y", get(x_prefixed));
    router = router.nest("/api", api());
    router
        .route_service("/svc", tower::service_fn(svc))
        .nest_service("/static", tower::service_fn(svc))
}

/// `/api` 아래 — 안쪽 `/`는 접두사 자체(`/api`)가 된다.
fn api() -> Router {
    Router::new()
        .route("/", get(api_root))
        .route("/echo", post(api_echo))
}

/// tower 서비스 — 핸들러 함수가 아니라 usr가 없다.
async fn svc(_req: axum::extract::Request) -> Result<Response, Infallible> {
    Ok(Response::new(Body::from("service")))
}

pub async fn items_show() -> &'static str {
    "axum07_app::items_show"
}
pub async fn items_patch() -> &'static str {
    "axum07_app::items_patch"
}
pub async fn files() -> &'static str {
    "axum07_app::files"
}
pub async fn user_prefixed() -> &'static str {
    "axum07_app::user_prefixed"
}
pub async fn json_named() -> &'static str {
    "axum07_app::json_named"
}
pub async fn x_prefixed() -> &'static str {
    "axum07_app::x_prefixed"
}
pub async fn order_lines() -> &'static str {
    "axum07_app::order_lines"
}
pub async fn api_root() -> &'static str {
    "axum07_app::api_root"
}
pub async fn api_echo() -> &'static str {
    "axum07_app::api_echo"
}
