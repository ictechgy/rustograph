//! 대형 실제 워크스페이스에서 확인한 서로 다른 선언의 ID 충돌을 독립 fixture로 재현한다.

use rustograph::graph::{EdgeKind, Kind};
use rustograph::source;
use std::collections::BTreeSet;

#[test]
fn distinct_impls_cfg_and_namespaces_keep_source_identity() {
    let root =
        std::env::temp_dir().join(format!("rustograph-symbol-identity-{}", std::process::id()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='identity_case'\nversion='0.1.0'\nedition='2021'\n[workspace]\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        r#"pub struct Value;
fn string_body() {}
fn str_body() {}
impl From<String> for Value {
    fn from(_: String) -> Self { string_body(); Self }
}
impl From<&str> for Value {
    fn from(_: &str) -> Self { str_body(); Self }
}
pub struct Generic<T>(T);
impl Generic<u8> { pub fn pick(&self) { string_body() } }
impl Generic<u16> { pub fn pick(&self) { str_body() } }
const _: () = ();
const _: () = ();
#[cfg(unix)] pub fn configured() { string_body() }
#[cfg(not(unix))] pub fn configured() { str_body() }
pub fn use_configured() { configured() }
pub mod shared { pub fn member() {} }
pub fn shared() { string_body() }
pub fn use_shared() { shared(); shared::member() }
mod first { pub trait Trait { fn run(&self); } }
mod second { pub trait Trait { fn run(&self); } }
impl first::Trait for Value { fn run(&self) { string_body() } }
impl second::Trait for Value { fn run(&self) { str_body() } }
impl Value {
    #[cfg(unix)] pub fn conditional() { string_body() }
    #[cfg(not(unix))] pub fn conditional() { str_body() }
}
"#,
    )
    .unwrap();
    let d = source::load(
        &root,
        &source::Options {
            symbol_level: true,
            ..Default::default()
        },
    )
    .unwrap();
    let _ = std::fs::remove_dir_all(root);
    let ids: BTreeSet<_> = d.vertices.iter().map(|v| v.id.as_str()).collect();
    assert_eq!(
        ids.len(),
        d.vertices.len(),
        "distinct source declarations must never share a vertex ID"
    );
    let from: Vec<_> = d
        .vertices
        .iter()
        .filter(|v| v.kind == Kind::Method && v.id.ends_with("::from"))
        .collect();
    assert_eq!(from.len(), 2);
    for vertex in from {
        let target = match vertex
            .position
            .as_deref()
            .unwrap()
            .rsplit_once(':')
            .unwrap()
            .1
        {
            "5" => "string_body",
            "8" => "str_body",
            other => panic!("unexpected implementation position {other}"),
        };
        assert!(d.edges.iter().any(|e| e.from == vertex.id
            && e.to == format!("identity_case::{target}")
            && e.kind == EdgeKind::Call
            && !e.tentative));
        assert_eq!(
            d.edges
                .iter()
                .filter(|e| e.from == vertex.id && e.kind == EdgeKind::Call)
                .count(),
            1,
            "one implementation must not inherit the other body"
        );
    }
    let configured: Vec<_> = d
        .vertices
        .iter()
        .filter(|v| v.id.ends_with("::configured"))
        .collect();
    assert_eq!(
        configured.len(),
        2,
        "cfg alternatives are preserved independently"
    );
    assert!(configured.iter().all(|v| v.cfg.is_some()));
    for target in configured {
        assert!(
            d.edges
                .iter()
                .any(|e| e.from == "identity_case::use_configured"
                    && e.to == target.id
                    && e.tentative),
            "syntax cannot select a cfg alternative"
        );
    }
    assert!(d
        .edges
        .iter()
        .all(|e| ids.contains(e.from.as_str()) && ids.contains(e.to.as_str())));
    let shared_fn = d
        .vertices
        .iter()
        .find(|v| v.kind == Kind::Fn && v.id.ends_with("::shared"))
        .unwrap();
    assert!(d.edges.iter().any(|e| e.from == "identity_case::use_shared"
        && e.to == shared_fn.id
        && e.kind == EdgeKind::Call
        && !e.tentative));
    assert!(d.edges.iter().any(|e| e.from == "identity_case::use_shared"
        && e.to == "identity_case::shared::member"
        && e.kind == EdgeKind::Call));
    let conditional: Vec<_> = d
        .vertices
        .iter()
        .filter(|v| v.id.ends_with("::conditional"))
        .collect();
    assert_eq!(conditional.len(), 2);
    assert!(
        conditional.iter().all(|v| v.cfg.is_some()),
        "method-owned cfg must survive source separation"
    );
    let unix = d.for_target(
        "aarch64-apple-darwin",
        &source::target_facts("aarch64-apple-darwin"),
    );
    assert_eq!(
        unix.vertices
            .iter()
            .filter(|v| v.id.ends_with("::conditional"))
            .count(),
        1
    );
}
