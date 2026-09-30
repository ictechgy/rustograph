//! 하위 라우터들 — nest·merge·지역 변수·재대입·상수 경로.

use crate::handlers::{self, files, items, users};
use axum::routing::{any, get, on, post, MethodFilter};
use axum::Router;

/// 관리 경로 상수 — 경로 인자가 리터럴이 아니어도 상수면 템플릿이다.
const ADMIN_RESET: &str = "/admin/reset";

/// `/api` 아래에 붙는 라우터.
pub fn api() -> Router {
    let items = Router::new()
        .route("/", get(items::list).post(items::create))
        .route("/{id}", get(items::show).put(items::update).delete(items::remove))
        .route("/{id}/tags/{tag}", get(items::tag))
        .route("/special", get(items::special));
    let mut router = Router::new().nest("/items", items);
    router = router.route("/files/{*path}", get(files::serve));
    router
        .route("/v{version}/status", get(handlers::status))
        .route("/users/{id}", any(users::any_method))
        .route("/search", on(MethodFilter::GET.or(MethodFilter::POST), handlers::search))
        .route("/closure", get(|| async { "axum_app::routes::api" }))
        .route("/lit/{{braces}}", get(handlers::lit))
        .route("/trailing/", get(handlers::trailing))
        .route("/tag_{name}", get(handlers::tag_prefixed))
        .fallback(handlers::api_fallback)
}

/// 루트에 merge되는 관리 라우터.
pub fn admin() -> Router {
    Router::new()
        .route("/admin/stats", get(handlers::admin_stats))
        .route(ADMIN_RESET, post(handlers::admin_reset))
}

/// 어디에도 붙지 않는 라우터 — 서빙 루트에서 닿지 않으므로 base 앵커로 나온다.
pub fn orphan() -> Router {
    Router::new().route("/orphan", get(handlers::orphan))
}
