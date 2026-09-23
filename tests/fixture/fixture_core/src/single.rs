//! 비-mod.rs 파일 모듈 — `#[path]`의 기준 디렉터리는 이 파일이 놓인
//! `src/`다(`module_dir`인 `src/single/`이 아니다).

#[path = "sibling.rs"]
pub mod inner;

/// 파일에 직접 선언된 인라인 모듈의 `#[path]`도 파일 디렉터리 기준이다
/// — `src/single/pathdir/`가 아니라 `src/pathdir/`다.
#[path = "pathdir"]
mod pin {
    #[path = "pin_leaf.rs"]
    pub mod pin_leaf;
}

/// `pin::pin_leaf`의 함수를 호출한다 — 인라인 `#[path]` 기준이
/// module_dir로 잘못 잡히면 `pin_leaf` 모듈이 통째로 빠진다.
pub fn call_pin() -> u32 {
    pin::pin_leaf::pv()
}
