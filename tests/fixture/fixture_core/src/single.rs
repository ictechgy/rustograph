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

    /// 일반 자식도 오버라이드된 기준 디렉터리를 따라간다 —
    /// `src/single/pin_plain.rs`가 아니라 `src/pathdir/pin_plain.rs`다.
    pub mod pin_plain;
}

/// `#[path]`로 로드된 파일은 자기 디렉터리를 소유한다 — `loaded.rs`의
/// 인라인 자식 기준은 `src/ploaded/loaded/`가 아니라 `src/ploaded/`다.
#[path = "ploaded/loaded.rs"]
pub mod pmod;

/// `pin`·`pmod` 아래의 함수를 호출한다 — 인라인 `#[path]` 기준이나
/// 로드 파일의 디렉터리 소유가 틀리면 이 모듈들이 통째로 빠진다.
pub fn call_pin() -> u32 {
    pin::pin_leaf::pv() + pin::pin_plain::pw() + pmod::call_leaf()
}
