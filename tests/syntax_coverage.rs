//! 구문 수확이 놓치던 흔한 Rust 형태를 실제 수확으로 고정한다 — `Self` 경로, lib 계열 crate type·
//! proc-macro 크레이트, 워크스페이스 밖 self 타입의 impl 본문, 필드 타입, 링커 진입점 속성, 함수 안
//! 아이템과 `macro_rules!` 본문, UFCS 호출. 모두 "살아 있는 코드가 unreachable로 보이거나 의존이
//! 그래프에서 사라지는" 쪽의 결함이라, 고친 뒤에도 보존을 좁히지 않는지를 함께 본다.

use rustograph::graph::{Document, EdgeKind};
use rustograph::{analysis, deps, source};
use std::path::{Path, PathBuf};

/// 테스트마다 임시 디렉터리에 만든 워크스페이스 — 끝나면 지운다.
struct Workspace(PathBuf);

impl Workspace {
    /// `files`의 (상대 경로, 내용)으로 워크스페이스를 만든다. 태그는 병렬 테스트 사이 충돌을 막는다.
    fn new(tag: &str, files: &[(&str, &str)]) -> Workspace {
        let root = std::env::temp_dir().join(format!(
            "rustograph-syntax-coverage-{tag}-{}",
            std::process::id()
        ));
        if root.exists() {
            std::fs::remove_dir_all(&root).unwrap();
        }
        for (path, text) in files {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        Workspace(root)
    }

    /// 워크스페이스 루트.
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

/// 단일 패키지 워크스페이스 매니페스트.
fn manifest(name: &str) -> String {
    format!("[package]\nname='{name}'\nversion='0.1.0'\nedition='2021'\n[workspace]\n")
}

/// 심볼 레벨 문서.
fn symbols(dir: &Path) -> Document {
    source::load(
        dir,
        &source::Options {
            symbol_level: true,
            ..Default::default()
        },
    )
    .unwrap()
}

/// 외부 트레이트 impl 보존을 켠 dead 선택지.
fn retaining_external_trait_impls() -> analysis::DeadOptions {
    let mut opts = analysis::DeadOptions::default();
    opts.retain_external_trait_impls = true;
    opts
}

/// 보고된 정점 ID 목록.
fn unreachable_ids(report: &analysis::DeadReport) -> Vec<String> {
    report.findings.iter().map(|f| f.id.clone()).collect()
}

/// `ids` 가운데 하나라도 보고됐으면 실패한다.
fn assert_not_reported(report: &analysis::DeadReport, ids: &[&str]) {
    let found = unreachable_ids(report);
    for id in ids {
        assert!(
            !found.iter().any(|f| f == id),
            "{id} is reported unreachable: {found:?}"
        );
    }
}

/// `ids`가 모두 보고됐는지 — 보존을 넓히다 진짜 미사용까지 가리지 않았는지 본다.
fn assert_reported(report: &analysis::DeadReport, ids: &[&str]) {
    let found = unreachable_ids(report);
    for id in ids {
        assert!(
            found.iter().any(|f| f == id),
            "{id} is missing from {found:?}"
        );
    }
}

/// `from`에서 `to`로 가는 해당 종류의 간선이 있는가.
fn has_edge(doc: &Document, from: &str, to: &str, kind: EdgeKind) -> bool {
    doc.edges
        .iter()
        .any(|e| e.from == from && e.to == to && e.kind == kind)
}

/// impl 본문의 `Self::f()`·`Self(..)`·`Self { .. }`·`-> Self`는 impl self 타입으로 해석된다.
#[test]
fn self_paths_resolve_to_the_impl_type() {
    let ws = Workspace::new(
        "self",
        &[
            ("Cargo.toml", &manifest("selfcall")),
            (
                "src/main.rs",
                r#"
pub struct D { pub n: u32 }
impl D {
    pub fn a(&self) -> bool { Self::b(self.n) }
    fn b(n: u32) -> bool { n > 0 }
    pub fn c(&self) -> bool { Some(self.n).is_some_and(|v| Self::d(v)) }
    fn d(n: u32) -> bool { n > 1 }
    pub fn make() -> Self { Self { n: helper_in_literal() } }
}
fn helper_in_literal() -> u32 { 1 }
fn never_called() {}
fn main() {
    let x = D::make();
    x.a();
    x.c();
}
"#,
            ),
        ],
    );
    let doc = symbols(ws.path());
    assert!(has_edge(
        &doc,
        "selfcall::D::a",
        "selfcall::D::b",
        EdgeKind::Call
    ));
    assert!(has_edge(
        &doc,
        "selfcall::D::make",
        "selfcall::D",
        EdgeKind::References
    ));
    assert!(has_edge(
        &doc,
        "selfcall::D::make",
        "selfcall::D",
        EdgeKind::Signature
    ));
    let report = analysis::dead(&doc).unwrap();
    assert_not_reported(&report, &["selfcall::D::b", "selfcall::D::d"]);
    assert_reported(&report, &["selfcall::never_called"]);
}

/// RAII 관용구 — `Self(1)` 생성자로만 만드는 타입의 `Drop`은 외부 트레이트 impl 보존 선택지로 산다.
#[test]
fn self_constructor_keeps_raii_drop_reachable() {
    let ws = Workspace::new(
        "guard",
        &[
            ("Cargo.toml", &manifest("guard")),
            (
                "src/main.rs",
                r#"
pub struct Guard(u32);
impl Guard {
    pub fn new() -> Self { Self(1) }
}
impl Drop for Guard {
    fn drop(&mut self) { cleanup(); }
}
fn cleanup() {}
fn main() {
    let _g = Guard::new();
}
"#,
            ),
        ],
    );
    let doc = symbols(ws.path());
    let report = analysis::dead_with_options(&doc, retaining_external_trait_impls()).unwrap();
    assert_not_reported(
        &report,
        &[
            "guard::Guard",
            "guard::Guard::<Drop>::drop",
            "guard::cleanup",
        ],
    );
}

/// 함수 안 `impl`의 `Self`는 바깥 impl의 self 타입이 아니다 — 안쪽 아이템이 바깥 `Self`를 빌려
/// 쓰지 못하는 rustc 규칙대로, 바깥 타입으로 간선을 만들지 않는다.
#[test]
fn nested_item_self_does_not_borrow_the_outer_impl() {
    let ws = Workspace::new(
        "nested-self",
        &[
            ("Cargo.toml", &manifest("nestedself")),
            (
                "src/main.rs",
                r#"
pub struct Outer;
impl Outer {
    pub fn run(&self) {
        struct Inner;
        impl Inner { fn go() -> u32 { Self::value() } fn value() -> u32 { 1 } }
        Inner::go();
    }
    fn value() -> u32 { 2 }
}
fn main() { Outer.run(); }
"#,
            ),
        ],
    );
    let doc = symbols(ws.path());
    assert!(
        !has_edge(
            &doc,
            "nestedself::Outer::run",
            "nestedself::Outer::value",
            EdgeKind::Call
        ),
        "inner Self resolved to the outer impl: {:?}",
        doc.edges
    );
}

/// `[lib] crate-type`을 적은 lib(cdylib·rlib)과 proc-macro 크레이트도 수확한다 — 예전에는 cargo
/// metadata의 kind가 `lib`이 아니라는 이유로 크레이트가 통째로 빠졌고, deps는 그 크레이트가 실제로
/// 쓰는 의존을 unused로 보고했다.
#[test]
fn lib_crate_types_and_proc_macros_are_harvested() {
    let ws = Workspace::new(
        "kinds",
        &[
            (
                "Cargo.toml",
                "[workspace]\nmembers=['cdy','mixed','pm','app','util']\nresolver='2'\n",
            ),
            (
                "util/Cargo.toml",
                "[package]\nname='util'\nversion='0.1.0'\nedition='2021'\n",
            ),
            ("util/src/lib.rs", "pub fn u() {}\n"),
            (
                "cdy/Cargo.toml",
                "[package]\nname='cdy'\nversion='0.1.0'\nedition='2021'\n[lib]\ncrate-type=['cdylib']\n[dependencies]\nutil={path='../util'}\n",
            ),
            (
                "cdy/src/lib.rs",
                "fn cdy_helper() { util::u(); }\n#[no_mangle]\npub extern \"C\" fn cdy_entry() { cdy_helper(); }\nfn cdy_really_dead() {}\n",
            ),
            (
                "mixed/Cargo.toml",
                "[package]\nname='mixed'\nversion='0.1.0'\nedition='2021'\n[lib]\ncrate-type=['cdylib','rlib']\n",
            ),
            (
                "mixed/src/lib.rs",
                "pub fn mixed_api() {}\nfn mixed_really_dead() {}\n",
            ),
            (
                "pm/Cargo.toml",
                "[package]\nname='pm'\nversion='0.1.0'\nedition='2021'\n[lib]\nproc-macro=true\n[dependencies]\nutil={path='../util'}\n",
            ),
            (
                "pm/src/lib.rs",
                "extern crate proc_macro;\nuse proc_macro::TokenStream;\nfn pm_helper(t: TokenStream) -> TokenStream { util::u(); t }\n#[proc_macro]\npub fn mk(t: TokenStream) -> TokenStream { pm_helper(t) }\nfn pm_really_dead() {}\n",
            ),
            (
                "app/Cargo.toml",
                "[package]\nname='app'\nversion='0.1.0'\nedition='2021'\n[dependencies]\nmixed={path='../mixed'}\npm={path='../pm'}\n",
            ),
            ("app/src/main.rs", "fn main() { mixed::mixed_api(); }\n"),
        ],
    );
    let doc = symbols(ws.path());
    for id in ["cdy::cdy_entry", "mixed::mixed_api", "pm::mk"] {
        assert!(
            doc.vertices.iter().any(|v| v.id == id),
            "{id} was not harvested"
        );
    }
    let report = analysis::dead(&doc).unwrap();
    assert_not_reported(&report, &["cdy::cdy_helper", "pm::pm_helper"]);
    assert_reported(
        &report,
        &[
            "cdy::cdy_really_dead",
            "mixed::mixed_really_dead",
            "pm::pm_really_dead",
        ],
    );
    let deps = deps::report(ws.path()).unwrap();
    let unused: Vec<String> = deps
        .unused
        .iter()
        .map(|u| format!("{} -> {}", u.package, u.dep))
        .collect();
    assert!(
        !unused
            .iter()
            .any(|u| u == "cdy -> util" || u == "pm -> util"),
        "used dependencies reported unused: {unused:?}"
    );
}

/// self 타입이 워크스페이스 정점이 아닌 impl(`impl Tr for String`, blanket `impl<T> Tr for T`,
/// `impl Tr for &Local`)의 본문 간선은 버리지 않는다 — 워크스페이스 트레이트의 선언 메서드가 그
/// 본문을 대신 소유해, 그 메서드를 부르는 곳에서 본문의 호출이 닿고 의존이 규칙·순환에 보인다.
#[test]
fn foreign_self_type_impl_bodies_keep_their_edges() {
    let ws = Workspace::new(
        "extimpl",
        &[
            ("Cargo.toml", &manifest("extimpl")),
            (
                "src/main.rs",
                r#"
mod ui;
mod db;
pub trait Render { fn render(&self) -> String; }
impl Render for String {
    fn render(&self) -> String { db::query_from_string_impl() }
}
impl Render for Vec<u8> {
    fn render(&self) -> String { ui::from_vec_impl() }
}
pub struct Local;
impl Render for &Local {
    fn render(&self) -> String { ui::from_ref_impl() }
}
pub trait Greet { fn greet(&self); }
impl<T: Render> Greet for T {
    fn greet(&self) { only_from_blanket(); }
}
fn only_from_blanket() {}
impl From<Local> for String {
    fn from(_: Local) -> String { db::only_from_foreign_trait() }
}
fn main() {
    let s = String::new();
    println!("{}", s.render());
    s.greet();
}
"#,
            ),
            (
                "src/db.rs",
                "pub fn query_from_string_impl() -> String { String::new() }\npub fn only_from_foreign_trait() -> String { String::new() }\n",
            ),
            (
                "src/ui.rs",
                "pub fn from_vec_impl() -> String { String::new() }\npub fn from_ref_impl() -> String { String::new() }\n",
            ),
        ],
    );
    let doc = symbols(ws.path());
    assert!(has_edge(
        &doc,
        "extimpl::Render::render",
        "extimpl::db::query_from_string_impl",
        EdgeKind::Call
    ));
    // 워크스페이스 밖 트레이트를 워크스페이스 밖 타입에 구현하면 impl 모듈만 본문을 소유한다 —
    // 의존은 보인다. 모듈은 `--retain-public`일 때만 보존 루트라 기본 dead 판정은 바뀌지 않는다.
    assert!(has_edge(
        &doc,
        "extimpl",
        "extimpl::db::only_from_foreign_trait",
        EdgeKind::Call
    ));
    let report = analysis::dead(&doc).unwrap();
    assert_reported(&report, &["extimpl::db::only_from_foreign_trait"]);
    assert_not_reported(
        &report,
        &[
            "extimpl::db::query_from_string_impl",
            "extimpl::ui::from_vec_impl",
            "extimpl::ui::from_ref_impl",
            "extimpl::only_from_blanket",
            // 워크스페이스 밖 타입에만 구현한 트레이트도 그 메서드를 쓰면 닿는다.
            "extimpl::Render",
            "extimpl::Greet",
        ],
    );
    assert!(
        doc.limitations
            .iter()
            .any(|l| l.starts_with("5 impl blocks have a self type outside the workspace graph")),
        "{:?}",
        doc.limitations
    );
}

/// struct·enum 필드의 타입은 그 타입 정점에서 나가는 references 간선이다 — 없으면 DTO 필드로만
/// 쓰는 타입이 unreachable로 보이고 필드가 다른 모듈을 가리키는 의존이 규칙에서 사라진다.
#[test]
fn field_types_are_references_from_the_owning_type() {
    let ws = Workspace::new(
        "fields",
        &[
            ("Cargo.toml", &manifest("fields")),
            (
                "src/main.rs",
                r#"
mod db;
pub struct Inner;
pub enum Kind { A, B(Payload) }
pub struct Payload;
pub struct Outer<T> { inner: Vec<Inner>, kind: Option<Kind>, conn: db::Conn, generic: T, me: Option<Box<Self>> }
fn main() {
    let o: Outer<u8> = make();
    let _ = &o;
}
fn make<T>() -> Outer<T> { unimplemented!() }
"#,
            ),
            ("src/db.rs", "pub struct Conn;\n"),
        ],
    );
    let doc = symbols(ws.path());
    for to in ["fields::Inner", "fields::Kind", "fields::db::Conn"] {
        assert!(
            has_edge(&doc, "fields::Outer", to, EdgeKind::References),
            "missing Outer -> {to}"
        );
    }
    assert!(has_edge(
        &doc,
        "fields::Kind",
        "fields::Payload",
        EdgeKind::References
    ));
    assert!(
        !doc.edges
            .iter()
            .any(|e| e.from == "fields::Outer" && e.to == "fields::Outer"),
        "Self in a field must not become a self edge"
    );
    let report = analysis::dead(&doc).unwrap();
    assert_not_reported(
        &report,
        &[
            "fields::Inner",
            "fields::Kind",
            "fields::Payload",
            "fields::db::Conn",
        ],
    );
}

/// 링커·런타임이 부르는 진입점 속성 — Rust 2024의 `#[unsafe(no_mangle)]`, `cfg_attr` 안쪽의
/// `no_mangle`, `#[used]`·`#[no_mangle]`·`#[global_allocator]` static, impl 메서드의 `#[no_mangle]`.
/// 예제·통합 테스트·벤치 타깃은 수확하지 않는다는 사실도 limitation으로 센다.
#[test]
fn linker_entry_attributes_are_retention_roots() {
    let ws = Workspace::new(
        "roots",
        &[
            (
                "Cargo.toml",
                "[package]\nname='roots'\nversion='0.1.0'\nedition='2021'\n[workspace]\n[lib]\npath='src/lib.rs'\n[[bin]]\nname='roots'\npath='src/main.rs'\n",
            ),
            (
                "src/lib.rs",
                r#"
use std::alloc::{GlobalAlloc, Layout, System};
pub fn used_by_main() {}
fn helper_unsafe_mangle() {}
#[unsafe(no_mangle)]
pub extern "C" fn ffi_unsafe_mangle() { helper_unsafe_mangle(); }
fn helper_cfg_attr_mangle() {}
#[cfg_attr(all(), no_mangle)]
pub extern "C" fn ffi_cfg_attr_mangle() { helper_cfg_attr_mangle(); }
fn helper_unsafe_export() {}
#[unsafe(export_name = "renamed")]
pub extern "C" fn ffi_unsafe_export() { helper_unsafe_export(); }
fn used_target() {}
#[used]
static KEEP: fn() = used_target;
#[no_mangle]
pub static EXPORTED_STATIC: u32 = 5;
pub struct MyAlloc;
unsafe impl GlobalAlloc for MyAlloc {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 { unsafe { System.alloc(l) } }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) { unsafe { System.dealloc(p, l) } }
}
#[global_allocator]
static GLOBAL: MyAlloc = MyAlloc;
pub struct S;
fn helper_method_mangle() {}
impl S {
    #[no_mangle]
    pub extern "C" fn method_mangle() { helper_method_mangle(); }
}
fn truly_dead() {}
pub fn only_from_example() {}
"#,
            ),
            ("src/main.rs", "fn main() { roots::used_by_main(); }\n"),
            (
                "examples/demo.rs",
                "fn main() { roots::only_from_example(); }\n",
            ),
            ("tests/it.rs", "#[test]\nfn t() {}\n"),
            ("benches/b.rs", "fn main() {}\n"),
        ],
    );
    let doc = symbols(ws.path());
    let report = analysis::dead(&doc).unwrap();
    assert_not_reported(
        &report,
        &[
            "roots::ffi_unsafe_mangle",
            "roots::helper_unsafe_mangle",
            "roots::ffi_cfg_attr_mangle",
            "roots::helper_cfg_attr_mangle",
            "roots::ffi_unsafe_export",
            "roots::helper_unsafe_export",
            "roots::KEEP",
            "roots::used_target",
            "roots::EXPORTED_STATIC",
            "roots::GLOBAL",
            "roots::MyAlloc",
            "roots::S::method_mangle",
            "roots::helper_method_mangle",
        ],
    );
    assert_reported(&report, &["roots::truly_dead", "roots::only_from_example"]);
}

/// 예제·통합 테스트·벤치 타깃은 수확하지 않는다 — 그 타깃에서만 쓰는 lib 아이템은 `--tests`를 줘도
/// unreachable이므로, 수확하지 않은 타깃 수를 문서 limitation으로 센다. 그런 타깃이 없으면 조용하다.
#[test]
fn unharvested_targets_are_counted_as_a_limitation() {
    let ws = Workspace::new(
        "targets",
        &[
            ("Cargo.toml", &manifest("targets")),
            ("src/lib.rs", "pub fn only_from_example() {}\n"),
            (
                "examples/demo.rs",
                "fn main() { targets::only_from_example(); }\n",
            ),
            ("tests/it.rs", "#[test]\nfn t() {}\n"),
            ("benches/b.rs", "fn main() {}\n"),
        ],
    );
    let doc = symbols(ws.path());
    assert!(
        doc.limitations.iter().any(|l| l
            .starts_with("3 example/test/bench targets of workspace members are not harvested")),
        "{:?}",
        doc.limitations
    );
    let quiet = Workspace::new(
        "targets-quiet",
        &[
            ("Cargo.toml", &manifest("quiet")),
            ("src/lib.rs", "pub fn f() {}\n"),
        ],
    );
    let doc = symbols(quiet.path());
    assert!(
        !doc.limitations
            .iter()
            .any(|l| l.contains("targets of workspace members are not harvested")),
        "{:?}",
        doc.limitations
    );
}

/// 함수 본문 최상위의 아이템(중첩 `fn`, 지역 `impl`)과 `macro_rules!` 정의 본문의 경로도 간선이
/// 된다. 매크로 본문은 토큰이라 호출 위치를 모르니 추정(tentative) 간선이다.
#[test]
fn nested_items_and_macro_rules_bodies_are_visited() {
    let ws = Workspace::new(
        "nested",
        &[
            ("Cargo.toml", &manifest("nested")),
            (
                "src/main.rs",
                r#"
mod m;
macro_rules! call_helper {
    () => { $crate::m::macro_only_helper() };
}
fn helper_from_nested_fn() {}
fn helper_from_local_impl() {}
fn helper_from_local_macro() {}
fn main() {
    fn inner() { helper_from_nested_fn(); }
    inner();
    struct L;
    impl L { fn go(&self) { helper_from_local_impl(); } }
    L.go();
    macro_rules! local { () => { crate::helper_from_local_macro() }; }
    local!();
    call_helper!();
}
"#,
            ),
            ("src/m.rs", "pub fn macro_only_helper() {}\n"),
        ],
    );
    let doc = symbols(ws.path());
    let macro_edge = doc
        .edges
        .iter()
        .find(|e| e.from == "nested::call_helper" && e.to == "nested::m::macro_only_helper")
        .expect("macro_rules body path becomes an edge");
    assert!(macro_edge.tentative, "macro body edges are tentative");
    let report = analysis::dead(&doc).unwrap();
    assert_not_reported(
        &report,
        &[
            "nested::helper_from_nested_fn",
            "nested::helper_from_local_impl",
            "nested::helper_from_local_macro",
            "nested::m::macro_only_helper",
        ],
    );
}

/// UFCS `<T as Tr>::m(..)`와 `<T>::m(..)`는 self 타입이 적힌 정적 호출이다 — 트레이트 선언만이
/// 아니라 그 타입의 impl 메서드로 call 간선이 간다.
#[test]
fn qualified_self_calls_reach_the_impl_method() {
    let ws = Workspace::new(
        "ufcs",
        &[
            ("Cargo.toml", &manifest("ufcs")),
            (
                "src/main.rs",
                r#"
pub trait Shape { fn area(&self) -> u32; }
pub struct Square(pub u32);
impl Shape for Square {
    fn area(&self) -> u32 { only_from_area() }
}
fn only_from_area() -> u32 { 1 }
impl Square { fn side(&self) -> u32 { only_from_side() } }
fn only_from_side() -> u32 { 2 }
fn main() {
    let s = Square(2);
    let _ = <Square as Shape>::area(&s);
    let _ = <Square>::side(&s);
}
"#,
            ),
        ],
    );
    let doc = symbols(ws.path());
    assert!(has_edge(
        &doc,
        "ufcs::main",
        "ufcs::Square::<Shape>::area",
        EdgeKind::Call
    ));
    assert!(has_edge(
        &doc,
        "ufcs::main",
        "ufcs::Square::side",
        EdgeKind::Call
    ));
    let report = analysis::dead(&doc).unwrap();
    assert_not_reported(
        &report,
        &[
            "ufcs::Square::<Shape>::area",
            "ufcs::only_from_area",
            "ufcs::only_from_side",
        ],
    );
}

/// 연관 함수를 값으로 넘긴 경로(`.map(Item::new)`)는 그 메서드의 참조다 — 타입에만 잇던 때는 그렇게만
/// 쓰는 연관 함수가 unreachable이었다. 같은 이름 메서드가 없는 배리언트 경로는 타입 참조로 남는다.
#[test]
fn associated_functions_passed_as_values_are_references() {
    let ws = Workspace::new(
        "assoc-value",
        &[
            ("Cargo.toml", &manifest("assocvalue")),
            (
                "src/main.rs",
                r#"
pub struct Item(i32);
impl Item {
    fn new(n: i32) -> Item { Item(only_from_new(n)) }
    fn unused_assoc() {}
}
fn only_from_new(n: i32) -> i32 { n }
pub enum Mode { A }
fn main() {
    let items: Vec<Item> = (0..3).map(Item::new).collect();
    let _ = (items.len(), Mode::A);
}
"#,
            ),
        ],
    );
    let doc = symbols(ws.path());
    assert!(has_edge(
        &doc,
        "assocvalue::main",
        "assocvalue::Item::new",
        EdgeKind::References
    ));
    let report = analysis::dead(&doc).unwrap();
    assert_not_reported(
        &report,
        &[
            "assocvalue::Item::new",
            "assocvalue::only_from_new",
            "assocvalue::Mode",
        ],
    );
    assert_reported(&report, &["assocvalue::Item::unused_assoc"]);
}

/// 표현식 목록으로 파싱되지 않는 매크로 인자(`json!({"k": f()})` 같은 독자 문법)의 이름은 아이템으로
/// 해석되면 추정 간선이 된다 — 예전에는 그 안에서만 쓰는 함수·상수가 unreachable이었다. 지역 묶음과
/// 같은 이름은 아이템으로 읽지 않는다.
#[test]
fn unparsed_macro_arguments_keep_tentative_references() {
    let ws = Workspace::new(
        "macro-tokens",
        &[
            ("Cargo.toml", &manifest("macrotokens")),
            (
                "src/main.rs",
                r#"
macro_rules! object { ({ $($k:literal : $v:expr),* }) => { vec![$(($k, $v)),*] }; }
const BASE: &str = "root";
fn tool_list() -> &'static str { "tools" }
fn shadowed() -> &'static str { "item" }
fn main() {
    let shadowed = "local";
    let pairs = object!({ "tools": tool_list(), "base": BASE, "s": shadowed });
    let _ = pairs;
}
"#,
            ),
        ],
    );
    let doc = symbols(ws.path());
    let edge = doc
        .edges
        .iter()
        .find(|e| e.from == "macrotokens::main" && e.to == "macrotokens::tool_list")
        .expect("name inside unparsed macro arguments becomes an edge");
    assert!(edge.tentative);
    let report = analysis::dead(&doc).unwrap();
    assert_not_reported(&report, &["macrotokens::tool_list", "macrotokens::BASE"]);
    assert_reported(&report, &["macrotokens::shadowed"]);
}

/// `from`에서 `to`로 가는 간선이 확정(tentative 아님)인가.
fn has_firm_edge(doc: &Document, from: &str, to: &str, kind: EdgeKind) -> bool {
    doc.edges
        .iter()
        .any(|e| e.from == from && e.to == to && e.kind == kind && !e.tentative)
}

/// 워크스페이스 트레이트를 워크스페이스 밖 타입에 구현한 impl 본문의 확정 의존은 impl이 선언된
/// 모듈에서 나간다 — 트레이트 선언 메서드는 그 본문으로 가는 추정 간선만 가져 도달성에만 쓰인다.
/// 선언 메서드가 확정 간선을 가지면 트레이트 모듈에서 impl 모듈로 가는 거짓 의존이 생겨 레이어
/// 규칙 위반과 모듈 순환으로 보였다.
#[test]
fn foreign_impl_body_dependencies_leave_from_the_impl_module() {
    let ws = Workspace::new(
        "foreignowner",
        &[
            ("Cargo.toml", &manifest("foreignowner")),
            (
                "src/main.rs",
                "mod core;\nmod adapters;\nfn main() { println!(\"{}\", core::show(&String::new())); }\n",
            ),
            (
                "src/core.rs",
                "pub trait Render { fn render(&self) -> String; }\npub fn show<R: Render>(r: &R) -> String { r.render() }\n",
            ),
            (
                "src/adapters.rs",
                "use crate::core::Render;\npub fn wrap(s: &str) -> String { format!(\"[{s}]\") }\nimpl Render for String { fn render(&self) -> String { wrap(self) } }\n",
            ),
            (
                ".rustograph.yml",
                "components:\n  core: [\"foreignowner::core/**\"]\n  adapters: [\"foreignowner::adapters/**\"]\ndeps:\n  core: []\n  adapters: [core]\n",
            ),
        ],
    );
    let doc = symbols(ws.path());
    assert!(has_firm_edge(
        &doc,
        "foreignowner::adapters",
        "foreignowner::adapters::wrap",
        EdgeKind::Call
    ));
    assert!(has_edge(
        &doc,
        "foreignowner::core::Render::render",
        "foreignowner::adapters::wrap",
        EdgeKind::Call
    ));
    assert!(!has_firm_edge(
        &doc,
        "foreignowner::core::Render::render",
        "foreignowner::adapters::wrap",
        EdgeKind::Call
    ));
    let cfg = rustograph::config::load(ws.path()).unwrap().unwrap();
    let rules = rustograph::rules::check(&doc, &cfg);
    assert!(
        rules.violations.is_empty(),
        "{:?}",
        rules
            .violations
            .iter()
            .map(|v| format!("{} -> {}", v.from, v.to))
            .collect::<Vec<_>>()
    );
    let modules = doc.view(rustograph::graph::Level::Module).unwrap();
    let module_cycles = analysis::cycles(&modules);
    assert!(
        module_cycles.cycles.is_empty(),
        "{:?}",
        module_cycles
            .cycles
            .iter()
            .map(|c| c.members.clone())
            .collect::<Vec<_>>()
    );
    // 도달성은 그대로다 — 트레이트 메서드를 부르는 곳에서 본문의 호출이 닿는다.
    let report = analysis::dead(&doc).unwrap();
    assert_not_reported(&report, &["foreignowner::adapters::wrap"]);
}

/// 밖 타입 impl 본문이 부르는 함수가 다시 그 트레이트 메서드를 부르면, 확정 호출만으로는 순환이
/// 아니다 — 선언 메서드에서 본문으로 가는 간선이 추정이라 심볼 순환 증거가 못 된다.
#[test]
fn foreign_impl_body_does_not_close_a_symbol_cycle_through_the_trait_method() {
    let ws = Workspace::new(
        "foreignsymcyc",
        &[
            ("Cargo.toml", &manifest("foreignsymcyc")),
            (
                "src/main.rs",
                "pub trait Render { fn render(&self) -> String; }\npub struct Local;\nimpl Render for Local { fn render(&self) -> String { \"l\".into() } }\nimpl Render for String { fn render(&self) -> String { wrap(self) } }\npub fn wrap(s: &str) -> String { Render::render(&Local) + s }\nfn main() { println!(\"{}\", wrap(\"x\")); }\n",
            ),
        ],
    );
    let doc = symbols(ws.path());
    let cycles = analysis::cycles(&doc);
    assert!(
        cycles.cycles.is_empty(),
        "{:?}",
        cycles
            .cycles
            .iter()
            .map(|c| c.members.clone())
            .collect::<Vec<_>>()
    );
}

/// 크레이트 경계를 넘어도 같다 — 트레이트 크레이트가 impl 크레이트에 의존하는 것처럼 보이면
/// Cargo가 금지하는 크레이트 순환이 보고됐다.
#[test]
fn foreign_impl_body_does_not_make_a_crate_cycle() {
    let ws = Workspace::new(
        "foreigncrates",
        &[
            (
                "Cargo.toml",
                "[workspace]\nmembers=['core','adapters','helper']\nresolver='2'\n",
            ),
            (
                "helper/Cargo.toml",
                "[package]\nname='helper'\nversion='0.1.0'\nedition='2021'\n",
            ),
            (
                "helper/src/lib.rs",
                "pub fn wrap(s: &str) -> String { s.to_string() }\n",
            ),
            (
                "core/Cargo.toml",
                "[package]\nname='core_t'\nversion='0.1.0'\nedition='2021'\n",
            ),
            (
                "core/src/lib.rs",
                "pub trait Render { fn render(&self) -> String; }\npub fn show<R: Render>(r: &R) -> String { r.render() }\n",
            ),
            (
                "adapters/Cargo.toml",
                "[package]\nname='adapters'\nversion='0.1.0'\nedition='2021'\n[dependencies]\ncore_t={path='../core'}\nhelper={path='../helper'}\n",
            ),
            (
                "adapters/src/lib.rs",
                "use core_t::Render;\npub fn local_wrap(s: &str) -> String { s.to_string() }\nimpl Render for String {\n    fn render(&self) -> String { helper::wrap(&local_wrap(self)) }\n}\n",
            ),
        ],
    );
    let doc = symbols(ws.path());
    let crates = doc.view(rustograph::graph::Level::Crate).unwrap();
    let cycles = analysis::cycles(&crates);
    assert!(
        cycles.cycles.is_empty(),
        "{:?}",
        cycles
            .cycles
            .iter()
            .map(|c| c.members.clone())
            .collect::<Vec<_>>()
    );
}

/// derive·attribute 매크로는 함수 이름이 아니라 등록한 이름으로 쓰인다 — `#[derive(pm::Hello)]`나
/// `use pm::Hello; #[derive(Hello)]`가 그 크레이트를 쓰는 증거다. proc-macro 크레이트를 수확하면서
/// 이 이름을 잃으면 deps가 실제로 쓰는 의존을 unused로 보고했다.
#[test]
fn derive_names_count_as_uses_of_a_proc_macro_crate() {
    let ws = Workspace::new(
        "pmderive",
        &[
            (
                "Cargo.toml",
                "[workspace]\nmembers=['app','pm','app2','app3']\nresolver='2'\n",
            ),
            (
                "pm/Cargo.toml",
                "[package]\nname='pm-derive'\nversion='0.1.0'\nedition='2021'\n[lib]\nproc-macro=true\n",
            ),
            (
                "pm/src/lib.rs",
                "extern crate proc_macro;\nuse proc_macro::TokenStream;\n#[proc_macro_derive(Hello, attributes(hello))]\npub fn hello_impl(_i: TokenStream) -> TokenStream { helper(); TokenStream::new() }\nfn helper() {}\n#[proc_macro_attribute]\npub fn traced(_a: TokenStream, i: TokenStream) -> TokenStream { i }\n",
            ),
            (
                "app/Cargo.toml",
                "[package]\nname='app'\nversion='0.1.0'\nedition='2021'\n[dependencies]\npm-derive={path='../pm'}\n",
            ),
            ("app/src/lib.rs", "#[derive(pm_derive::Hello)]\npub struct S;\n"),
            (
                "app2/Cargo.toml",
                "[package]\nname='app2'\nversion='0.1.0'\nedition='2021'\n[dependencies]\npm-derive={path='../pm'}\n",
            ),
            (
                "app2/src/lib.rs",
                "use pm_derive::Hello;\n#[derive(Hello)]\npub struct S;\n",
            ),
            (
                "app3/Cargo.toml",
                "[package]\nname='app3'\nversion='0.1.0'\nedition='2021'\n[dependencies]\npm-derive={path='../pm'}\n",
            ),
            (
                "app3/src/lib.rs",
                "#[pm_derive::traced]\npub fn f() {}\n",
            ),
        ],
    );
    let deps = deps::report(ws.path()).unwrap();
    let unused: Vec<String> = deps
        .unused
        .iter()
        .map(|u| format!("{} -> {}", u.package, u.dep))
        .collect();
    assert!(
        !unused
            .iter()
            .any(|u| u.ends_with("-> pm_derive") || u.ends_with("-> pm-derive")),
        "used proc-macro dependency reported unused: {unused:?}"
    );
}

/// 함수 안 `impl`·`trait`의 메서드 매개변수는 그 본문에서 같은 이름의 아이템을 가린다 — 묶지 않으면
/// 매개변수 `log("x")` 호출이 모듈 함수 `log`로 읽혀 거짓 순환이 생겼다.
#[test]
fn local_impl_method_parameters_shadow_module_items() {
    let ws = Workspace::new(
        "localimplparams",
        &[
            ("Cargo.toml", &manifest("localimplparams")),
            (
                "src/main.rs",
                "pub fn log(_: &str) { run() }\npub fn trace(_: &str) { run2() }\npub fn run() {\n    struct L;\n    impl L {\n        fn m(&self, log: &dyn Fn(&str)) { log(\"x\") }\n    }\n    L.m(&|_| {});\n}\npub fn run2() {\n    trait T { fn t(&self, trace: &dyn Fn(&str)) { trace(\"y\") } }\n    struct M;\n    impl T for M {}\n    M.t(&|_| {});\n}\nfn main() { log(\"a\"); trace(\"b\"); }\n",
            ),
        ],
    );
    let doc = symbols(ws.path());
    assert!(!has_edge(
        &doc,
        "localimplparams::run",
        "localimplparams::log",
        EdgeKind::Call
    ));
    assert!(!has_edge(
        &doc,
        "localimplparams::run2",
        "localimplparams::trace",
        EdgeKind::Call
    ));
    let cycles = analysis::cycles(&doc);
    assert!(
        cycles.cycles.is_empty(),
        "{:?}",
        cycles
            .cycles
            .iter()
            .map(|c| c.members.clone())
            .collect::<Vec<_>>()
    );
}
