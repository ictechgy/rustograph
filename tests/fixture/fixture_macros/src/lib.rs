//! fixture proc 매크로 크레이트 — 의미 해석이 proc 매크로 서버를 통해
//! 실제 확장 트리까지 걷는지 검증하는 입력.

extern crate proc_macro;

use proc_macro::TokenStream;

/// `crate::util::helper()` 호출로 확장된다 — 확장이 되면
/// 호출자 → helper 확정 간선이 생겨야 한다.
/// proc 매크로는 `$crate`를 emit할 수 없으므로 `crate::`(호출부 크레이트)
/// 경로를 쓴다 — ra는 확장 안의 자기 크레이트명 경로를 해석하지 못한다.
#[proc_macro]
pub fn emit_helper_call(_input: TokenStream) -> TokenStream {
    "crate::util::helper()".parse().unwrap()
}

/// 입력을 그대로 돌려주는 속성 매크로 — 확장 안의 impl은 매크로 파일
/// 소스를 가지지만 syn이 만든 메서드 정점이 실재하므로, 호출은
/// 타입이 아니라 그 메서드 정점으로 가야 한다.
#[proc_macro_attribute]
pub fn keep(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}
