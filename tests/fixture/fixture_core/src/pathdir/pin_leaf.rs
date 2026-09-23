//! `single.rs`의 인라인 `#[path = "pathdir"] mod pin` 안의
//! `#[path = "pin_leaf.rs"] mod pin_leaf`가 가리키는 파일 — 비-mod.rs
//! 파일의 인라인 조상 `#[path]`는 파일 디렉터리(`src/`) 기준이다.

/// `call_pin`이 호출하는 함수.
pub fn pv() -> u32 {
    11
}
