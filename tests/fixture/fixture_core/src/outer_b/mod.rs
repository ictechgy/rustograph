//! 같은 파일을 가리키는 두 모듈 중 다른 쪽 — `super::`이 `outer_b`다.

#[path = "../outer_a/duplex.rs"]
pub mod inner;

/// `inner` 문맥에서 `super::Tr`이 가리키는 *다른* 트레이트.
pub trait Tr {
    fn m(&self) -> u32;
}

/// `inner` 문맥에서 `super::probe()`가 가리키는 대상.
pub fn probe() -> u32 {
    2
}
