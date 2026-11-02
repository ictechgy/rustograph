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
