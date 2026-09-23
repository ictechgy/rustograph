//! 비-mod.rs 파일 모듈 — `#[path]`의 기준 디렉터리는 이 파일이 놓인
//! `src/`다(`module_dir`인 `src/single/`이 아니다).

#[path = "sibling.rs"]
pub mod inner;
