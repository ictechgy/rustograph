//! 본문 타입 위치가 도달성·영향 그래프에서 사라지지 않는지 실제 수확으로 검증한다.

use rustograph::{graph::EdgeKind, source};

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
