//! 서버 진입점.

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    actix_web::HttpServer::new(actix_app::app)
        .bind(("127.0.0.1", 8080))?
        .run()
        .await
}
