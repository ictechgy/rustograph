//! 공유 파일 — 모듈 레벨 `#[path]` 모듈(`shared`)과 fn 안 지역 모듈
//! (`local_shadowed`의 `local_shared`)이 같은 파일을 가리킨다.
//! 지역 쪽에서 해석된 정의가 이 파일의 모듈 레벨 정점을 훔치면 안 된다.

/// 모듈 레벨 정점이 되는 구조체.
pub struct Shared;

impl Shared {
    /// 지역 모듈에서도 같은 위치를 가리키는 연관 함수.
    pub fn val() -> u32 {
        9
    }
}

/// 자유 함수 — 지역 모듈 경로로도 호출 가능하다.
pub fn helper() -> u32 {
    1
}
