//! mod 선언이 없는 orphan 파일 — 그래프 정점이 없어 사실에 usr가 없다.

pub fn stray() {
    let _ = sqlx::query!("SELECT * FROM orphan_t");
}
