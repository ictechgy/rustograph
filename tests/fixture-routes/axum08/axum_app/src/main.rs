//! 서버 진입점 — 여기서 서빙하는 라우터가 선언의 루트다.

#[tokio::main]
async fn main() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await.unwrap();
    axum::serve(listener, axum_app::app()).await.unwrap();
}
