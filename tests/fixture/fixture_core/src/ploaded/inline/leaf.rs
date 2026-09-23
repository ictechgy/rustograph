//! `ploaded/loaded.rs`의 `mod inline` 안의 `mod leaf;` — `#[path]`로
//! 로드된 파일의 인라인 자손은 파일 디렉터리 아래로 내려간다.

/// `call_leaf`가 호출하는 함수.
pub fn pl() -> u32 {
    13
}
