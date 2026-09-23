//! `single.rs`의 `#[path = "ploaded/loaded.rs"] mod pmod`이 가리키는
//! 파일 — `#[path]`로 로드된 파일은 자기 디렉터리(`ploaded/`)를
//! 자식 기준으로 소유한다(스템 디렉터리 `ploaded/loaded/`를 만들지
//! 않는다 — rustc 실증).

/// 인라인 자식 — `leaf`는 `ploaded/inline/leaf.rs`다.
pub mod inline {
    /// 일반 파일 자식 — 부모의 실효 디렉터리 `ploaded/inline/` 아래.
    pub mod leaf;
}

/// `call_pin`이 호출하는 함수.
pub fn call_leaf() -> u32 {
    inline::leaf::pl()
}
