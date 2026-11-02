//! 글롭 임포트가 rustc처럼 동작하는지 실제 수확으로 검증한다 — 들이는 모듈에서 보이는 이름만 들이고,
//! 같은 이름의 명시 임포트(해석되지 않는 std 임포트 포함)에 가려지며, 대상이 바뀌면 다시 만들어지고,
//! 펼친 재노출의 `#[cfg]`를 글롭의 조건과 함께 싣는다. 각 fixture는 rustc로 컴파일되는 코드다.

use rustograph::cfgeval::Facts;
use rustograph::graph::{Document, EdgeKind};
use rustograph::{analysis, source};
use std::path::{Path, PathBuf};

/// 임시 디렉터리에 `files`를 쓴 워크스페이스 — 끝나면 지운다.
struct Workspace(PathBuf);

impl Workspace {
    fn new(tag: &str, files: &[(&str, &str)]) -> Workspace {
        let root = std::env::temp_dir().join(format!(
            "rustograph-glob-imports-{tag}-{}",
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

    fn package(tag: &str, main: &str) -> Workspace {
        let manifest =
            format!("[package]\nname='{tag}'\nversion='0.1.0'\nedition='2021'\n[workspace]\n");
        Workspace::new(tag, &[("Cargo.toml", &manifest), ("src/main.rs", main)])
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn symbols(&self) -> Document {
        source::load(
            &self.0,
            &source::Options {
                symbol_level: true,
                ..Default::default()
            },
        )
        .unwrap()
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn has_edge(doc: &Document, from: &str, to: &str, kind: EdgeKind) -> bool {
    doc.edges
        .iter()
        .any(|e| e.from == from && e.to == to && e.kind == kind)
}

fn any_edge(doc: &Document, from: &str, to: &str) -> bool {
    doc.edges.iter().any(|e| e.from == from && e.to == to)
}

fn dead_ids(doc: &Document) -> Vec<String> {
    analysis::dead(doc)
        .unwrap()
        .findings
        .into_iter()
        .map(|f| f.id)
        .collect()
}

/// `pub(super) use`는 부모 모듈(`top`)과 그 자손에서만 보인다 — 밖의 `other`가 `top::a::*`를 글롭으로
/// 들여도 `g`는 오지 않고, 다른 글롭(`util::*`)의 `g`가 쓰인다. `pub(crate) use`의 `f`는 크레이트
/// 어디서나 보여 글롭으로 온다. (rustc 실행 결과: `util::g`)
#[test]
fn pub_super_reexport_is_not_globbed_outside_its_parent() {
    let ws = Workspace::package(
        "pubsuper",
        r#"
mod util { pub fn g() {} }
mod top {
    pub mod a {
        mod inner { pub fn f() {} pub fn g() {} }
        pub(crate) use self::inner::f;
        pub(super) use self::inner::g;
        pub fn use_g() { g() }
    }
    pub fn in_scope() { a::g() }
}
mod other {
    use crate::util::*;
    use crate::top::a::*;
    pub fn call() { f(); g(); }
}
fn main() { other::call(); top::in_scope(); top::a::use_g(); }
"#,
    );
    let doc = ws.symbols();
    let call = "pubsuper::other::call";
    assert!(has_edge(&doc, call, "pubsuper::util::g", EdgeKind::Call));
    assert!(!has_edge(
        &doc,
        call,
        "pubsuper::top::a::inner::g",
        EdgeKind::Call
    ));
    assert!(has_edge(
        &doc,
        call,
        "pubsuper::top::a::inner::f",
        EdgeKind::Call
    ));
    // 부모 안에서는 `pub(super)` 재노출이 보인다.
    assert!(has_edge(
        &doc,
        "pubsuper::top::in_scope",
        "pubsuper::top::a::inner::g",
        EdgeKind::Call
    ));
    assert!(dead_ids(&doc).is_empty(), "{:?}", dead_ids(&doc));
}

/// 비공개 `use`는 선언한 모듈과 그 자손에서만 보인다 — 형제 `b`의 `use crate::a::*`는 `a`의 비공개
/// `use self::imp::helper`를 들이지 않고 `util::helper`를 쓴다. (rustc 실행 결과: `util::helper`)
#[test]
fn private_import_is_not_globbed_by_a_sibling() {
    let ws = Workspace::package(
        "privglob",
        r#"
mod util { pub fn helper() {} }
mod a {
    mod imp { pub fn helper() {} }
    use self::imp::helper;
    pub fn fa() { helper() }
}
mod b {
    use crate::util::*;
    use crate::a::*;
    pub fn fb() { helper(); fa(); }
}
fn main() { b::fb(); }
"#,
    );
    let doc = ws.symbols();
    assert!(has_edge(
        &doc,
        "privglob::b::fb",
        "privglob::util::helper",
        EdgeKind::Call
    ));
    assert!(!has_edge(
        &doc,
        "privglob::b::fb",
        "privglob::a::imp::helper",
        EdgeKind::Call
    ));
    assert!(has_edge(
        &doc,
        "privglob::a::fa",
        "privglob::a::imp::helper",
        EdgeKind::Call
    ));
    assert!(dead_ids(&doc).is_empty(), "{:?}", dead_ids(&doc));
}

/// 비공개 타입 임포트(`use crate::error::Result`)도 형제의 글롭으로 새지 않는다 — `b`의 `Result`는 std
/// prelude다.
#[test]
fn private_type_import_is_not_globbed_by_a_sibling() {
    let ws = Workspace::package(
        "privres",
        r#"
mod error { pub type Result<T> = std::result::Result<T, String>; }
mod a {
    use crate::error::Result;
    pub fn fa() -> Result<u8> { Ok(1) }
}
mod b {
    use crate::a::*;
    pub fn fb() -> Result<u8, String> { fa() }
}
fn main() { let _ = b::fb(); }
"#,
    );
    let doc = ws.symbols();
    assert!(any_edge(&doc, "privres::a::fa", "privres::error::Result"));
    assert!(!any_edge(&doc, "privres::b::fb", "privres::error::Result"));
    assert!(!any_edge(&doc, "privres::b", "privres::error::Result"));
}

/// 다른 크레이트의 글롭(`use a::*`)은 `pub` 이름만 들인다 — 비공개 모듈 `types`, 비공개 `use
/// types::Result`, `pub(crate) use types::helper`는 오지 않아 `helper`는 `b`의 `util::helper`다.
/// (rustc 실행 결과: `b::util::helper`)
#[test]
fn private_items_and_imports_are_not_globbed_across_crates() {
    let ws = Workspace::new(
        "xws",
        &[
            (
                "Cargo.toml",
                "[workspace]\nmembers = ['a', 'b']\nresolver = '2'\n",
            ),
            (
                "a/Cargo.toml",
                "[package]\nname='a'\nversion='0.1.0'\nedition='2021'\n",
            ),
            (
                "a/src/lib.rs",
                "mod types { pub type Result<T> = std::result::Result<T, String>; pub fn helper() {} }\n\
                 use types::Result;\npub(crate) use types::helper;\n\
                 pub fn api() -> Result<u8> { helper(); Ok(1) }\n",
            ),
            (
                "b/Cargo.toml",
                "[package]\nname='b'\nversion='0.1.0'\nedition='2021'\n\
                 [dependencies]\na = { path = '../a' }\n",
            ),
            (
                "b/src/main.rs",
                "mod util { pub fn helper() {} }\nuse util::*;\nuse a::*;\n\
                 fn run() -> Result<u8, String> { helper(); api() }\n\
                 fn main() { let _ = run(); }\n",
            ),
        ],
    );
    let doc = ws.symbols();
    assert!(has_edge(&doc, "b::run", "b::util::helper", EdgeKind::Call));
    assert!(has_edge(&doc, "b::run", "a::api", EdgeKind::Call));
    assert!(!any_edge(&doc, "b::run", "a::types::helper"));
    assert!(!any_edge(&doc, "b::run", "a::types::Result"));
    assert!(!any_edge(&doc, "b", "a::types"));
}

/// 계층 규칙이 비공개 임포트를 글롭 너머로 보지 않는다 — `model`의 비공개 `use crate::db::Conn`은 `ui`의
/// `use crate::model::*`로 오지 않으므로 `ui → db` 위반이 없고 `rules --strict`는 0이다.
#[test]
fn layer_rules_do_not_see_private_imports_through_globs() {
    let ws = Workspace::new(
        "layers",
        &[
            (
                "Cargo.toml",
                "[package]\nname='layers'\nversion='0.1.0'\nedition='2021'\n[workspace]\n",
            ),
            (
                "rules.yml",
                "components:\n  db: [\"layers::db/**\"]\n  model: [\"layers::model/**\"]\n  \
                 ui: [\"layers::ui/**\"]\ndeps:\n  ui: [model]\n  model: [db]\n  db: []\n",
            ),
            (
                "src/main.rs",
                "mod db { pub struct Conn; impl Conn { pub fn open() -> Conn { Conn } } }\n\
                 mod model {\n    use crate::db::Conn;\n    pub struct User;\n    \
                 pub fn load() -> User { let _c = Conn::open(); User }\n}\n\
                 mod ui {\n    use crate::model::*;\n    pub fn view() { let _u: User = load(); }\n}\n\
                 fn main() { ui::view(); }\n",
            ),
        ],
    );
    let doc = ws.symbols();
    assert!(!any_edge(&doc, "layers::ui", "layers::db::Conn"));
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_rustograph"))
        .args(["rules", "--strict", "--config"])
        .arg(ws.path().join("rules.yml"))
        .arg("--dir")
        .arg(ws.path())
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// 명시 임포트는 해석되지 않는 std 경로여도(`use std::error::Error`) 같은 이름의 글롭 임포트(부모의
/// `use crate::error::Error`를 들인 `use super::*`)를 가린다 — `child::g`의 `Error`는 std 트레이트다.
#[test]
fn unresolved_explicit_import_shadows_a_glob() {
    let ws = Workspace::package(
        "extshadow",
        r#"
mod error {
    #[derive(Debug)]
    pub struct Error;
    impl std::fmt::Display for Error { fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "e") } }
    impl std::error::Error for Error {}
}
mod m {
    use crate::error::Error;
    pub fn f() -> Result<(), Error> { Ok(()) }
    pub mod child {
        use super::*;
        use std::error::Error;
        pub fn g() -> Result<(), Box<dyn Error>> { f().map_err(|e| Box::new(e) as Box<dyn Error>) }
    }
}
fn main() { let _ = m::child::g(); }
"#,
    );
    let doc = ws.symbols();
    assert!(any_edge(&doc, "extshadow::m::f", "extshadow::error::Error"));
    assert!(has_edge(
        &doc,
        "extshadow::m::child::g",
        "extshadow::m::f",
        EdgeKind::Call
    ));
    assert!(!any_edge(
        &doc,
        "extshadow::m::child::g",
        "extshadow::error::Error"
    ));
}

/// 해석되지 않는 명시 임포트는 그 이름의 해석을 멈춘다 — 크레이트 루트에 같은 이름(`Error`)이 있어도
/// 관대한 루트 해석으로 넘어가지 않고, 그 이름을 글롭으로 받은 자식(`use super::*`)도 마찬가지다.
#[test]
fn unresolved_explicit_import_blocks_the_crate_root_fallback() {
    let ws = Workspace::package(
        "rootfallback",
        r#"
pub struct Error;
mod m {
    use std::fmt::Error;
    pub fn f() -> Error { Error }
    pub mod child {
        use super::*;
        pub fn g() -> Error { f() }
    }
}
fn main() { let _ = m::child::g(); let _ = Error; }
"#,
    );
    let doc = ws.symbols();
    assert!(!any_edge(&doc, "rootfallback::m::f", "rootfallback::Error"));
    assert!(!any_edge(
        &doc,
        "rootfallback::m::child::g",
        "rootfallback::Error"
    ));
    assert!(has_edge(
        &doc,
        "rootfallback::m::child::g",
        "rootfallback::m::f",
        EdgeKind::Call
    ));
}

/// 글롭 대상이 바퀴 사이에 바뀌면 그 글롭이 펼친 이름도 다시 만든다 — `k::y`는 처음에 `p::*`의 `y`였다가
/// 명시 재노출 `pub use crate::q::y`(→ `r::y`)가 풀리면 `r::y`다. 남은 `p::y::only_in_p`는 `m`의 임포트가
/// 아니다. (rustc 실행 결과: `r::y::shared`)
#[test]
fn glob_tables_are_rebuilt_when_their_target_changes() {
    let ws = Workspace::package(
        "stale",
        r#"
mod p { pub mod y { pub fn only_in_p() {} pub fn shared() {} } }
mod r { pub mod y { pub fn shared() {} } }
mod q { pub use crate::r::y; }
mod k {
    #[allow(unused_imports)]
    use crate::p::*;
    pub use crate::q::y;
}
mod m {
    use crate::k::y::*;
    pub fn call() { shared() }
}
fn main() { m::call(); crate::p::y::only_in_p(); }
"#,
    );
    let doc = ws.symbols();
    assert!(has_edge(
        &doc,
        "stale::m::call",
        "stale::r::y::shared",
        EdgeKind::Call
    ));
    assert!(!has_edge(
        &doc,
        "stale::m::call",
        "stale::p::y::shared",
        EdgeKind::Call
    ));
    assert!(!any_edge(&doc, "stale::m", "stale::p::y::only_in_p"));
}

/// 글롭이 펼친 재노출은 글롭의 `#[cfg]`와 재노출의 `#[cfg]`를 함께 싣는다 — `#[cfg(unix)] use a::*`가 들인
/// `#[cfg(windows)] pub use Handle`은 어느 빌드에도 없으므로 `--target` 리눅스에서 지워지고,
/// `#[cfg(feature = "x")] use c2::*`가 들인 `#[cfg(test)] pub use Fake`는 `--exclude-tests`가 지운다.
#[test]
fn glob_expanded_reexports_carry_both_cfg_conditions() {
    let ws = Workspace::package(
        "cfgx",
        r#"
mod win { pub struct Handle; }
mod a {
    #[cfg(windows)]
    pub use crate::win::Handle;
    pub fn fa() {}
}
mod b {
    #[cfg(unix)]
    #[allow(unused_imports)]
    use crate::a::*;
    pub fn fb() {}
}
mod t { pub struct Fake; }
mod c2 {
    #[cfg(test)]
    pub use crate::t::Fake;
}
mod d {
    #[cfg(feature = "x")]
    #[allow(unused_imports)]
    use crate::c2::*;
}
fn main() { a::fa(); b::fb(); }
"#,
    );
    let doc = ws.symbols();
    let cfg_of = |from: &str, to: &str| {
        doc.edges
            .iter()
            .find(|e| e.from == from && e.to == to && e.kind == EdgeKind::Uses)
            .map(|e| e.cfg.clone())
    };
    assert_eq!(
        cfg_of("cfgx::b", "cfgx::win::Handle"),
        Some(Some("all(unix , windows)".to_string()))
    );
    assert_eq!(
        cfg_of("cfgx::d", "cfgx::t::Fake"),
        Some(Some("all(feature = \"x\" , test)".to_string()))
    );
    let linux = "x86_64-unknown-linux-gnu";
    let on_linux = doc.for_target(linux, &Facts::from_triple(linux));
    assert!(!any_edge(&on_linux, "cfgx::b", "cfgx::win::Handle"));
    assert!(!any_edge(&doc.without_tests(), "cfgx::d", "cfgx::t::Fake"));
}

/// 2015 판의 `pub(in top)`는 크레이트 기준 경로다 — `top::a`의 `pub(in top) use self::inner::g`는 `top`과
/// 그 자손에서만 보이므로 `other`의 `use top::a::*`로 오지 않고 `util::g`가 쓰인다. 경로를 해석하지 못한
/// 제한 가시성도 크레이트 밖으로는 새지 않는다. (rustc 실행 결과: `util::g`, `inner::g`)
#[test]
fn edition_2015_pub_in_path_is_crate_relative() {
    let ws = Workspace::new(
        "ed2015",
        &[
            (
                "Cargo.toml",
                "[package]\nname='ed2015'\nversion='0.1.0'\nedition='2015'\n[workspace]\n",
            ),
            (
                "src/main.rs",
                "mod util { pub fn g() {} }\n\
                 mod top {\n    pub mod a {\n        mod inner { pub fn g() {} }\n        \
                 pub(in top) use self::inner::g;\n    }\n    pub fn in_top() { a::g() }\n}\n\
                 mod other {\n    use util::*;\n    use top::a::*;\n    pub fn call() { g() }\n}\n\
                 fn main() { other::call(); top::in_top(); }\n",
            ),
        ],
    );
    let doc = ws.symbols();
    assert!(has_edge(
        &doc,
        "ed2015::other::call",
        "ed2015::util::g",
        EdgeKind::Call
    ));
    assert!(!has_edge(
        &doc,
        "ed2015::other::call",
        "ed2015::top::a::inner::g",
        EdgeKind::Call
    ));
    assert!(has_edge(
        &doc,
        "ed2015::top::in_top",
        "ed2015::top::a::inner::g",
        EdgeKind::Call
    ));
}

/// `pub(in crate::top)` 재노출은 `top` 안(`top::sib`)의 글롭에는 오고 밖(`other`)의 글롭에는 오지 않는다.
/// (rustc 실행 결과: `util::g`, `inner::g`, `inner::g`)
#[test]
fn pub_in_path_reexport_is_globbed_only_inside_that_module() {
    let ws = Workspace::package(
        "pubin",
        r#"
mod util { pub fn g() {} }
mod top {
    pub mod mid {
        pub mod a {
            mod inner { pub fn g() {} }
            pub(in crate::top) use self::inner::g;
        }
        pub fn in_mid() { a::g() }
    }
    pub mod sib {
        use super::mid::a::*;
        pub fn s() { g() }
    }
}
mod other {
    use crate::util::*;
    use crate::top::mid::a::*;
    pub fn call() { g() }
}
fn main() { other::call(); top::sib::s(); top::mid::in_mid(); }
"#,
    );
    let doc = ws.symbols();
    let inner_g = "pubin::top::mid::a::inner::g";
    assert!(has_edge(
        &doc,
        "pubin::other::call",
        "pubin::util::g",
        EdgeKind::Call
    ));
    assert!(!has_edge(
        &doc,
        "pubin::other::call",
        inner_g,
        EdgeKind::Call
    ));
    assert!(has_edge(
        &doc,
        "pubin::top::sib::s",
        inner_g,
        EdgeKind::Call
    ));
}

/// `use m::*`가 `m`의 마지막 이름에 `m`을 묶는 항목은 Rust 이름이 아니라 그 글롭의 의존 증거다 — 그
/// 모듈 밖으로 전해지지 않는다. `b`의 `pub use super::*`(→ `a`)를 글롭으로 들인 `other`, `p`의
/// `pub use crate::q::*`(→ `q`)를 글롭으로 들인 `importer`는 그 모듈(`a`·`q`)을 들이지 않는다.
/// 재노출된 아이템(`q::x`)으로 가는 임포트는 그대로다. (rustc 실행 결과: `util::g`, `a::inner::g` / `q::x`)
#[test]
fn glob_module_name_entry_does_not_propagate() {
    let ws = Workspace::package(
        "chainnarrow",
        r#"
mod util { pub fn g() {} }
mod a {
    mod inner { pub fn g() {} }
    #[allow(unused_imports)]
    use self::inner::g;
    pub mod b {
        #[allow(unused_imports)]
        pub use super::*;
        pub fn in_a() { g() }
    }
}
mod other {
    use crate::util::*;
    use crate::a::b::*;
    pub fn call() { g(); in_a(); }
}
fn main() { other::call(); }
"#,
    );
    let doc = ws.symbols();
    assert!(!any_edge(&doc, "chainnarrow::other", "chainnarrow::a"));
    assert!(has_edge(
        &doc,
        "chainnarrow::other::call",
        "chainnarrow::util::g",
        EdgeKind::Call
    ));
    assert!(has_edge(
        &doc,
        "chainnarrow::a::b::in_a",
        "chainnarrow::a::inner::g",
        EdgeKind::Call
    ));
    // 글롭한 모듈 자신(`b`)으로 가는 의존 증거는 남는다.
    assert!(any_edge(&doc, "chainnarrow::other", "chainnarrow::a::b"));

    let ws = Workspace::package(
        "pubglob",
        r#"
mod q { pub fn x() {} }
mod p {
    pub use crate::q::*;
    pub fn inside() { x() }
}
mod importer {
    use crate::p::*;
    pub fn call() { x(); inside(); }
}
fn main() { importer::call(); }
"#,
    );
    let doc = ws.symbols();
    assert!(!any_edge(&doc, "pubglob::importer", "pubglob::q"));
    assert!(has_edge(
        &doc,
        "pubglob::importer",
        "pubglob::q::x",
        EdgeKind::Uses
    ));
    assert!(has_edge(
        &doc,
        "pubglob::importer::call",
        "pubglob::q::x",
        EdgeKind::Call
    ));
    assert!(any_edge(&doc, "pubglob::p", "pubglob::q"));
}

/// 비공개 글롭이 들인 재노출은 그 모듈 밖에서 보이지 않는다 — `b`의 `use crate::a::*`가 들인 `a`의
/// `pub use crate::inner::g`는 `b`를 글롭하는 `other`에게 오지 않는다. (rustc 실행 결과: `util::g`, `inner::g`)
#[test]
fn private_glob_narrows_the_reexports_it_brings_in() {
    let ws = Workspace::package(
        "privglobreexp",
        r#"
mod util { pub fn g() {} }
mod inner { pub fn g() {} }
mod a { pub use crate::inner::g; }
mod b {
    #[allow(unused_imports)]
    use crate::a::*;
    pub fn bf() { g() }
}
mod other {
    use crate::util::*;
    use crate::b::*;
    pub fn call() { g(); bf(); }
}
fn main() { other::call(); }
"#,
    );
    let doc = ws.symbols();
    let call = "privglobreexp::other::call";
    assert!(has_edge(
        &doc,
        call,
        "privglobreexp::util::g",
        EdgeKind::Call
    ));
    assert!(!has_edge(
        &doc,
        call,
        "privglobreexp::inner::g",
        EdgeKind::Call
    ));
    assert!(has_edge(
        &doc,
        "privglobreexp::b::bf",
        "privglobreexp::inner::g",
        EdgeKind::Call
    ));
}

/// 비공개 글롭이 들인 공개 아이템도 그 모듈 밖에서 보이지 않는다 — `b`의 `use crate::a::*`가 들인 `a::g`는
/// `other`에게 오지 않는다. (rustc 실행 결과: `util::g`, `a::g`)
#[test]
fn private_glob_narrows_the_items_it_brings_in() {
    let ws = Workspace::package(
        "privglobchain",
        r#"
mod util { pub fn g() {} }
mod a { pub fn g() {} }
mod b {
    #[allow(unused_imports)]
    use crate::a::*;
    pub fn bf() { g() }
}
mod other {
    use crate::util::*;
    use crate::b::*;
    pub fn call() { g(); bf(); }
}
fn main() { other::call(); }
"#,
    );
    let doc = ws.symbols();
    let call = "privglobchain::other::call";
    assert!(has_edge(
        &doc,
        call,
        "privglobchain::util::g",
        EdgeKind::Call
    ));
    assert!(!has_edge(&doc, call, "privglobchain::a::g", EdgeKind::Call));
    assert!(has_edge(
        &doc,
        "privglobchain::b::bf",
        "privglobchain::a::g",
        EdgeKind::Call
    ));
}

/// 같은 대상을 들이는 두 글롭의 가시성은 넓은 쪽이다 — `k`의 `pub use crate::p::*`와 비공개
/// `use crate::q::*`(→ 같은 `p::x`)가 겹쳐도 `x`는 공개라 `other`의 `use crate::k::*`로 온다.
/// (rustc 실행 결과: `p::x`)
#[test]
fn same_target_globs_merge_to_the_wider_scope() {
    let ws = Workspace::package(
        "widerscope",
        r#"
mod p { pub fn x() {} }
mod q { pub use crate::p::x; }
mod k {
    pub use crate::p::*;
    #[allow(unused_imports)]
    use crate::q::*;
}
mod other {
    use crate::k::*;
    pub fn call() { x() }
}
fn main() { other::call(); }
"#,
    );
    let doc = ws.symbols();
    assert!(has_edge(
        &doc,
        "widerscope::other::call",
        "widerscope::p::x",
        EdgeKind::Call
    ));
}

/// 같은 대상을 들이는 두 글롭의 조건은 약한 쪽이다 — 어느 글롭으로든 들어오면 임포트가 있다. 비교할 수
/// 없는 `unix`와 `windows`는 조건 없음(어느 `--target`에서도 남는다), 부분 집합인 `unix`와
/// `all(unix , feature = "fb")`는 `unix`, 조건 없는 글롭이 하나라도 있으면 조건 없음이다.
#[test]
fn same_target_globs_merge_to_the_weaker_cfg() {
    let ws = Workspace::new(
        "cfgmerge",
        &[
            (
                "Cargo.toml",
                "[package]\nname='cfgmerge'\nversion='0.1.0'\nedition='2021'\n\
                 [features]\nfa=[]\nfb=[]\n[workspace]\n",
            ),
            (
                "src/main.rs",
                r#"
mod p { pub fn x() {} pub fn y() {} }
mod q { pub use crate::p::x; pub use crate::p::y; }
mod incomparable {
    #[cfg(unix)]
    #[allow(unused_imports)]
    use crate::p::*;
    #[cfg(windows)]
    #[allow(unused_imports)]
    use crate::q::*;
}
mod comparable {
    #[cfg(unix)]
    #[allow(unused_imports)]
    use crate::p::*;
    #[cfg(all(unix, feature = "fb"))]
    #[allow(unused_imports)]
    use crate::q::*;
}
mod one_unconditional {
    #[allow(unused_imports)]
    use crate::p::*;
    #[cfg(feature = "fa")]
    #[allow(unused_imports)]
    use crate::q::*;
}
fn main() { p::x(); p::y(); }
"#,
            ),
        ],
    );
    let doc = ws.symbols();
    let cfg_of = |from: &str| {
        doc.edges
            .iter()
            .find(|e| e.from == from && e.to == "cfgmerge::p::x" && e.kind == EdgeKind::Uses)
            .map(|e| e.cfg.clone())
    };
    assert_eq!(cfg_of("cfgmerge::incomparable"), Some(None));
    assert_eq!(
        cfg_of("cfgmerge::comparable"),
        Some(Some("unix".to_string()))
    );
    assert_eq!(cfg_of("cfgmerge::one_unconditional"), Some(None));
    let wasm = "wasm32-unknown-unknown";
    let on_wasm = doc.for_target(wasm, &Facts::from_triple(wasm));
    assert!(has_edge(
        &on_wasm,
        "cfgmerge::incomparable",
        "cfgmerge::p::x",
        EdgeKind::Uses
    ));
    assert!(!any_edge(
        &on_wasm,
        "cfgmerge::comparable",
        "cfgmerge::p::x"
    ));
}

/// 해석되지 않는 임포트의 표지는 재노출 글롭과 자식의 글롭을 거쳐 전해진다 — 손자의 `Error`는 `m`의
/// `use std::fmt::Error`이고 크레이트 루트의 `Error`가 아니다.
#[test]
fn unresolved_import_marker_crosses_reexport_and_child_globs() {
    let ws = Workspace::package(
        "markerhops",
        r#"
#[allow(dead_code)]
pub struct Error;
mod m {
    #[allow(unused_imports)]
    use std::fmt::Error;
    pub mod child {
        #[allow(unused_imports)]
        pub use super::*;
        pub mod grandchild {
            use super::*;
            pub fn g() -> Result<(), Error> { Err(Error) }
        }
    }
}
fn main() { let _ = m::child::grandchild::g(); }
"#,
    );
    let doc = ws.symbols();
    assert!(!any_edge(
        &doc,
        "markerhops::m::child::grandchild::g",
        "markerhops::Error"
    ));
}

/// 비공개 임포트는 자손의 글롭 사슬로 이어진다 — `a`의 비공개 `use crate::x::Y`가 `b`·`c`의
/// `use super::*`를 거쳐 `c::f`의 `Y`가 된다. 같은 이름의 비공개 `fn x`는 공개 재노출 `x`(모듈)를 글롭
/// 밖에서 가리지 않는다. (rustc 실행 결과: `m::x::f`)
#[test]
fn private_imports_follow_descendant_globs_and_private_items_do_not_mask_reexports() {
    let ws = Workspace::package(
        "descchain",
        r#"
mod x { pub struct Y; }
mod a {
    #[allow(unused_imports)]
    use crate::x::Y;
    pub mod b {
        #[allow(unused_imports)]
        use super::*;
        pub mod c {
            use super::*;
            pub fn f() -> Y { Y }
        }
    }
}
fn main() { let _ = a::b::c::f(); }
"#,
    );
    let doc = ws.symbols();
    assert!(any_edge(&doc, "descchain::a::b::c::f", "descchain::x::Y"));

    let ws = Workspace::package(
        "nsfilter",
        r#"
mod m { pub mod x { pub fn f() {} } }
mod t {
    #[allow(dead_code)]
    fn x() {}
    pub use crate::m::x;
}
mod importer {
    use crate::t::*;
    pub fn call() { x::f(); }
}
fn main() { importer::call(); }
"#,
    );
    let doc = ws.symbols();
    assert!(has_edge(
        &doc,
        "nsfilter::importer::call",
        "nsfilter::m::x::f",
        EdgeKind::Call
    ));
}

/// 글롭한 모듈 자신(`use crate::inner::extdep::*`의 `extdep`)은 이름이 아니다 — 그 모듈과 자손(`use super::*`)에서
/// 이름 해석에 끼지 않아, `child`의 `extdep::Thing`은 같은 이름의 외부 크레이트다. 글롭한 모듈로 가는 uses
/// 증거와 외부 크레이트 글롭(`pub use extdep::*`)의 의존 증거는 남는다. (rustc 실행 결과: 외부 크레이트의
/// `Thing::new`)
#[test]
fn glob_module_name_is_not_a_binding_but_stays_dependency_evidence() {
    let ws = Workspace::new(
        "fakedesc",
        &[
            (
                "extdep/Cargo.toml",
                "[package]\nname='extdep'\nversion='0.1.0'\nedition='2021'\n",
            ),
            (
                "extdep/src/lib.rs",
                "pub struct Thing; pub struct Other; impl Thing { pub fn new() -> Thing { Thing } }\n",
            ),
            (
                "app/Cargo.toml",
                "[package]\nname='fakedesc'\nversion='0.1.0'\nedition='2021'\n\
                 [dependencies]\nextdep = { path = '../extdep' }\n[workspace]\n",
            ),
            (
                "app/src/main.rs",
                r#"
mod inner {
    pub mod extdep {
        pub struct Thing;
        impl Thing { pub fn new() -> Thing { Thing } }
    }
}
mod m {
    #[allow(unused_imports)]
    use crate::inner::extdep::*;
    pub mod child {
        #[allow(unused_imports)]
        use super::*;
        pub fn f() -> extdep::Thing { extdep::Thing::new() }
    }
}
fn main() { let _ = m::child::f(); }
"#,
            ),
        ],
    );
    let app = ws.path().join("app");
    for include_deps in [false, true] {
        let doc = source::load(
            &app,
            &source::Options {
                symbol_level: true,
                include_deps,
                ..Default::default()
            },
        )
        .unwrap();
        let f = "fakedesc::m::child::f";
        assert!(!any_edge(&doc, f, "fakedesc::inner::extdep::Thing::new"));
        assert!(!any_edge(&doc, f, "fakedesc::inner::extdep::Thing"));
        assert!(has_edge(
            &doc,
            "fakedesc::m",
            "fakedesc::inner::extdep",
            EdgeKind::Uses
        ));
        assert!(!any_edge(
            &doc,
            "fakedesc::m::child",
            "fakedesc::inner::extdep"
        ));
        if include_deps {
            assert!(has_edge(&doc, f, "extdep", EdgeKind::References));
        }
    }

    // 외부 크레이트를 글롭으로 재노출해도(`pub use extdep::*`) 그 의존 증거는 남는다 — deps가 미사용으로 보지 않는다.
    let ws = Workspace::new(
        "extws2",
        &[
            (
                "extdep/Cargo.toml",
                "[package]\nname='extdep'\nversion='0.1.0'\nedition='2021'\n",
            ),
            (
                "extdep/src/lib.rs",
                "pub struct Thing; impl Thing { pub fn new() -> Thing { Thing } }\n",
            ),
            (
                "ws/Cargo.toml",
                "[workspace]\nmembers = ['a']\nresolver = '2'\n",
            ),
            (
                "ws/a/Cargo.toml",
                "[package]\nname='a'\nversion='0.1.0'\nedition='2021'\n\
                 [dependencies]\nextdep = { path = '../../extdep' }\n",
            ),
            (
                "ws/a/src/main.rs",
                "mod m {\n    pub use extdep::*;\n}\nmod user {\n    use crate::m::*;\n    \
                 pub fn f() -> Thing { Thing::new() }\n}\nfn main() { let _ = user::f(); }\n",
            ),
        ],
    );
    let dir = ws.path().join("ws");
    let doc = source::load(
        &dir,
        &source::Options {
            symbol_level: true,
            include_deps: true,
            ..Default::default()
        },
    )
    .unwrap();
    assert!(has_edge(&doc, "a::m", "extdep", EdgeKind::Uses));
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_rustograph"))
        .args(["deps", "--format", "json", "--dir"])
        .arg(&dir)
        .output()
        .unwrap();
    let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(report["unused"], serde_json::json!([]), "{report}");
}
