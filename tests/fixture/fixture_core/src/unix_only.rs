//! #[cfg(unix)] 모듈 — 조건부 모듈의 cfg 메타데이터를 검증한다.

pub fn unix_only_fn() -> u32 {
    3
}
