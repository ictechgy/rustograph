//! 핸들러 — 본문은 자기 정점 ID다.

pub async fn root() -> &'static str {
    "axum_app::handlers::root"
}
pub async fn health() -> &'static str {
    "axum_app::handlers::health"
}
/// HEAD 응답은 본문이 지워지므로 핸들러 ID를 헤더로도 싣는다.
pub async fn health_head() -> ([(&'static str, &'static str); 1], &'static str) {
    (
        [("x-handler", "axum_app::handlers::health_head")],
        "axum_app::handlers::health_head",
    )
}
pub async fn tag_prefixed() -> &'static str {
    "axum_app::handlers::tag_prefixed"
}
pub async fn status() -> &'static str {
    "axum_app::handlers::status"
}
pub async fn status_literal() -> &'static str {
    "axum_app::handlers::status_literal"
}
pub async fn search() -> &'static str {
    "axum_app::handlers::search"
}
pub async fn lit() -> &'static str {
    "axum_app::handlers::lit"
}
pub async fn trailing() -> &'static str {
    "axum_app::handlers::trailing"
}
pub async fn api_fallback() -> (axum::http::StatusCode, &'static str) {
    (
        axum::http::StatusCode::NOT_FOUND,
        "axum_app::handlers::api_fallback",
    )
}
pub async fn admin_stats() -> &'static str {
    "axum_app::handlers::admin_stats"
}
pub async fn admin_reset() -> &'static str {
    "axum_app::handlers::admin_reset"
}
pub async fn orphan() -> &'static str {
    "axum_app::handlers::orphan"
}

/// 항목 CRUD.
pub mod items {
    pub async fn list() -> &'static str {
        "axum_app::handlers::items::list"
    }
    pub async fn create() -> &'static str {
        "axum_app::handlers::items::create"
    }
    pub async fn show() -> &'static str {
        "axum_app::handlers::items::show"
    }
    pub async fn update() -> &'static str {
        "axum_app::handlers::items::update"
    }
    pub async fn remove() -> &'static str {
        "axum_app::handlers::items::remove"
    }
    pub async fn tag() -> &'static str {
        "axum_app::handlers::items::tag"
    }
    pub async fn special() -> &'static str {
        "axum_app::handlers::items::special"
    }
}

/// 파일 catch-all.
pub mod files {
    pub async fn serve() -> &'static str {
        "axum_app::handlers::files::serve"
    }
}

/// 모든 method를 받는 사용자 경로.
pub mod users {
    pub async fn any_method() -> &'static str {
        "axum_app::handlers::users::any_method"
    }
}
