//! 핸들러 — 본문은 자기 정점 ID다.

use actix_web::{get, route, routes, web, HttpResponse, Responder};

#[get("/")]
pub async fn index() -> impl Responder {
    "actix_app::handlers::index"
}

pub async fn ping() -> impl Responder {
    "actix_app::handlers::ping"
}

pub async fn multi() -> impl Responder {
    "actix_app::handlers::multi"
}

pub async fn files() -> impl Responder {
    "actix_app::handlers::files"
}

pub async fn cfg_post() -> impl Responder {
    "actix_app::handlers::cfg_post"
}

#[routes]
#[get("/r1")]
#[post("/r2")]
pub async fn routes_multi() -> impl Responder {
    "actix_app::handlers::routes_multi"
}

#[route("/m", method = "GET", method = "PUT")]
pub async fn route_macro() -> impl Responder {
    "actix_app::handlers::route_macro"
}

#[get("/status")]
pub async fn v1_status() -> impl Responder {
    "actix_app::handlers::v1_status"
}

/// 어느 App에도 등록하지 않은 매크로 핸들러 — base 앵커로 나온다.
#[get("/lonely")]
pub async fn lonely() -> impl Responder {
    "actix_app::handlers::lonely"
}

/// `.configure(config)`로 스코프에 끼워 넣는 등록.
pub fn config(cfg: &mut web::ServiceConfig) {
    cfg.service(web::resource("/cfg").route(web::post().to(cfg_post)));
}

/// 항목 — 정수 id가 슬러그보다 먼저 등록된다(등록 순서 디스패치).
pub mod items {
    use actix_web::{get, post, Responder};

    #[get("/items")]
    pub async fn list() -> impl Responder {
        "actix_app::handlers::items::list"
    }

    #[post("/items")]
    pub async fn create() -> impl Responder {
        "actix_app::handlers::items::create"
    }

    #[get("/items/{id:\\d+}")]
    pub async fn show() -> impl Responder {
        "actix_app::handlers::items::show"
    }

    #[get("/items/{slug}")]
    pub async fn by_slug() -> impl Responder {
        "actix_app::handlers::items::by_slug"
    }
}

/// 한 리소스의 라우트 수준 method 가드.
pub mod users {
    use actix_web::Responder;

    pub async fn get() -> impl Responder {
        "actix_app::handlers::users::get"
    }

    pub async fn delete() -> impl Responder {
        "actix_app::handlers::users::delete"
    }
}

/// HttpResponse를 쓰는 핸들러가 없으면 경고가 나므로 하나 둔다.
pub async fn unused_response() -> HttpResponse {
    HttpResponse::Ok().body("actix_app::handlers::unused_response")
}
