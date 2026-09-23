//! `single.rs`의 `#[path = "pathdir"] mod pin` 안의 일반 `mod pin_plain;`
//! — 인라인 `#[path]` 조상 아래 일반 자식도 오버라이드된 디렉터리를 쓴다.

/// `call_pin`이 호출하는 함수.
pub fn pw() -> u32 {
    12
}
