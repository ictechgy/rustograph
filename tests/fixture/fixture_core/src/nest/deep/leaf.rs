//! 인라인 조상의 `#[path]` 오버라이드로 도달하는 모듈 —
//! `nest::inner`의 기준 디렉터리는 `src/nest/inner`가 아니라
//! `src/nest/deep`이다.

/// `call_leaf`의 호출 대상.
pub fn leaf_probe() -> u32 {
    9
}
