//! `#[cfg]`로 갈린 대안 임포트와 임포트 조건의 전파를 실제 수확으로 검증한다.
//!
//! rustc는 한 빌드의 한 이름공간에 같은 이름을 들이는 서로 다른 `use` 둘을 두지 않는다(명시 `use`는 E0252, 글롭
//! 이름은 쓰면 E0659). 그런 둘이 보이면 `#[cfg]`로 갈린 서로 다른 빌드의 것이다 — 모든 빌드의 합집합을 보는
//! 그래프는 둘 다 남기고, 그 이름으로 해석한 간선은 거쳐 온 임포트의 조건을 싣는다. 각 fixture는 rustc로
//! 컴파일되는 코드다(`cargo check`, `cargo check --tests`).

use rustograph::cfgeval::Facts;
use rustograph::graph::{Document, EdgeKind};
use rustograph::{analysis, deps, source};

const LINUX: &str = "x86_64-unknown-linux-gnu";
const WINDOWS: &str = "x86_64-pc-windows-msvc";

/// 임시 디렉터리의 단일 패키지 워크스페이스(`[features]`는 `features`)를 심볼 레벨로 수확하고 지운다.
fn harvest(tag: &str, features: &str, main: &str) -> Document {
    let manifest = format!(
        "[package]\nname='{tag}'\nversion='0.1.0'\nedition='2021'\n[features]\n{features}\n[workspace]\n"
    );
    harvest_files(tag, &[("Cargo.toml", &manifest), ("src/main.rs", main)])
}

/// 임시 디렉터리에 `files`(상대 경로, 내용)를 쓴 워크스페이스를 심볼 레벨로 수확하고 지운다.
fn harvest_files(tag: &str, files: &[(&str, &str)]) -> Document {
    load_files(tag, files, |root| {
        source::load(
            root,
            &source::Options {
                symbol_level: true,
                ..Default::default()
            },
        )
        .unwrap()
    })
}

/// 임시 디렉터리에 `files`를 쓰고 `run`을 그 경로로 부른 뒤 지운다.
fn load_files<T>(tag: &str, files: &[(&str, &str)], run: impl FnOnce(&std::path::Path) -> T) -> T {
    let root = std::env::temp_dir().join(format!(
        "rustograph-cfg-alternatives-{tag}-{}",
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
    let out = run(&root);
    std::fs::remove_dir_all(&root).unwrap();
    out
}

/// `from → to` `kind` 간선의 조건 — 간선이 없으면 `None`, 조건 없는 간선이면 `Some(None)`이다.
fn edge_cfg(doc: &Document, from: &str, to: &str, kind: EdgeKind) -> Option<Option<String>> {
    doc.edges
        .iter()
        .find(|e| e.from == from && e.to == to && e.kind == kind && !e.tentative)
        .map(|e| e.cfg.clone())
}

/// `cfg`라는 조건을 단 간선 — [`edge_cfg`]의 기대값을 짧게 쓴다.
fn under(cfg: &str) -> Option<Option<String>> {
    Some(Some(cfg.to_string()))
}

/// 문서의 unreachable 정점 ID.
fn dead_ids(doc: &Document) -> Vec<String> {
    analysis::dead(doc)
        .unwrap()
        .findings
        .into_iter()
        .map(|f| f.id)
        .collect()
}

/// 트리플 `triple`의 cfg 팩트로 거른 문서 — `--target`과 같다.
fn for_target(doc: &Document, triple: &str) -> Document {
    doc.for_target(triple, &Facts::from_triple(triple))
}

/// 플랫폼마다 갈린 두 명시 `use`는 둘 다 남는다 — `Sys::open()`은 빌드마다 다른 정점을 부르고 간선은 그 `use`의
/// 조건을 싣는다. 예전에는 소스 순서상 뒤의 `use`만 남아, 리눅스에서 실제로 도는 `unix_impl::Sys::open`이
/// unreachable로 나왔다(`--target` 리눅스에서도). 이제 `--target` 리눅스에서는 rustc의 dead_code 경고와 같이
/// windows 쪽만 남는다.
#[test]
fn cfg_split_explicit_imports_keep_both_alternatives() {
    let doc = harvest(
        "splituse",
        "",
        r#"
mod unix_impl {
    pub struct Sys;
    impl Sys {
        pub fn open() -> Sys {
            Sys
        }
    }
}
mod win_impl {
    pub struct Sys;
    impl Sys {
        pub fn open() -> Sys {
            Sys
        }
    }
}
#[cfg(unix)]
use crate::unix_impl::Sys;
#[cfg(windows)]
use crate::win_impl::Sys;
fn main() {
    let _ = Sys::open();
}
"#,
    );
    let main = "splituse::main";
    assert_eq!(
        edge_cfg(&doc, main, "splituse::unix_impl::Sys::open", EdgeKind::Call),
        under("unix")
    );
    assert_eq!(
        edge_cfg(&doc, main, "splituse::win_impl::Sys::open", EdgeKind::Call),
        under("windows")
    );
    let dead = dead_ids(&doc);
    assert!(!dead.iter().any(|id| id.ends_with("Sys::open")), "{dead:?}");
    let on_linux = dead_ids(&for_target(&doc, LINUX));
    assert!(on_linux.contains(&"splituse::win_impl::Sys::open".to_string()));
    assert!(!on_linux.contains(&"splituse::unix_impl::Sys::open".to_string()));
    let on_windows = dead_ids(&for_target(&doc, WINDOWS));
    assert!(on_windows.contains(&"splituse::unix_impl::Sys::open".to_string()));
    assert!(!on_windows.contains(&"splituse::win_impl::Sys::open".to_string()));
}

/// 플랫폼 모듈을 글롭으로 다시 내보내는 흔한 모양(`#[cfg(unix)] pub use self::unix::*;`와 windows 쪽)도 둘 다
/// 남는다. 예전에는 뒤의 글롭이 이겨 `sys::page_size()`가 windows 함수에만 닿았다.
#[test]
fn cfg_split_glob_reexports_keep_both_platforms() {
    let doc = harvest(
        "splitglob",
        "",
        r#"
mod sys {
    #[cfg(unix)]
    mod unix {
        pub(crate) fn page_size() -> usize {
            4096
        }
    }
    #[cfg(windows)]
    mod windows {
        pub(crate) fn page_size() -> usize {
            65536
        }
    }
    #[cfg(unix)]
    pub(crate) use self::unix::*;
    #[cfg(windows)]
    pub(crate) use self::windows::*;
}
fn main() {
    sys::page_size();
}
"#,
    );
    let main = "splitglob::main";
    assert_eq!(
        edge_cfg(
            &doc,
            main,
            "splitglob::sys::unix::page_size",
            EdgeKind::Call
        ),
        under("unix")
    );
    assert_eq!(
        edge_cfg(
            &doc,
            main,
            "splitglob::sys::windows::page_size",
            EdgeKind::Call
        ),
        under("windows")
    );
    assert_eq!(
        edge_cfg(
            &doc,
            "splitglob::sys",
            "splitglob::sys::unix::page_size",
            EdgeKind::Uses
        ),
        under("unix")
    );
    assert!(dead_ids(&doc).is_empty(), "{:?}", dead_ids(&doc));
}

/// 경로 접두사가 `#[cfg]`로 갈린 모듈 별칭(`use unix_sys as platform;`과 `use win_sys as platform;` — rustix의
/// `ioctl` 모양)이면 별칭마다 나머지 경로를 걷는다.
#[test]
fn cfg_split_module_aliases_resolve_through_each_alternative() {
    let doc = harvest(
        "splitalias",
        "",
        r#"
#[cfg(unix)]
mod unix_sys {
    pub fn compose() -> u32 {
        1
    }
}
#[cfg(windows)]
mod win_sys {
    pub fn compose() -> u32 {
        2
    }
}
#[cfg(unix)]
use unix_sys as platform;
#[cfg(windows)]
use win_sys as platform;
fn main() {
    platform::compose();
}
"#,
    );
    let main = "splitalias::main";
    assert_eq!(
        edge_cfg(&doc, main, "splitalias::unix_sys::compose", EdgeKind::Call),
        under("unix")
    );
    assert_eq!(
        edge_cfg(&doc, main, "splitalias::win_sys::compose", EdgeKind::Call),
        under("windows")
    );
    assert!(dead_ids(&doc).is_empty(), "{:?}", dead_ids(&doc));
}

/// 본문·시그니처 간선은 그 이름을 들인 `use`의 조건을 물려받는다 — 직접 들인 이름이든, 조건부 재노출을 거친
/// 경로(`crate::sync::Condvar::new()`)든 같다. std 쪽 대안(`#[cfg(not(..))] use std::sync::Condvar`)은 그래프 밖이라
/// 간선이 없다. 다른 모듈이 그 재노출을 `use`하면 그 `uses` 간선도 재노출의 조건을 싣는다. 예전에는 `uses` 간선만
/// 조건을 싣고 본문 간선은 조건 없는 확정 간선이었다.
#[test]
fn body_edges_inherit_the_import_cfg() {
    let doc = harvest(
        "inherit",
        "pl = []",
        r#"
mod pl {
    pub struct Condvar;
    impl Condvar {
        pub fn new() -> Condvar {
            Condvar
        }
    }
}
mod sync {
    #[cfg(feature = "pl")]
    pub use crate::pl::Condvar;
    #[cfg(not(feature = "pl"))]
    pub use std::sync::Condvar;
}
mod user {
    use crate::sync::Condvar;
    pub fn make() -> Condvar {
        Condvar::new()
    }
}
#[cfg(feature = "pl")]
use crate::pl::Condvar;
#[cfg(not(feature = "pl"))]
use std::sync::Condvar;
fn make() -> Condvar {
    Condvar::new()
}
#[cfg(feature = "pl")]
fn make_gated() -> Condvar {
    Condvar::new()
}
fn main() {
    make();
    user::make();
    let _ = crate::sync::Condvar::new();
    #[cfg(feature = "pl")]
    make_gated();
}
"#,
    );
    let pl = "feature = \"pl\"";
    let new = "inherit::pl::Condvar::new";
    let ty = "inherit::pl::Condvar";
    assert_eq!(
        edge_cfg(&doc, "inherit::make", new, EdgeKind::Call),
        under(pl)
    );
    assert_eq!(
        edge_cfg(&doc, "inherit::make", ty, EdgeKind::Signature),
        under(pl)
    );
    assert_eq!(
        edge_cfg(&doc, "inherit::main", new, EdgeKind::Call),
        under(pl)
    );
    assert_eq!(
        edge_cfg(&doc, "inherit::user::make", new, EdgeKind::Call),
        under(pl)
    );
    assert_eq!(
        edge_cfg(&doc, "inherit::user", ty, EdgeKind::Uses),
        under(pl)
    );
    // 아이템과 임포트의 조건이 같으면 간선의 조건 표기는 그대로다.
    assert_eq!(
        edge_cfg(&doc, "inherit::make_gated", new, EdgeKind::Call),
        under(pl)
    );
}

/// 테스트 빌드에서만 들이는 대안(`#[cfg(test)] use crate::fake::Clock;`)도 남고, `--exclude-tests`가 그 간선을
/// 지운다. 예전에는 뒤의 `#[cfg(not(test))]` 쪽만 조건 없이 남아, 테스트 빌드에서 쓰이는 `fake::Clock::now`가
/// 늘 unreachable이었고 `real` 쪽 간선은 `--exclude-tests`와 무관했다.
#[test]
fn test_only_alternative_is_dropped_by_exclude_tests() {
    let doc = harvest(
        "clock",
        "",
        r#"
mod real {
    pub struct Clock;
    impl Clock {
        pub fn now() -> u64 {
            1
        }
    }
}
mod fake {
    pub struct Clock;
    impl Clock {
        pub fn now() -> u64 {
            0
        }
    }
}
#[cfg(test)]
use crate::fake::Clock;
#[cfg(not(test))]
use crate::real::Clock;
fn stamp() -> u64 {
    Clock::now()
}
fn main() {
    stamp();
}
"#,
    );
    let stamp = "clock::stamp";
    assert_eq!(
        edge_cfg(&doc, stamp, "clock::fake::Clock::now", EdgeKind::Call),
        under("test")
    );
    assert_eq!(
        edge_cfg(&doc, stamp, "clock::real::Clock::now", EdgeKind::Call),
        under("not (test)")
    );
    assert!(!dead_ids(&doc).contains(&"clock::fake::Clock::now".to_string()));
    let without_tests = dead_ids(&doc.without_tests());
    assert!(without_tests.contains(&"clock::fake::Clock::now".to_string()));
    assert!(!without_tests.contains(&"clock::real::Clock::now".to_string()));
}

/// 이름을 묶지 않는 `use T as _`는 여럿이어도 서로 가리지 않는다 — 모두 그 모듈의 `uses` 의존 증거다. 예전에는
/// 한 모듈의 `use crate::ext::{AExt as _, BExt as _}`에서 뒤의 것만 남아 `AExt`로 가는 `uses` 간선이 없었다
/// (syn 2.0.119의 `syn::error`가 `PunctExt`를 그렇게 잃어 `dead --retain-public`에 거짓으로 나왔다).
#[test]
fn every_underscore_import_is_use_evidence() {
    let doc = harvest(
        "under",
        "",
        r#"
mod ext {
    pub(crate) trait AExt {
        fn a(&self) -> u8;
    }
    impl AExt for u8 {
        fn a(&self) -> u8 {
            *self
        }
    }
    pub(crate) trait BExt {
        fn b(&self) -> u8;
    }
    impl BExt for u8 {
        fn b(&self) -> u8 {
            *self
        }
    }
}
mod user {
    use crate::ext::{AExt as _, BExt as _};
    pub fn run() -> u8 {
        1u8.a() + 2u8.b()
    }
}
fn main() {
    user::run();
}
"#,
    );
    for tr in ["under::ext::AExt", "under::ext::BExt"] {
        assert_eq!(
            edge_cfg(&doc, "under::user", tr, EdgeKind::Uses),
            Some(None),
            "{tr}"
        );
    }
}

/// 조건 없는 글롭 옆의 `#[cfg]` 명시 `use`는 글롭을 가리지 않는다 — 그 조건이 아닌 빌드에서는 글롭의 이름이
/// 보인다. `use a::*;` 옆 `#[cfg(test)] use a::f;`의 `f()`는 같은 대상이라 조건 없는 간선이고, 재노출을 거쳐도
/// 같다. 이전 브랜치 머리(187163b)에서는 명시 `use`가 글롭 칸을 바꿔 그 간선이 `cfg(test)`를 달아,
/// `--exclude-tests`와 `--target`이 테스트가 아닌 빌드에서 쓰이는 `a::f`를 unreachable로 냈다(rustc는 경고 없음).
#[test]
fn cfg_explicit_use_does_not_hide_an_unconditional_glob() {
    let doc = harvest(
        "glob_and_gated",
        "",
        r#"
mod a {
    pub fn f() {}
    pub fn g() {}
}
mod util {
    pub fn h() {}
}
mod re {
    pub use crate::util::*;
    #[cfg(test)]
    pub use crate::util::h;
}
use a::*;
#[cfg(test)]
use a::f;
fn main() {
    f();
    g();
    re::h();
}
"#,
    );
    let main = "glob_and_gated::main";
    assert_eq!(
        edge_cfg(&doc, main, "glob_and_gated::a::f", EdgeKind::Call),
        Some(None)
    );
    assert_eq!(
        edge_cfg(&doc, main, "glob_and_gated::util::h", EdgeKind::Call),
        Some(None)
    );
    for filtered in [doc.without_tests(), for_target(&doc, LINUX)] {
        assert!(dead_ids(&filtered).is_empty(), "{:?}", dead_ids(&filtered));
    }
}

/// 다른 대상이면 둘 다 남는다 — `use real::*;` 옆 `#[cfg(test)] use mock::Clock;`의 `Clock::now()`는 테스트 빌드의
/// `mock`과 그 밖의 빌드의 `real`을 부른다. 예전에는(9a79ce5·187163b 모두) 명시 `use`가 글롭을 늘 가려, 테스트가
/// 아닌 빌드에서 쓰이는 `real::Clock`·`real::Clock::now`가 기본 `dead`에 나왔다.
#[test]
fn cfg_explicit_use_beside_a_glob_of_another_target_is_an_alternative() {
    let doc = harvest(
        "mockclock",
        "",
        r#"
mod real {
    pub struct Clock;
    impl Clock {
        pub fn now() -> u32 {
            1
        }
    }
}
#[cfg(test)]
mod mock {
    pub struct Clock;
    impl Clock {
        pub fn now() -> u32 {
            2
        }
    }
}
use real::*;
#[cfg(test)]
use mock::Clock;
fn main() {
    let _ = Clock::now();
}
"#,
    );
    let main = "mockclock::main";
    assert_eq!(
        edge_cfg(&doc, main, "mockclock::mock::Clock::now", EdgeKind::Call),
        under("test")
    );
    assert_eq!(
        edge_cfg(&doc, main, "mockclock::real::Clock::now", EdgeKind::Call),
        Some(None)
    );
    assert!(!dead_ids(&doc).contains(&"mockclock::real::Clock::now".to_string()));
    assert!(!dead_ids(&doc.without_tests()).contains(&"mockclock::real::Clock::now".to_string()));
}

/// 모든 선언이 `#[cfg]`를 단 로컬 아이템 옆의 같은 이름공간 임포트는 그 아이템이 없는 빌드의 대안이다 —
/// `#[cfg(feature = "x")] fn sum()` 옆 `#[cfg(not(feature = "x"))] use scalar::sum;`의 `sum()`은 두 정점을 부른다.
/// 예전에는 로컬 아이템이 늘 이겨, 기본 빌드에서 쓰이는 `scalar::sum`이 `dead`에 나왔다.
#[test]
fn gated_local_item_and_same_namespace_import_are_alternatives() {
    let doc = harvest(
        "gatedlocal",
        "x = []",
        r#"
mod scalar {
    pub fn sum() -> u32 {
        1
    }
}
#[cfg(feature = "x")]
fn sum() -> u32 {
    2
}
#[cfg(not(feature = "x"))]
use scalar::sum;
fn main() {
    let _ = sum();
}
"#,
    );
    let main = "gatedlocal::main";
    assert_eq!(
        edge_cfg(&doc, main, "gatedlocal::sum", EdgeKind::Call),
        Some(None)
    );
    assert_eq!(
        edge_cfg(&doc, main, "gatedlocal::scalar::sum", EdgeKind::Call),
        under("not (feature = \"x\")")
    );
    assert!(dead_ids(&doc).is_empty(), "{:?}", dead_ids(&doc));
}

/// `#[cfg]`로 갈린 대안 임포트를 self 타입으로 쓴 impl(`impl Sys { .. }`)은 대안마다 수확한다 — 메서드 정점은 그
/// 대안 타입 아래에 그 임포트의 조건을 달고 생긴다. 187163b는 뒤의 `use`(windows) 아래에만 정점을 두고 그리로 가는
/// 유일한 간선에 `cfg(windows)`를 달아, `--target` 리눅스가 리눅스에서 쓰이는 `extra`를 unreachable로 냈다.
#[test]
fn impl_on_a_cfg_split_import_is_harvested_per_alternative() {
    let doc = harvest(
        "splitimpl",
        "",
        r#"
mod unix_impl {
    pub struct Sys;
}
mod win_impl {
    pub struct Sys;
}
#[cfg(unix)]
use crate::unix_impl::Sys;
#[cfg(windows)]
use crate::win_impl::Sys;
impl Sys {
    pub fn extra() -> u32 {
        7
    }
}
fn main() {
    let _ = Sys::extra();
    let _s = Sys;
}
"#,
    );
    let vertex_cfg = |id: &str| {
        doc.vertices
            .iter()
            .find(|v| v.id == id)
            .map(|v| v.cfg.clone())
    };
    assert_eq!(
        vertex_cfg("splitimpl::unix_impl::Sys::extra"),
        under("unix")
    );
    assert_eq!(
        vertex_cfg("splitimpl::win_impl::Sys::extra"),
        under("windows")
    );
    assert_eq!(
        edge_cfg(
            &doc,
            "splitimpl::main",
            "splitimpl::unix_impl::Sys::extra",
            EdgeKind::Call
        ),
        under("unix")
    );
    let on_linux = dead_ids(&for_target(&doc, LINUX));
    assert_eq!(on_linux, ["splitimpl::win_impl::Sys"]);
    let on_windows = dead_ids(&for_target(&doc, WINDOWS));
    assert_eq!(on_windows, ["splitimpl::unix_impl::Sys"]);
}

/// 해석되는 `use`와 해석되지 않는 `use`(std 경로)가 같은 칸을 다투면 소스 순서와 무관하게 해석되는 쪽이 남는다 —
/// `#[cfg(feature = "pl")] use extdep::Mutex;` 뒤에 `#[cfg(not(..))] use std::sync::Mutex;`가 와도 `extdep`을 쓰는
/// 증거가 남는다. 예전에는(9a79ce5·187163b 모두) 뒤의 std `use`가 이겨 `deps`가 `extdep`을 unused로 냈다.
#[test]
fn external_import_survives_a_later_unresolved_alternative() {
    let unused = load_files(
        "extalt",
        &[
            (
                "Cargo.toml",
                "[package]\nname='extalt'\nversion='0.1.0'\nedition='2021'\n[features]\npl = []\n\
                 [dependencies]\nextdep = { path = 'extdep' }\n[workspace]\nexclude = ['extdep']\n",
            ),
            (
                "src/main.rs",
                "#[cfg(feature = \"pl\")]\nuse extdep::Mutex;\n#[cfg(not(feature = \"pl\"))]\nuse std::sync::Mutex;\n\
                 fn main() {\n    let _m = Mutex::new(());\n}\n",
            ),
            (
                "extdep/Cargo.toml",
                "[package]\nname='extdep'\nversion='0.1.0'\nedition='2021'\n",
            ),
            (
                "extdep/src/lib.rs",
                "pub struct Mutex<T>(pub T);\nimpl<T> Mutex<T> {\n    pub fn new(t: T) -> Self {\n        Mutex(t)\n    }\n}\n",
            ),
        ],
        |root| {
            deps::report(root)
                .unwrap()
                .unused
                .iter()
                .map(|u| format!("{} -> {}", u.package, u.dep))
                .collect::<Vec<_>>()
        },
    );
    assert!(unused.is_empty(), "{unused:?}");
}

/// 앞 바퀴의 해석이 뒤에 거둬진 대상(`z`의 `pub use foo::bar`가 첫 바퀴에 `z`의 글롭이 펼쳐지기 전이라 크레이트
/// 루트 `foo::bar`로 읽힌 것)은 서로를 글롭으로 들이는 `x`·`y` 사이에 남지 않는다. 9a79ce5는 그 거짓 대상만,
/// 187163b는 거짓 대상과 제 대상을 함께 이었다 — rustc는 루트의 `foo::bar`를 쓰이지 않는다고 경고한다.
#[test]
fn retracted_first_pass_target_does_not_survive_a_glob_cycle() {
    let doc = harvest(
        "stale",
        "",
        r#"
mod x {
    pub use crate::z::*;
    #[allow(unused_imports)]
    pub use crate::y::*;
}
mod y {
    #[allow(unused_imports)]
    pub use crate::x::*;
}
mod foo {
    pub fn bar() {}
}
mod real {
    pub mod foo {
        pub fn bar() {}
    }
}
mod z {
    use crate::real::*;
    pub use foo::bar;
}
fn main() {
    x::bar();
}
"#,
    );
    assert_eq!(
        edge_cfg(&doc, "stale::main", "stale::real::foo::bar", EdgeKind::Call),
        Some(None)
    );
    assert_eq!(
        edge_cfg(&doc, "stale::main", "stale::foo::bar", EdgeKind::Call),
        None
    );
    assert_eq!(dead_ids(&doc), ["stale::foo::bar"]);
}

/// 명시 `use T as _`는 글롭이 들인 `_`(`pub use AExt as _`를 다시 들인 것)를 가리지 않는다 — 이름을 묶지 않는다.
#[test]
fn explicit_underscore_import_keeps_a_glob_underscore_import() {
    let doc = harvest(
        "globunder",
        "",
        r#"
mod ext {
    pub trait AExt {
        fn a(&self) -> u8;
    }
    impl AExt for u8 {
        fn a(&self) -> u8 {
            *self
        }
    }
    pub trait BExt {
        fn b(&self) -> u8;
    }
    impl BExt for u8 {
        fn b(&self) -> u8 {
            *self
        }
    }
}
mod prelude {
    pub use crate::ext::AExt as _;
}
mod user {
    use crate::ext::BExt as _;
    use crate::prelude::*;
    pub fn run() -> u8 {
        1u8.a() + 2u8.b()
    }
}
fn main() {
    user::run();
}
"#,
    );
    for tr in ["globunder::ext::AExt", "globunder::ext::BExt"] {
        assert_eq!(
            edge_cfg(&doc, "globunder::user", tr, EdgeKind::Uses),
            Some(None),
            "{tr}"
        );
    }
}

/// 필드 타입과 값으로 넘긴 연관 함수(`let open = Handle::open;`)도 대안마다 간선이고 그 임포트의 조건을 싣는다.
#[test]
fn field_types_and_associated_fns_as_values_follow_each_alternative() {
    let doc = harvest(
        "fieldalt",
        "",
        r#"
mod unix_impl {
    pub struct Handle;
    impl Handle {
        pub fn open() -> Handle {
            Handle
        }
    }
}
mod win_impl {
    pub struct Handle;
    impl Handle {
        pub fn open() -> Handle {
            Handle
        }
    }
}
#[cfg(unix)]
use crate::unix_impl::Handle;
#[cfg(windows)]
use crate::win_impl::Handle;
pub struct Holder {
    pub handle: Handle,
}
fn main() {
    let open = Handle::open;
    let _ = Holder { handle: open() };
}
"#,
    );
    for (platform, cfg) in [("unix", "unix"), ("win", "windows")] {
        assert_eq!(
            edge_cfg(
                &doc,
                "fieldalt::Holder",
                &format!("fieldalt::{platform}_impl::Handle"),
                EdgeKind::References
            ),
            under(cfg)
        );
        assert_eq!(
            edge_cfg(
                &doc,
                "fieldalt::main",
                &format!("fieldalt::{platform}_impl::Handle::open"),
                EdgeKind::References
            ),
            under(cfg)
        );
    }
}

/// 속성(derive 인자) 경로도 대안마다 간선이고 그 임포트의 조건을 싣는다 — `#[cfg(unix)] use pma as dm;`와
/// `#[cfg(not(unix))] use pmb as dm;` 옆의 `#[derive(dm::Hello)]`.
#[test]
fn attribute_paths_follow_each_alternative() {
    let pm = |name: &str| {
        format!(
            "extern crate proc_macro;\nuse proc_macro::TokenStream;\n#[proc_macro_derive(Hello)]\n\
             pub fn hello_{name}(_i: TokenStream) -> TokenStream {{\n    TokenStream::new()\n}}\n"
        )
    };
    let manifest = |name: &str| {
        format!(
            "[package]\nname='{name}'\nversion='0.1.0'\nedition='2021'\n[lib]\nproc-macro=true\n"
        )
    };
    let (pma, pmb, pma_toml, pmb_toml) = (pm("pma"), pm("pmb"), manifest("pma"), manifest("pmb"));
    let doc = harvest_files(
        "attralt",
        &[
            (
                "Cargo.toml",
                "[workspace]\nmembers=['app','pma','pmb']\nresolver='2'\n",
            ),
            ("pma/Cargo.toml", &pma_toml),
            ("pma/src/lib.rs", &pma),
            ("pmb/Cargo.toml", &pmb_toml),
            ("pmb/src/lib.rs", &pmb),
            (
                "app/Cargo.toml",
                "[package]\nname='app'\nversion='0.1.0'\nedition='2021'\n\
                 [dependencies]\npma={path='../pma'}\npmb={path='../pmb'}\n",
            ),
            (
                "app/src/lib.rs",
                "#[cfg(unix)]\nuse pma as dm;\n#[cfg(not(unix))]\nuse pmb as dm;\n\n#[derive(dm::Hello)]\npub struct S;\n",
            ),
        ],
    );
    assert_eq!(
        edge_cfg(&doc, "app::S", "pma::hello_pma", EdgeKind::References),
        under("unix")
    );
    assert_eq!(
        edge_cfg(&doc, "app::S", "pmb::hello_pmb", EdgeKind::References),
        under("not (unix)")
    );
}

/// 글롭 이름은 같은 이름의 로컬 아이템 옆에 합법적으로 있으면서 가려지기만 한다(E0255는 명시 `use`만이다). 로컬
/// 아이템의 선언이 여럿이면(`#[cfg(unix)] fn sys`와 `#[cfg(not(unix))] fn sys`) 모든 빌드를 덮을 수 있어 글롭 이름은
/// 대안이 아니다 — 826e20b는 대안으로 보고 `m::run → sys`(루트)라는 거짓 간선과 거짓 순환 `m::run → sys`를 냈고,
/// 글롭으로 다시 들인 모듈(`user`)에서도 `common::open`으로 가는 거짓 간선을 냈다. rustc는 루트 `sys`와
/// `common::open`을 쓰이지 않는다고 경고한다(187163b와 같은 보고).
#[test]
fn glob_name_beside_complementary_gated_locals_is_not_an_alternative() {
    let doc = harvest(
        "gatedpair",
        "",
        r#"
fn sys() -> u32 {
    m::run()
}
mod m {
    #[allow(unused_imports)]
    use super::*;
    #[cfg(unix)]
    fn sys() -> u32 {
        1
    }
    #[cfg(not(unix))]
    fn sys() -> u32 {
        2
    }
    pub fn run() -> u32 {
        sys()
    }
}
mod common {
    pub fn open() -> u32 {
        0
    }
    pub fn close() -> u32 {
        0
    }
}
mod plat {
    pub use crate::common::*;
    #[cfg(unix)]
    pub fn open() -> u32 {
        1
    }
    #[cfg(not(unix))]
    pub fn open() -> u32 {
        2
    }
}
mod user {
    use crate::plat::*;
    pub fn run() -> u32 {
        open() + close()
    }
}
fn main() {
    let _ = m::run();
    let _ = user::run();
}
"#,
    );
    assert_eq!(
        edge_cfg(&doc, "gatedpair::m::run", "gatedpair::sys", EdgeKind::Call),
        None
    );
    assert_eq!(
        edge_cfg(
            &doc,
            "gatedpair::user::run",
            "gatedpair::common::open",
            EdgeKind::Call
        ),
        None
    );
    assert!(analysis::cycles(&doc).cycles.is_empty());
    assert_eq!(
        dead_ids(&doc),
        ["gatedpair::common::open", "gatedpair::sys"]
    );
}

/// 선언이 하나인 `#[cfg]` 로컬 아이템 옆의 글롭 이름은 그 아이템이 없는 빌드의 대안이다 — 간선은 그 조건의 부정을
/// 단다. `plat`의 `#[cfg(unix)] pub fn open` 옆 `pub use crate::common::*`를 글롭으로 들인 `user`의 `open()`은 리눅스에서
/// `plat::open`, 그 밖에서 `common::open`이다(rustc 리눅스 빌드는 `common::open`을 쓰이지 않는다고 경고한다).
#[test]
fn glob_name_beside_a_single_gated_local_is_an_alternative_elsewhere() {
    let doc = harvest(
        "gatedone",
        "",
        r#"
mod common {
    pub fn open() -> u32 {
        0
    }
}
mod plat {
    pub use crate::common::*;
    #[cfg(unix)]
    pub fn open() -> u32 {
        1
    }
}
mod user {
    use crate::plat::*;
    pub fn run() -> u32 {
        open()
    }
}
fn main() {
    let _ = user::run();
}
"#,
    );
    let run = "gatedone::user::run";
    assert_eq!(
        edge_cfg(&doc, run, "gatedone::plat::open", EdgeKind::Call),
        Some(None)
    );
    assert_eq!(
        edge_cfg(&doc, run, "gatedone::common::open", EdgeKind::Call),
        under("not (unix)")
    );
    assert!(dead_ids(&doc).is_empty(), "{:?}", dead_ids(&doc));
    assert_eq!(
        dead_ids(&for_target(&doc, LINUX)),
        ["gatedone::common::open"]
    );
    assert!(dead_ids(&for_target(&doc, WINDOWS)).is_empty());
}

/// `#[cfg] mod imp` 옆의 `#[cfg(not(..))] use crate::other as imp;`도 대안이다 — 경로 접두사(`imp::f()`,
/// `crate::imp::S::m()`)와 글롭(`use crate::imp::*`) 모두 두 모듈로 간다. 예전에는 `mod` 선언이 자식 모듈 표에 먼저
/// 들어가 있어 조건 없는 앞선 선언으로 보였고, 기본 빌드에서 쓰이는 `other::f`·`other::S::m`이 `dead`에 나왔다.
#[test]
fn gated_module_and_alias_import_are_alternatives() {
    let doc = harvest(
        "gatedmod",
        "x = []",
        r#"
mod other {
    pub fn f() -> u32 {
        2
    }
    pub struct S;
    impl S {
        pub fn m() -> u32 {
            5
        }
    }
}
#[cfg(feature = "x")]
mod imp {
    pub fn f() -> u32 {
        1
    }
    pub struct S;
    impl S {
        pub fn m() -> u32 {
            6
        }
    }
}
#[cfg(not(feature = "x"))]
use crate::other as imp;
mod user {
    pub fn run() -> u32 {
        crate::imp::f() + crate::imp::S::m()
    }
}
mod user2 {
    use crate::imp::*;
    pub fn run() -> u32 {
        f()
    }
}
fn main() {
    let _ = user::run();
    let _ = user2::run();
}
"#,
    );
    let off = "not (feature = \"x\")";
    for (from, to) in [
        ("gatedmod::user::run", "gatedmod::other::f"),
        ("gatedmod::user::run", "gatedmod::other::S::m"),
        ("gatedmod::user2::run", "gatedmod::other::f"),
    ] {
        assert_eq!(edge_cfg(&doc, from, to, EdgeKind::Call), under(off), "{to}");
    }
    assert_eq!(
        edge_cfg(
            &doc,
            "gatedmod::user::run",
            "gatedmod::imp::f",
            EdgeKind::Call
        ),
        Some(None)
    );
    let dead = dead_ids(&doc);
    for used in [
        "gatedmod::other::f",
        "gatedmod::other::S::m",
        "gatedmod::imp::f",
    ] {
        assert!(!dead.contains(&used.to_string()), "{used}: {dead:?}");
    }
}

/// impl의 트레이트 경로가 `#[cfg]`로 갈린 대안 임포트를 지나면(`use unix_impl as plat;`·`use win_impl as plat;` 옆의
/// `impl plat::Tr for X`) 대안마다 implements 간선이다 — 예전에는 뒤의 것(windows)만 이어 기본 `dead`와 `--target`
/// 리눅스가 `unix_impl::Tr`을 냈다. 이제 리눅스에서는 rustc처럼 `win_impl::Tr`만 낸다.
#[test]
fn split_trait_path_gets_an_implements_edge_per_alternative() {
    let doc = harvest(
        "splittrait",
        "",
        r#"
mod unix_impl {
    pub trait Tr {
        fn t(&self) -> u32;
    }
}
mod win_impl {
    pub trait Tr {
        fn t(&self) -> u32;
    }
}
#[cfg(unix)]
use crate::unix_impl as plat;
#[cfg(windows)]
use crate::win_impl as plat;
struct X;
impl plat::Tr for X {
    fn t(&self) -> u32 {
        1
    }
}
fn run<T: plat::Tr>(x: &T) -> u32 {
    x.t()
}
fn main() {
    let _ = run(&X);
}
"#,
    );
    let x = "splittrait::X";
    assert_eq!(
        edge_cfg(&doc, x, "splittrait::unix_impl::Tr", EdgeKind::Implements),
        under("unix")
    );
    assert_eq!(
        edge_cfg(&doc, x, "splittrait::win_impl::Tr", EdgeKind::Implements),
        under("windows")
    );
    assert!(dead_ids(&doc).is_empty(), "{:?}", dead_ids(&doc));
    assert_eq!(
        dead_ids(&for_target(&doc, LINUX)),
        ["splittrait::win_impl::Tr"]
    );
}

/// 2015 판 크레이트에서는 `use foo::bar`가 크레이트 기준이라 수렴 중 거둔 대상(크레이트 루트 `foo::bar`)이 제 대상일 수
/// 있다 — 거둔 대상을 지우지 않고(187163b와 같다) 그렇게 남긴 크레이트 수를 limitation으로 싣는다. 826e20b는 그것을
/// 지워, 2015 판 rustc가 쓰는 루트 `foo::bar`를 `dead`로 냈다.
#[test]
fn edition_2015_keeps_retracted_targets_and_says_so() {
    let manifest = "[package]\nname='legacy'\nversion='0.1.0'\nedition='2015'\n[workspace]\n";
    let main = r#"
mod x {
    pub use z::*;
    #[allow(unused_imports)]
    pub use y::*;
}
mod y {
    #[allow(unused_imports)]
    pub use x::*;
}
mod foo {
    pub fn bar() {}
}
mod real {
    pub mod foo {
        pub fn bar() {}
    }
}
mod z {
    #[allow(unused_imports)]
    use real::*;
    pub use foo::bar;
}
fn main() {
    x::bar();
}
"#;
    let doc = harvest_files("legacy", &[("Cargo.toml", manifest), ("src/main.rs", main)]);
    assert!(!dead_ids(&doc).contains(&"legacy::foo::bar".to_string()));
    assert!(
        doc.limitations
            .iter()
            .any(|l| l.starts_with("1 edition-2015 crates kept import targets")),
        "{:?}",
        doc.limitations
    );
}

/// 거둔 대상은 재노출 사슬(`w1`·`w2`가 차례로 `pub use crate::x::bar`를 다시 내보냄)을 거쳐도 지워지고, 수렴하지 못했다는
/// limitation은 없다. 826e20b는 사슬이 길면(고리 셋 이상) 깨끗하게 다시 쌓는 횟수(4)가 모자라 모든 모듈을 "still changed"로
/// 세거나 거짓 간선을 남겼다.
#[test]
fn retracted_targets_are_cleaned_through_reexport_chains() {
    let doc = harvest(
        "stalechain",
        "",
        r#"
mod x {
    pub use crate::z::*;
    #[allow(unused_imports)]
    pub use crate::y::*;
}
mod y {
    #[allow(unused_imports)]
    pub use crate::x::*;
}
mod foo {
    pub fn bar() {}
}
mod real {
    pub mod foo {
        pub fn bar() {}
    }
}
mod z {
    use crate::real::*;
    pub use foo::bar;
}
mod w1 {
    pub use crate::x::bar;
}
mod w2 {
    pub use crate::w1::bar;
}
mod w3 {
    pub use crate::w2::bar;
}
mod w4 {
    pub use crate::w3::bar;
}
fn main() {
    w4::bar();
}
"#,
    );
    assert_eq!(
        edge_cfg(
            &doc,
            "stalechain::main",
            "stalechain::real::foo::bar",
            EdgeKind::Call
        ),
        Some(None)
    );
    assert_eq!(dead_ids(&doc), ["stalechain::foo::bar"]);
    assert!(
        !doc.limitations
            .iter()
            .any(|l| l.contains("still changed") || l.contains("stale re-export")),
        "{:?}",
        doc.limitations
    );
}

/// 같은 조건의 글롭과 명시 `use`는 늘 함께 있어 명시 `use`가 글롭을 가린다 — `#[cfg(feature = "x")] use a::*;` 옆
/// `#[cfg(feature = "x")] use b::f;`의 `f()`는 `b::f`뿐이다(rustc `--features x`는 `a::f`를 쓰이지 않는다고 경고한다).
#[test]
fn cfg_explicit_use_hides_a_glob_under_the_same_cfg() {
    let doc = harvest(
        "samecfg",
        "x = []",
        r#"
mod a {
    pub fn f() -> u32 {
        1
    }
    pub fn g() -> u32 {
        1
    }
}
mod b {
    pub fn f() -> u32 {
        2
    }
}
#[cfg(feature = "x")]
use a::*;
#[cfg(feature = "x")]
use b::f;
fn main() {
    #[cfg(feature = "x")]
    let _ = f() + g();
}
"#,
    );
    assert_eq!(
        edge_cfg(&doc, "samecfg::main", "samecfg::a::f", EdgeKind::Call),
        None
    );
    assert_eq!(
        edge_cfg(&doc, "samecfg::main", "samecfg::b::f", EdgeKind::Call),
        under("feature = \"x\"")
    );
    assert_eq!(dead_ids(&doc), ["samecfg::a::f"]);
}

/// 문법을 모르는 매크로 인자 토큰(`matches!(sum(), n if n > 0)`)처럼 자리를 모르는 해석도 `#[cfg]` 로컬 아이템과 그
/// 대안 임포트를 함께 준다(추정 간선).
#[test]
fn position_agnostic_resolution_reaches_gated_local_alternatives() {
    let doc = harvest(
        "mention",
        "x = []",
        r#"
mod scalar {
    pub fn sum() -> u32 {
        1
    }
}
#[cfg(feature = "x")]
fn sum() -> u32 {
    2
}
#[cfg(not(feature = "x"))]
use scalar::sum;
fn main() {
    let _ = matches!(sum(), n if n > 0);
}
"#,
    );
    for to in ["mention::sum", "mention::scalar::sum"] {
        assert!(
            doc.edges
                .iter()
                .any(|e| e.from == "mention::main" && e.to == to && e.tentative),
            "{to}"
        );
    }
}

/// 자기 `#[cfg]`를 단, 해석되지 않는 명시 `use`(`#[cfg(feature = "x")] use std::time::Instant as Clock;`)도 글롭을 가리지
/// 않는다 — 그 조건이 아닌 빌드의 `Clock::now()`는 글롭의 `real::Clock::now`다.
#[test]
fn conditional_unresolved_use_does_not_hide_a_glob() {
    let doc = harvest(
        "condstd",
        "x = []",
        r#"
mod real {
    pub struct Clock;
    impl Clock {
        pub fn now() -> u32 {
            1
        }
    }
}
use real::*;
#[cfg(feature = "x")]
use std::time::Instant as Clock;
fn main() {
    #[cfg(not(feature = "x"))]
    let _ = Clock::now();
}
"#,
    );
    assert_eq!(
        edge_cfg(
            &doc,
            "condstd::main",
            "condstd::real::Clock::now",
            EdgeKind::Call
        ),
        Some(None)
    );
    assert!(!dead_ids(&doc).contains(&"condstd::real::Clock::now".to_string()));
}

/// 대안마다 수확하는 impl도 수확 계수(cfg 아이템)는 블록마다 한 번이다 — `#[cfg(test)]` 메서드 하나는 한 번 센다.
#[test]
fn split_impl_counts_harvest_statistics_once() {
    let doc = harvest(
        "countimpl",
        "",
        r#"
mod unix_impl {
    pub struct Sys;
}
mod win_impl {
    pub struct Sys;
}
#[cfg(unix)]
use crate::unix_impl::Sys;
#[cfg(windows)]
use crate::win_impl::Sys;
impl Sys {
    #[cfg(test)]
    pub fn probe() -> u32 {
        0
    }
    pub fn extra() -> u32 {
        7
    }
}
fn main() {
    let _ = Sys::extra();
}
"#,
    );
    assert!(
        doc.limitations
            .iter()
            .any(|l| l.starts_with("1 items behind #[cfg]")),
        "{:?}",
        doc.limitations
    );
}
