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

/// 토큰 스트림에서 `name` 식별자를 찾는다 — 원본 span을 그대로 가진다.
fn find_ident(ts: TokenStream, name: &str) -> Option<proc_macro::Ident> {
    for tt in ts {
        match tt {
            proc_macro::TokenTree::Ident(i) if i.to_string() == name => return Some(i),
            proc_macro::TokenTree::Group(g) => {
                if let Some(i) = find_ident(g.stream(), name) {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// 플레이스홀더 식별자를 원본 span을 가진 토큰으로 치환한다 —
/// proc_macro만으로는 토큰 중간에 span을 끼울 수 없다.
fn substitute(ts: TokenStream, from: &str, to: proc_macro::Ident) -> TokenStream {
    ts.into_iter()
        .map(|tt| match tt {
            proc_macro::TokenTree::Group(g) => proc_macro::TokenTree::Group(
                proc_macro::Group::new(g.delimiter(), substitute(g.stream(), from, to.clone())),
            ),
            proc_macro::TokenTree::Ident(i) if i.to_string() == from => {
                proc_macro::TokenTree::Ident(to.clone())
            }
            other => other,
        })
        .collect()
}

/// 입력 impl을 그대로 다시 emit하면서, 메서드 이름 토큰의 span을 보존한
/// 채 다른 트레이트(`crate::c::Tr`)의 형제 impl을 추가한다 — 생성
/// 메서드의 이름 위치가 진짜 선언과 겹치므로(quote_spanned! 패턴)
/// provenance 검증은 위치만으로는 부족하고 트레이트 정체까지 봐야 한다.
#[proc_macro_attribute]
pub fn spawn_sibling(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let m = find_ident(item.clone(), "m").expect("method m");
    let sibling: TokenStream = "impl crate::c::Tr for crate::S { fn _m(&self) -> u32 { 3 } }"
        .parse()
        .unwrap();
    let mut out = item;
    out.extend(substitute(sibling, "_m", m));
    out
}

/// span을 보존한 채, 같은 소스 표기(`Tr`)지만 다른 트레이트를 가리키는
/// 형제 impl을 익명 const 안에 추가한다 — const 안의 `use`가 `Tr`을
/// `crate::b::Tr`로 가려서, 소스 표기만으로는 진짜 `impl Tr for S2`
/// (`Tr` → `crate::a::Tr`)와 구분이 안 된다. 해석된 정체로 가려야 한다.
#[proc_macro_attribute]
pub fn spawn_shadowed(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let m = find_ident(item.clone(), "m").expect("method m");
    let sib: TokenStream = "const _: () = { use crate::b::Tr; impl Tr for S2 { fn _m(&self) -> u32 { 3 } } };"
        .parse()
        .unwrap();
    let mut out = item;
    out.extend(substitute(sib, "_m", m));
    out
}

/// span을 보존한 채 제네릭 인자만 다른 형제 impl을 추가한다 —
/// `d::G<u8>`의 진짜 impl과 `d::G<u16>`의 생성 impl은 해석된 트레이트가
/// 같으므로 소스 표기의 인자 부분으로 가려야 한다.
#[proc_macro_attribute]
pub fn spawn_generic_sibling(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let m = find_ident(item.clone(), "m").expect("method m");
    let sib: TokenStream =
        "const _: () = { use crate::d; impl d::G<u16> for S3 { fn _m(&self) -> u32 { 3 } } };"
            .parse()
            .unwrap();
    let mut out = item;
    out.extend(substitute(sib, "_m", m));
    out
}

/// 입력 아이템을 익명 const 블록으로 감싼다 — serde_derive가 impl을
/// `const _: () = { .. }`로 감싸는 패턴. 입력 토큰은 그대로 유지되므로
/// 안의 메서드는 진짜 선언 위치를 가진다 — syn이 만든 정점과 매칭돼야
/// 한다(const 래퍼는 지역성 검사를 트리거하면 안 된다).
#[proc_macro_attribute]
pub fn wrap_in_const(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut inner = TokenStream::new();
    inner.extend(item);
    let group = proc_macro::Group::new(proc_macro::Delimiter::Brace, inner);
    let mut out: TokenStream = "const _: () =".parse().unwrap();
    out.extend(std::iter::once(proc_macro::TokenTree::Group(group)));
    out.extend(";".parse::<TokenStream>().unwrap());
    out
}

/// 입력 impl의 헤더 토큰을 통째로 복제해(`a`만 `c`로 교체, 각 토큰의
/// span 유지) `crate::c::Tr` 형제 impl을 만든다 — 위조 경로의 원본
/// 범위는 입력 `a::Tr`과 같으므로 헤더 원본 범위 대조를 통과하지만
/// 해석은 다른 트레이트로 간다.
fn forged_sibling(item: &TokenStream) -> TokenStream {
    let m = find_ident(item.clone(), "m").expect("method m");
    // 입력 헤더 토큰을 `impl`/`for` 사이(트레이트 경로)와 `for`/본문
    // 사이(self 타입)로 나눠 복제한다 — 각 토큰의 원본 span이 유지돼
    // 헤더 원본 범위 대조를 통과한다.
    let toks: Vec<proc_macro::TokenTree> = item.clone().into_iter().collect();
    let for_pos = toks
        .iter()
        .position(|t| matches!(t, proc_macro::TokenTree::Ident(i) if i.to_string() == "for"))
        .expect("for keyword");
    let brace_pos = toks
        .iter()
        .position(|t| matches!(t, proc_macro::TokenTree::Group(_)))
        .expect("body group");
    let clone_spans = |toks: &[proc_macro::TokenTree]| -> Vec<proc_macro::TokenTree> {
        toks.iter()
            .map(|t| match t {
                // 트레이트 경로의 첫 세그먼트 `a`만 `c`로 바꾼다 —
                // 나머지 토큰(포함해 `Tr`)은 span까지 그대로다.
                proc_macro::TokenTree::Ident(i) if i.to_string() == "a" => {
                    proc_macro::TokenTree::Ident(proc_macro::Ident::new("c", i.span()))
                }
                _ => t.clone(),
            })
            .collect()
    };
    let forged_trait = clone_spans(&toks[1..for_pos]);
    let forged_self = clone_spans(&toks[for_pos + 1..brace_pos]);
    let cs = |text: &str| {
        proc_macro::TokenTree::Ident(proc_macro::Ident::new(text, proc_macro::Span::call_site()))
    };
    // 형제: impl c::Tr for S4 { fn m(&self) -> u32 { crate::forged_target() } }
    let mut sib: TokenStream = std::iter::once(cs("impl"))
        .chain(forged_trait)
        .chain(std::iter::once(cs("for")))
        .chain(forged_self)
        .collect();
    let body: TokenStream = "fn _m(&self) -> u32 { crate::forged_target() }".parse().unwrap();
    sib.extend(std::iter::once(proc_macro::TokenTree::Group(
        proc_macro::Group::new(proc_macro::Delimiter::Brace, substitute(body, "_m", m)),
    )));
    sib
}

/// 입력 impl을 `crate::passthrough! { .. }`(함수형 매크로) 안에 숨기고
/// span-위조 형제 impl을 낸다 — 확장 안의 함수형 매크로 호출을 재귀
/// 확장하지 않으면 원본 사본이 숨은 채 위조 사본이 단독 후보로 채택된다.
#[proc_macro_attribute]
pub fn forge_sibling(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // 원본은 함수형 매크로 안으로 — 확장 전에는 토큰 트리라 안이 안 보인다.
    let mut out: TokenStream = "crate::passthrough!".parse().unwrap();
    out.extend(std::iter::once(proc_macro::TokenTree::Group(
        proc_macro::Group::new(proc_macro::Delimiter::Brace, item.clone()),
    )));
    out.extend(forged_sibling(&item));
    out
}

/// 어트리뷰트 인자를 그대로 아이템으로 emit하고 입력 아이템은 버린다 —
/// `#[emit_args(impl ..)] struct X;` 꼴에서 원본 impl이 인자 토큰
/// 안에만 존재하는 형태를 만든다.
#[proc_macro_attribute]
pub fn emit_args(attr: TokenStream, _item: TokenStream) -> TokenStream {
    attr
}

/// `forge_sibling`의 변형 — 원본 impl을 `#[emit_args(..)]`의 인자 토큰
/// 안에 숨긴다. 확장 트리에서 `Carrier` 아이템에 달린 속성 매크로를
/// 확장하지 않으면 원본 사본이 안 보여 위조 형제가 단독 후보가 된다.
#[proc_macro_attribute]
pub fn forge_via_attr(_attr: TokenStream, item: TokenStream) -> TokenStream {
    // #[fixture_macros::emit_args( <item> )] struct Carrier5;
    // `#[name( 인자 )]`를 문자열 파싱으로 만들면 미폐쇄 구분자라
    // 안 되므로 토큰으로 조립한다.
    let args = proc_macro::Group::new(proc_macro::Delimiter::Parenthesis, item.clone());
    let mut attr_body: TokenStream = "fixture_macros::emit_args".parse().unwrap();
    attr_body.extend(std::iter::once(proc_macro::TokenTree::Group(args)));
    let bracket = proc_macro::Group::new(proc_macro::Delimiter::Bracket, attr_body);
    let mut out: TokenStream = "#".parse().unwrap();
    out.extend(std::iter::once(proc_macro::TokenTree::Group(bracket)));
    out.extend("struct Carrier5;".parse::<TokenStream>().unwrap());
    out.extend(forged_sibling(&item));
    out
}
