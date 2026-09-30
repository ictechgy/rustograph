//! 합성 axum 0.8 서버 — `rustograph routes` 테스트와 라우팅 오라클이 같은 소스를 쓴다.
//! 핸들러는 자기 정점 ID를 응답 본문으로 돌려준다(오라클이 어느 핸들러에 닿았는지 본다).

pub mod handlers;
pub mod routes;

use axum::routing::get;
use axum::Router;

/// 서빙되는 라우터(main.rs가 `axum::serve`로 띄운다).
pub fn app() -> Router {
    Router::new()
        .route("/", get(handlers::root))
        .route("/health", get(handlers::health).head(handlers::health_head))
        .nest("/api", routes::api())
        .merge(routes::admin())
}
