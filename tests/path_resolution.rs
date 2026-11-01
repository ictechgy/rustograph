//! `use` 임포트와 경로 접두사 해석이 실제 수확에서 간선을 잃지 않는지 검증한다.

use rustograph::graph::{Document, EdgeKind};
use rustograph::{analysis, source};

/// 임시 디렉터리의 단일 패키지 워크스페이스를 심볼 레벨로 수확하고 지운다.
fn harvest(tag: &str, main: &str) -> Document {
    let root = std::env::temp_dir().join(format!(
        "rustograph-path-resolution-{tag}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        format!("[package]\nname='{tag}'\nversion='0.1.0'\nedition='2021'\n[workspace]\n"),
    )
    .unwrap();
    std::fs::write(root.join("src/main.rs"), main).unwrap();
    let document = source::load(
        &root,
        &source::Options {
            symbol_level: true,
            ..Default::default()
        },
    );
    std::fs::remove_dir_all(&root).unwrap();
    document.unwrap()
}

fn has_edge(doc: &Document, from: &str, to: &str, kind: EdgeKind) -> bool {
    doc.edges
        .iter()
        .any(|e| e.from == from && e.to == to && e.kind == kind)
}

/// `use a::b::{self}`·`{self as x}`는 모듈 `b`를 들인다 — 그 이름으로 시작하는 호출이 해석되고
/// 모듈 `uses` 간선이 생긴다. 이전에는 `b::self`를 찾다 임포트가 사라져 중첩 모듈의 함수가
/// unreachable로 나오고, 모듈 레벨 `uses`(rules·cycles의 입력)가 빠졌다.
#[test]
fn use_group_self_import_resolves_calls_and_uses_edges() {
    let doc = harvest(
        "selfimp",
        r#"
mod outer { pub mod inner { pub fn f() {} pub fn g() {} } }
mod a {
    use crate::outer::inner::{self};
    pub fn call() { inner::f(); }
}
mod b {
    use crate::outer::inner::{self as renamed};
    pub fn call() { renamed::g(); }
}
fn main() { a::call(); b::call(); }
"#,
    );
    assert!(has_edge(
        &doc,
        "selfimp::a::call",
        "selfimp::outer::inner::f",
        EdgeKind::Call
    ));
    assert!(has_edge(
        &doc,
        "selfimp::b::call",
        "selfimp::outer::inner::g",
        EdgeKind::Call
    ));
    for module in ["selfimp::a", "selfimp::b"] {
        assert!(has_edge(
            &doc,
            module,
            "selfimp::outer::inner",
            EdgeKind::Uses
        ));
    }
    let rep = analysis::dead(&doc).unwrap();
    assert!(rep.findings.is_empty(), "{:?}", rep.findings);
}

/// 같은 모듈에 `fn util`과 `use crate::util`이 함께 있으면 `util::g()`의 `util`은 임포트한 모듈이다
/// (경로 접두사는 타입 이름공간). 한 세그먼트 호출 `util(1)`은 지금처럼 그 fn이다. 이전에는 fn이
/// 접두사를 가려 `util::g()`가 미해석이 되고 `util::g`가 unreachable로 나왔다.
#[test]
fn value_item_does_not_hide_an_imported_module_prefix() {
    let doc = harvest(
        "valueprefix",
        r#"
mod util { pub fn g() {} }
mod b {
    use crate::util;
    fn util(x: u8) -> u8 { x }
    pub fn caller() { util::g(); let _ = util(1); }
}
fn main() { b::caller(); }
"#,
    );
    assert!(has_edge(
        &doc,
        "valueprefix::b::caller",
        "valueprefix::util::g",
        EdgeKind::Call
    ));
    assert!(has_edge(
        &doc,
        "valueprefix::b::caller",
        "valueprefix::b::util",
        EdgeKind::Call
    ));
    let rep = analysis::dead(&doc).unwrap();
    assert!(rep.findings.is_empty(), "{:?}", rep.findings);
}
