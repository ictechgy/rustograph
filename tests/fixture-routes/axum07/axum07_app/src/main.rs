//! 0.7 서빙 — `into_make_service`로 넘긴 라우터가 루트다.

#[tokio::main]
async fn main() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await.unwrap();
    axum::serve(listener, axum07_app::app().into_make_service())
        .await
        .unwrap();
}
