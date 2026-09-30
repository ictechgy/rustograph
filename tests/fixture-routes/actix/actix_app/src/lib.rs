//! 합성 actix-web 4 서버 — 매크로·리소스·스코프·설정 함수·경로 정규화를 덮는다.
//! 핸들러는 자기 정점 ID를 응답 본문으로 돌려준다.

pub mod handlers;

use actix_web::body::MessageBody;
use actix_web::dev::{ServiceFactory, ServiceRequest, ServiceResponse};
use actix_web::middleware::NormalizePath;
use actix_web::{guard, web, App, Error};

/// 서빙되는 App(main.rs의 `HttpServer::new`가 부른다). 등록 순서가 디스패치 순서다.
pub fn app() -> App<
    impl ServiceFactory<
        ServiceRequest,
        Config = (),
        Response = ServiceResponse<impl MessageBody>,
        Error = Error,
        InitError = (),
    >,
> {
    App::new()
        .wrap(NormalizePath::trim())
        .service(handlers::index)
        .service(
            web::scope("/api")
                .service(handlers::items::list)
                .service(handlers::items::create)
                .service(handlers::items::show)
                .service(handlers::items::by_slug)
                .route("/ping", web::get().to(handlers::ping))
                .service(
                    web::resource("/users/{id}")
                        .route(web::get().to(handlers::users::get))
                        .route(web::delete().to(handlers::users::delete)),
                )
                .service(web::resource(["/a", "/b"]).to(handlers::multi))
                .configure(handlers::config),
        )
        .service(web::resource("/files/{tail}*").to(handlers::files))
        .service(handlers::routes_multi)
        .service(handlers::route_macro)
        .service(
            web::scope("/v1")
                .guard(guard::Header("x-api", "1"))
                .service(handlers::v1_status),
        )
}
