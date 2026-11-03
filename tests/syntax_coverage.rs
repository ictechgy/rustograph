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
