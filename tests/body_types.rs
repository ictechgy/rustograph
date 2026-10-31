//! 본문 타입 위치가 도달성·영향 그래프에서 사라지지 않는지 실제 수확으로 검증한다.

use rustograph::{graph::EdgeKind, source};

#[test]
fn review_nested_item_generics_do_not_resolve_to_global_symbols() {
    let root = std::env::temp_dir().join(format!(
        "rustograph-nested-type-scope-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='nested_scope'\nversion='0.1.0'\nedition='2021'\n[workspace]\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/main.rs"),
        r#"
struct T;
const N: usize = 2;
struct Actual;
fn main() {
    { fn inner<T, const N: usize>() { let _: T; let _: [u8; N]; } }
    let _: Actual;
}
"#,
    )
    .unwrap();
    let document = source::load(
        &root,
        &source::Options {
            symbol_level: true,
            ..Default::default()
        },
    )
    .unwrap();
    std::fs::remove_dir_all(root).unwrap();
    for target in ["T", "N"] {
        assert!(
            !document
                .edges
                .iter()
                .any(|edge| edge.from == "nested_scope::main"
                    && edge.to == format!("nested_scope::{target}")),
            "nested generic resolved to global {target}"
        );
    }
    assert!(document
        .edges
        .iter()
        .any(|edge| edge.from == "nested_scope::main"
            && edge.to == "nested_scope::Actual"
            && edge.kind == EdgeKind::References));
}

#[test]
fn review_type_generic_does_not_hide_a_bare_value_function() {
    let root =
        std::env::temp_dir().join(format!("rustograph-value-namespace-{}", std::process::id()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='value_namespace'\nversion='0.1.0'\nedition='2021'\n[workspace]\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/main.rs"),
        "fn T() {}\nfn generic<T>() { T(); }\nfn main() { generic::<u8>(); }\n",
    )
    .unwrap();
    let document = source::load(
        &root,
        &source::Options {
            symbol_level: true,
            ..Default::default()
        },
    )
    .unwrap();
    std::fs::remove_dir_all(root).unwrap();
    assert!(document
        .edges
        .iter()
        .any(|edge| edge.from == "value_namespace::generic"
            && edge.to == "value_namespace::T"
            && edge.kind == EdgeKind::Call));
    assert!(!rustograph::analysis::dead(&document)
        .unwrap()
        .findings
        .iter()
        .any(|finding| finding.id == "value_namespace::T"));
}

#[test]
fn feedback_type_walk_keeps_const_expressions_and_generic_shadowing() {
    let root =
        std::env::temp_dir().join(format!("rustograph-body-type-scope-{}", std::process::id()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='body_scope'\nversion='0.1.0'\nedition='2021'\n[workspace]\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/main.rs"),
        r#"
struct T;
const N: usize = 2;
const fn length() -> usize { 2 }
struct Holder<const K: usize>;
macro_rules! body_type { () => { u8 } }
fn main() {
    let _: [u8; N];
    let _: Holder<{ length() }>;
    let _: body_type!();
    let _ = |_: T| -> T { loop {} };
}
fn shadow<T, const N: usize>() { let _: T; let _: [u8; N]; }
"#,
    )
    .unwrap();
    let document = source::load(
        &root,
        &source::Options {
            symbol_level: true,
            ..Default::default()
        },
    )
    .unwrap();
    std::fs::remove_dir_all(root).unwrap();
    for (target, kind) in [
        ("N", EdgeKind::References),
        ("length", EdgeKind::Call),
        ("body_type", EdgeKind::Call),
        ("T", EdgeKind::References),
    ] {
        assert!(
            document
                .edges
                .iter()
                .any(|edge| edge.from == "body_scope::main"
                    && edge.to == format!("body_scope::{target}")
                    && edge.kind == kind),
            "missing type-position dependency {target}"
        );
    }
    for target in ["T", "N"] {
        assert!(
            !document
                .edges
                .iter()
                .any(|edge| edge.from == "body_scope::shadow"
                    && edge.to == format!("body_scope::{target}")),
            "generic parameter resolved to global {target}"
        );
    }
}

#[test]
fn feedback_body_type_positions_keep_reference_edges() {
    let root = std::env::temp_dir().join(format!("rustograph-body-types-{}", std::process::id()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='body_types'\nversion='0.1.0'\nedition='2021'\n[workspace]\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/main.rs"),
        r#"
struct A;
struct B;
type C = u32;
type D = u8;
struct E;
trait Tr { fn f(); }
impl Tr for E { fn f() {} }
fn main() {
    let _: Vec<A> = Vec::new();
    let _ = Vec::<B>::new();
    let _ = "1".parse::<C>();
    let _ = 0 as D;
    <E as Tr>::f();
}
"#,
    )
    .unwrap();
    let document = source::load(
        &root,
        &source::Options {
            symbol_level: true,
            ..Default::default()
        },
    )
    .unwrap();
    std::fs::remove_dir_all(root).unwrap();
    for name in ["A", "B", "C", "D", "E", "Tr"] {
        assert!(
            document
                .edges
                .iter()
                .any(|edge| edge.from == "body_types::main"
                    && edge.to == format!("body_types::{name}")
                    && edge.kind == EdgeKind::References
                    && !edge.tentative),
            "missing body type {name}"
        );
    }
}
