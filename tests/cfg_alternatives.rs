//! `#[cfg]`로 갈린 대안 임포트와 임포트 조건의 전파를 실제 수확으로 검증한다.
//!
//! rustc는 한 빌드의 한 이름공간에 같은 이름을 들이는 서로 다른 `use` 둘을 두지 않는다(명시 `use`는 E0252, 글롭
//! 이름은 쓰면 E0659). 그런 둘이 보이면 `#[cfg]`로 갈린 서로 다른 빌드의 것이다 — 모든 빌드의 합집합을 보는
//! 그래프는 둘 다 남기고, 그 이름으로 해석한 간선은 거쳐 온 임포트의 조건을 싣는다. 각 fixture는 rustc로
//! 컴파일되는 코드다(`cargo check`, `cargo check --tests`).

use rustograph::cfgeval::Facts;
use rustograph::graph::{Document, EdgeKind};
use rustograph::{analysis, source};

const LINUX: &str = "x86_64-unknown-linux-gnu";
const WINDOWS: &str = "x86_64-pc-windows-msvc";

/// 임시 디렉터리의 단일 패키지 워크스페이스(`[features]`는 `features`)를 심볼 레벨로 수확하고 지운다.
fn harvest(tag: &str, features: &str, main: &str) -> Document {
    let root = std::env::temp_dir().join(format!(
        "rustograph-cfg-alternatives-{tag}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname='{tag}'\nversion='0.1.0'\nedition='2021'\n\
             [features]\n{features}\n[workspace]\n"
        ),
    )
    .unwrap();
    std::fs::write(root.join("src/main.rs"), main).unwrap();
    let document = source::load(
        &root,
        &source::Options {
            symbol_level: true,
            ..Default::default()
        },
    );
    std::fs::remove_dir_all(&root).unwrap();
    document.unwrap()
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
