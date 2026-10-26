//! 대형 실제 워크스페이스에서 확인한 서로 다른 선언의 ID 충돌을 독립 fixture로 재현한다.

use rustograph::graph::{EdgeKind, Kind};
use rustograph::source;
use std::collections::BTreeSet;

#[test]
fn one_physical_impl_file_keeps_distinct_module_binding_contexts() {
    let root =
        std::env::temp_dir().join(format!("rustograph-shared-context-{}", std::process::id()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='shared_context'\nversion='0.1.0'\nedition='2021'\n[workspace]\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "pub struct S;\npub mod a;\npub mod b;\n",
    )
    .unwrap();
    for (name, value) in [("a", 1), ("b", 2)] {
        std::fs::write(root.join(format!("src/{name}.rs")), format!("pub trait Tr {{ fn m(&self)->u32; }}\n#[path=\"shared.rs\"] pub mod inner;\npub fn probe()->u32 {{ {value} }}\npub fn marker() {{}}\n")).unwrap();
    }
    std::fs::write(root.join("src/shared.rs"), "use super::Tr;\nimpl Tr for crate::S { #[cfg_attr(any(), super::marker)] fn m(&self)->u32 { super::probe() } }\n").unwrap();
    let meta = rustograph::cargo_meta::load(&root).unwrap();
    let (d, spans) = source::owner_spans(&root, &meta).unwrap();
    let methods: Vec<_> = d
        .vertices
        .iter()
        .filter(|v| {
            v.kind == Kind::Method
                && v.position
                    .as_deref()
                    .is_some_and(|p| p.contains("src/shared.rs:"))
        })
        .collect();
    assert_eq!(methods.len(), 2);
    assert_ne!(methods[0].id, methods[1].id);
    for (module, other) in [("a", "b"), ("b", "a")] {
        let method = methods
            .iter()
            .find(|v| v.module == format!("shared_context::{module}::inner"))
            .unwrap();
        for target in ["probe", "marker"] {
            assert!(d.edges.iter().any(|e| e.from == method.id
                && e.to == format!("shared_context::{module}::{target}")
                && !e.tentative));
            assert!(!d.edges.iter().any(
                |e| e.from == method.id && e.to == format!("shared_context::{other}::{target}")
            ));
        }
        assert!(
            spans
                .iter()
                .any(|span| span.id == method.id && span.file.ends_with("shared.rs")),
            "public source spans retain each context's exact ID"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

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
#[cfg(unix)] pub struct Choice;
#[cfg(not(unix))] pub struct Choice;
mod helper_one { pub fn help() {} }
mod helper_two { pub fn help() {} }
impl Choice { pub fn act() { helper_one::help() } }
#[cfg(unix)] pub trait ConditionalParent { fn route() { helper_one::help() } }
#[cfg(not(unix))] pub trait ConditionalParent { fn route() { helper_two::help() } }
"#,
    )
    .unwrap();
    let d = source::load(
        &root,
        &source::Options {
            symbol_level: true,
            tests: true,
            extra_roots: vec![
                "identity_case::Value::<From>::from".to_string(),
                "identity_case::Choice::act".to_string(),
            ],
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
    let choices: Vec<_> = types
        .vertices
        .iter()
        .filter(|v| v.kind == Kind::Struct && v.id.ends_with("::Choice"))
        .collect();
    assert_eq!(choices.len(), 2);
    for choice in &choices {
        assert!(
            types.edges.iter().any(|e| e.from == choice.id
                && e.to == "identity_case::helper_one"
                && e.kind == EdgeKind::Call
                && e.tentative),
            "a shared method must project to every possible type owner"
        );
        assert!(
            types.roots.contains(&choice.id),
            "a shared source root projects to all candidate owners"
        );
        assert!(!types.edges.iter().any(|e| e.from == choice.id && e.kind == EdgeKind::Contains), "collapsing shared method ownership must not invent type-to-type or type-to-crate containment");
    }
    let parents: Vec<_> = types
        .vertices
        .iter()
        .filter(|v| v.kind == Kind::Trait && v.id.ends_with("::ConditionalParent"))
        .collect();
    assert_eq!(parents.len(), 2);
    for parent in parents {
        let helper = if parent.cfg.as_ref().unwrap().contains("not") {
            "identity_case::helper_two"
        } else {
            "identity_case::helper_one"
        };
        assert!(
            types.edges.iter().any(|e| e.from == parent.id
                && e.to == helper
                && e.kind == EdgeKind::Call
                && !e.tentative),
            "known trait member ownership must project to the exact declaration"
        );
        let other = if helper.ends_with("one") {
            "identity_case::helper_two"
        } else {
            "identity_case::helper_one"
        };
        assert!(!types
            .edges
            .iter()
            .any(|e| e.from == parent.id && e.to == other && e.kind == EdgeKind::Call));
    }
    assert!(types
        .limitations
        .iter()
        .any(|l| l.contains("project to multiple owners")));
}
