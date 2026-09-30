//! 본문 지역 묶음(let·매개변수·클로저·패턴) 스코프.
//!
//! 표현식 위치의 단일 식별자 경로는 rustc에서 지역 묶음이 아이템보다 먼저
//! 해석된다. 이 순서를 지키지 않으면 `fn f(repo: &Repo) { repo.all() }`의
//! `repo`가 같은 이름의 크레이트 루트 모듈 `crate::repo`로 읽혀 가짜
//! references 간선이 되고, 순회가 그 모듈이 import한 아이템 전부로 번진다.
//! 이 모듈은 이름만 모은다 — 무엇을 가리는지는 방문자가 정한다.

use std::collections::BTreeSet;

/// 중첩 블록마다 한 겹씩 쌓는 지역 이름 스택이다.
///
/// 흐름 민감 분석이 아니라 어휘 스코프 근사다: 한 블록 안에서는 `let`
/// 문 뒤부터 이름이 보이고(방문 순서로 보장), 블록을 나가면 사라진다.
#[derive(Default)]
pub(super) struct Scopes {
    frames: Vec<BTreeSet<String>>,
}

impl Scopes {
    /// 매개변수 이름으로 첫 겹을 만든다 — 함수 본문 전체에서 보인다.
    pub(super) fn with_params(params: &[String]) -> Scopes {
        Scopes {
            frames: vec![params.iter().cloned().collect()],
        }
    }

    /// 새 겹을 연다(블록·match 팔·클로저·for 본문·if/while 조건+본문).
    pub(super) fn push(&mut self) {
        self.frames.push(BTreeSet::new());
    }

    /// 가장 안쪽 겹을 닫는다. 첫 겹(매개변수)은 닫지 않는다 — 짝이 안 맞는
    /// 방문이 있어도 매개변수 가림이 사라지지 않게 하기 위해서다.
    pub(super) fn pop(&mut self) {
        if self.frames.len() > 1 {
            self.frames.pop();
        }
    }

    /// 패턴이 묶는 이름을 가장 안쪽 겹에 더한다.
    pub(super) fn bind(&mut self, pat: &syn::Pat) {
        let mut names = Vec::new();
        pat_bindings(pat, &mut names);
        if self.frames.is_empty() {
            self.frames.push(BTreeSet::new());
        }
        if let Some(top) = self.frames.last_mut() {
            top.extend(names);
        }
    }

    /// 지역 묶음인가 — 어느 겹에든 있으면 아이템보다 먼저 해석된다.
    pub(super) fn is_local(&self, name: &str) -> bool {
        self.frames.iter().any(|f| f.contains(name))
    }

    /// 중첩 아이템(블록 안 `fn`)은 바깥 지역을 캡처하지 않는다 — 방문하는
    /// 동안 스택을 비웠다가 되돌린다.
    pub(super) fn take(&mut self) -> Vec<BTreeSet<String>> {
        std::mem::take(&mut self.frames)
    }

    /// `take`로 비운 스택을 되돌린다.
    pub(super) fn restore(&mut self, frames: Vec<BTreeSet<String>>) {
        self.frames = frames;
    }
}

/// 함수 시그니처의 매개변수가 묶는 이름이다(`self` 수신자는 경로 판정에서
/// 따로 처리하므로 넣지 않는다).
pub(super) fn param_bindings(sig: &syn::Signature) -> Vec<String> {
    let mut names = Vec::new();
    for arg in &sig.inputs {
        if let syn::FnArg::Typed(t) = arg {
            pat_bindings(&t.pat, &mut names);
        }
    }
    names
}

/// 패턴이 새로 묶는 식별자를 모은다.
///
/// 식별자 패턴은 상수·유닛 구조체·유닛 배리언트를 가리킬 수도 있다
/// (`match x { MAX => .. }`). rustc는 그 이름이 스코프의 그런 아이템이면
/// 묶음이 아니라 비교로 읽는다. 구문 수확은 아이템 종류를 모르므로 Rust
/// 명명 관례로 근사한다: `ref`·`mut`·`@` 하위 패턴이 붙었거나 대문자로
/// 시작하지 않으면 묶음이다. 소문자 상수 패턴은 그 스코프에서 같은 이름
/// 참조를 잃는 쪽(간선 누락)으로 틀린다 — 가짜 간선보다 드물고 안전하다.
pub(super) fn pat_bindings(pat: &syn::Pat, out: &mut Vec<String>) {
    match pat {
        syn::Pat::Ident(p) => {
            let name = p.ident.to_string();
            let marked = p.by_ref.is_some() || p.mutability.is_some() || p.subpat.is_some();
            let upper = name.chars().next().is_some_and(char::is_uppercase);
            if marked || !upper {
                out.push(name);
            }
            if let Some((_, sub)) = &p.subpat {
                pat_bindings(sub, out);
            }
        }
        syn::Pat::Struct(p) => {
            for f in &p.fields {
                pat_bindings(&f.pat, out);
            }
        }
        syn::Pat::TupleStruct(p) => p.elems.iter().for_each(|e| pat_bindings(e, out)),
        syn::Pat::Tuple(p) => p.elems.iter().for_each(|e| pat_bindings(e, out)),
        syn::Pat::Slice(p) => p.elems.iter().for_each(|e| pat_bindings(e, out)),
        syn::Pat::Or(p) => p.cases.iter().for_each(|e| pat_bindings(e, out)),
        syn::Pat::Reference(p) => pat_bindings(&p.pat, out),
        syn::Pat::Type(p) => pat_bindings(&p.pat, out),
        syn::Pat::Paren(p) => pat_bindings(&p.pat, out),
        // 리터럴·범위·경로·와일드카드·나머지·매크로 패턴은 이름을 묶지 않는다.
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(src: &str) -> Vec<String> {
        let pat = syn::parse::Parser::parse_str(syn::Pat::parse_multi, src).unwrap();
        let mut out = Vec::new();
        pat_bindings(&pat, &mut out);
        out
    }

    #[test]
    fn identifier_patterns_follow_binding_rules() {
        assert_eq!(names("repo"), ["repo"]);
        assert_eq!(names("MAX"), Vec::<String>::new());
        assert_eq!(names("ref mut Big"), ["Big"]);
        assert_eq!(names("(a, [b, ..], S { c, d: e })"), ["a", "b", "c", "e"]);
        assert_eq!(names("Some(x) | Other(x)"), ["x", "x"]);
        assert_eq!(names("whole @ Some(inner)"), ["whole", "inner"]);
        assert_eq!(names("1..=9"), Vec::<String>::new());
    }

    #[test]
    fn scopes_hide_names_after_pop_but_keep_params() {
        let mut s = Scopes::with_params(&["p".to_string()]);
        s.push();
        s.bind(&syn::parse_quote!(inner));
        assert!(s.is_local("inner") && s.is_local("p"));
        s.pop();
        assert!(!s.is_local("inner"));
        s.pop();
        assert!(s.is_local("p"));
    }
}
