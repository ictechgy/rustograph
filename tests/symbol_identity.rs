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
pub trait ConditionalTrait {
    #[cfg(unix)] fn platform();
    #[cfg(not(unix))] fn platform();
}
mod marks { pub fn first() {} pub fn second() {} }
pub struct AttrValue;
impl From<String> for AttrValue {
    #[cfg_attr(any(), marks::first)]
    fn from(_: String) -> Self { string_body(); Self }
}
impl From<&str> for AttrValue {
    #[cfg_attr(any(), marks::second)]
    fn from(_: &str) -> Self { str_body(); Self }
}
#[cfg(unix)] #[no_mangle] pub extern "C" fn cfg_entry() {}
#[cfg(not(unix))] pub extern "C" fn cfg_entry() {}
#[cfg(unix)] #[test] fn cfg_test() {}
#[cfg(not(unix))] fn cfg_test() {}
pub struct Same { x: i32 }
#[allow(non_snake_case)] pub fn Same() {}
impl Same { pub fn method() { string_body() } }
pub fn use_same() { Same() }
"#,
    )
    .unwrap();
    let d = source::load(
        &root,
        &source::Options {
            symbol_level: true,
            tests: true,
            extra_roots: vec!["identity_case::Value::<From>::from".to_string()],
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
    assert!(
        d.limitations
            .iter()
            .any(|l| l.contains("1 requested root IDs do not exist")),
        "a stale ID must not be silently accepted as a root"
    );
    let from: Vec<_> = d
        .vertices
        .iter()
        .filter(|v| {
            v.kind == Kind::Method
                && v.id.starts_with("identity_case::Value::<From>::")
                && v.id.ends_with("::from")
        })
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
    let trait_methods: Vec<_> = d
        .vertices
        .iter()
        .filter(|v| v.id.ends_with("::platform"))
        .collect();
    assert_eq!(
        trait_methods.len(),
        2,
        "cfg trait declarations need their own spans even without bodies"
    );
    assert_ne!(trait_methods[0].id, trait_methods[1].id);
    assert!(trait_methods.iter().all(|v| v.cfg.is_some()));
    assert_eq!(
        unix.vertices
            .iter()
            .filter(|v| v.id.ends_with("::platform"))
            .count(),
        1
    );
    let attr_methods: Vec<_> = d
        .vertices
        .iter()
        .filter(|v| {
            v.id.starts_with("identity_case::AttrValue::<From>::") && v.id.ends_with("::from")
        })
        .collect();
    assert_eq!(attr_methods.len(), 2);
    for method in attr_methods {
        let first = d.edges.iter().any(|e| {
            e.from == method.id && e.kind == EdgeKind::Call && e.to == "identity_case::string_body"
        });
        let (expected, other) = if first {
            ("first", "second")
        } else {
            ("second", "first")
        };
        assert!(d.edges.iter().any(|e| e.from == method.id
            && e.to == format!("identity_case::marks::{expected}")
            && !e.tentative));
        assert!(
            !d.edges
                .iter()
                .any(|e| e.from == method.id && e.to == format!("identity_case::marks::{other}")),
            "a known attribute owner must never fan to another declaration"
        );
    }
    for name in ["cfg_entry", "cfg_test"] {
        let roots: Vec<_> = d
            .roots
            .iter()
            .filter(|id| id.ends_with(&format!("::{name}")))
            .collect();
        assert_eq!(
            roots.len(),
            1,
            "only the annotated source instance is a root"
        );
        let root_vertex = d.vertices.iter().find(|v| &v.id == roots[0]).unwrap();
        assert!(!root_vertex.cfg.as_ref().unwrap().contains("not"));
    }
    let same_fn = d
        .vertices
        .iter()
        .find(|v| v.kind == Kind::Fn && v.id.ends_with("::Same"))
        .unwrap();
    assert!(d.edges.iter().any(|e| e.from == "identity_case::Same"
        && e.to == "identity_case::Same::method"
        && e.kind == EdgeKind::Contains));
    assert!(!d.edges.iter().any(|e| e.from == same_fn.id
        && e.to == "identity_case::Same::method"
        && e.kind == EdgeKind::Contains));
    assert!(d.edges.iter().any(|e| e.from == "identity_case::use_same"
        && e.to == same_fn.id
        && e.kind == EdgeKind::Call
        && !e.tentative));
    let types = d.view(rustograph::graph::Level::Type).unwrap();
    assert!(types
        .vertices
        .iter()
        .any(|v| v.id == "identity_case::Same" && v.kind == Kind::Struct));
    assert!(types.edges.iter().any(|e| e.from == "identity_case::Same"
        && e.to == "identity_case"
        && e.kind == EdgeKind::Call));
}
