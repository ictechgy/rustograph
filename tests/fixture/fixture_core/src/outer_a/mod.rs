//! 같은 파일을 가리키는 두 모듈 중 한쪽 — `super::`이 `outer_a`다.

pub mod duplex;

/// `duplex`의 `super::Tr`이 가리키는 트레이트.
pub trait Tr {
    fn m(&self) -> u32;
}

/// `duplex`의 `super::probe()`가 이 문맥에서 가리키는 대상.
pub fn probe() -> u32 {
    1
}
