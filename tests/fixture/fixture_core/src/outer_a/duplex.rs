//! 같은 물리 파일을 두 모듈이 가리킨다 — `outer_a::duplex`와
//! `outer_b::inner`. `super::` 경로는 어느 문맥으로 해석되느냐에 따라
//! 다른 정점을 가리킨다 — ra가 임의의 문맥으로 def를 묶으면
//! 잘못된 모듈의 본문 간선이 정점에 귀속될 수 있다.

use super::Tr;

impl Tr for crate::S {
    fn m(&self) -> u32 {
        super::probe()
    }
}
