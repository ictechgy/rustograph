//! `dead`의 보존 루트 밖 사용 — 언어나 외부 크레이트가 부르는 std/외부 트레이트 impl 메서드와,
//! 워크스페이스 밖에서 불리는 라이브러리 공개 API — 를 실제 수확으로 검증한다.

use rustograph::{analysis, source};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// 테스트마다 임시 디렉터리에 만든 단일 패키지 워크스페이스 — 끝나면 지운다.
struct Crate(PathBuf);

impl Crate {
    fn new(tag: &str, files: &[(&str, &str)]) -> Crate {
        let root = std::env::temp_dir().join(format!(
            "rustograph-dead-retention-{tag}-{}",
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
        Crate(root)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Crate {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn manifest(name: &str) -> String {
    format!("[package]\nname='{name}'\nversion='0.1.0'\nedition='2021'\n[workspace]\n")
}

fn symbols(dir: &Path) -> rustograph::graph::Document {
    source::load(
        dir,
        &source::Options {
            symbol_level: true,
            ..Default::default()
        },
    )
    .unwrap()
}

/// 보존 선택지를 켠 [`analysis::DeadOptions`] — 크레이트 밖 호출자처럼 기본값에서 필드를 바꿔 만든다
/// (`#[non_exhaustive]`라 구조체 리터럴은 받지 않는다).
fn retaining_external_trait_impls() -> analysis::DeadOptions {
    let mut opts = analysis::DeadOptions::default();
    opts.retain_external_trait_impls = true;
    opts
}

fn ids(rep: &analysis::DeadReport) -> Vec<&str> {
    rep.findings.iter().map(|f| f.id.as_str()).collect()
}

fn run(dir: &Path, args: &[&str]) -> (Option<i32>, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_rustograph"))
        .args(args)
        .arg("--dir")
        .arg(dir)
        .output()
        .expect("rustograph binary must start");
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    (out.status.code(), String::from_utf8(out.stdout).unwrap())
}

/// `for`가 부르는 `Iterator::next`, 스코프 끝이 부르는 `Drop::drop`, 포맷 매크로가 부르는
/// `Display::fmt`, `.into()`가 부르는 `From::from` 두 개 — 모두 std 트레이트 impl이다. 그 안에서만
/// 부르는 함수(`bump`·`cleanup`·`helper_in_macro`)가 있다. 워크스페이스 트레이트 `Local`의 미사용
/// impl, 아무도 만들지 않는 `Unused`의 `Drop`과 그것만 부르는 함수는 진짜 미사용이다.
const TRAIT_IMPLS: &str = r#"
use std::fmt;

pub trait Local {
    fn local_m(&self);
}

struct Counter(u32);
impl Iterator for Counter {
    type Item = u32;
    fn next(&mut self) -> Option<u32> {
        bump();
        if self.0 < 3 { self.0 += 1; Some(self.0) } else { None }
    }
}
fn bump() {}

struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        cleanup();
    }
}
fn cleanup() {}

struct X;
struct Y;
struct P;
impl fmt::Display for P {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", helper_in_macro())
    }
}
fn helper_in_macro() -> u32 { 1 }
impl From<X> for P { fn from(_: X) -> P { P } }
impl From<Y> for P { fn from(_: Y) -> P { P } }

struct Unused;
impl Drop for Unused { fn drop(&mut self) { only_from_unused_drop(); } }
fn only_from_unused_drop() {}

struct L;
impl Local for L { fn local_m(&self) {} }

fn main() {
    for _ in Counter(0) {}
    let _g = Guard;
    let p: P = X.into();
    let _q: P = Y.into();
    println!("{p}");
    let _l = L;
}
"#;

/// 기본 판정은 바뀌지 않는다 — std 트레이트 impl과 그 안에서만 부르는 함수가 unreachable로
/// 남는다. 대신 닿는 타입에 붙은 그런 메서드 수와 보존 선택지가 limitation에 실린다.
#[test]
fn default_dead_keeps_findings_and_counts_external_trait_impls() {
    let krate = Crate::new(
        "default",
        &[
            ("Cargo.toml", &manifest("rgfx")),
            ("src/main.rs", TRAIT_IMPLS),
        ],
    );
    let doc = symbols(krate.path());
    let rep = analysis::dead(&doc).unwrap();
    let found = ids(&rep);
    for id in [
        "rgfx::Counter::<Iterator>::next",
        "rgfx::Guard::<Drop>::drop",
        "rgfx::P::<Display>::fmt",
        "rgfx::bump",
        "rgfx::cleanup",
        "rgfx::helper_in_macro",
        "rgfx::Unused::<Drop>::drop",
        "rgfx::L::<Local>::local_m",
    ] {
        assert!(found.contains(&id), "{id} missing from {found:?}");
    }
    // 같은 경로의 `From::from` 두 선언은 소스 위치 ID로 남는다(id 형식은 그대로다).
    assert_eq!(
        found
            .iter()
            .filter(|id| id.starts_with("rgfx::P::<From>::"))
            .count(),
        2
    );
    // next·drop·fmt·from 둘 — 닿지 않는 Unused의 drop은 세지 않는다.
    assert!(
        rep.limitations.iter().any(|l| l
            .starts_with("5 unreachable methods implement traits declared outside the workspace")
            && l.contains("--retain-external-trait-impls")),
        "{:?}",
        rep.limitations
    );
}

/// 보존 선택지를 켜면 std 트레이트 impl 메서드와 그 안에서만 부르는 함수가 빠진다. 워크스페이스
/// 트레이트의 미사용 impl, 만들지 않는 타입의 `Drop`과 그것만 부르는 함수는 계속 보고된다.
#[test]
fn retaining_external_trait_impls_removes_the_cascade_only() {
    let krate = Crate::new(
        "retain",
        &[
            ("Cargo.toml", &manifest("rgfx")),
            ("src/main.rs", TRAIT_IMPLS),
        ],
    );
    let doc = symbols(krate.path());
    let rep = analysis::dead_with_options(&doc, retaining_external_trait_impls()).unwrap();
    let found = ids(&rep);
    for gone in [
        "rgfx::Counter::<Iterator>::next",
        "rgfx::Guard::<Drop>::drop",
        "rgfx::P::<Display>::fmt",
        "rgfx::bump",
        "rgfx::cleanup",
        "rgfx::helper_in_macro",
    ] {
        assert!(!found.contains(&gone), "{gone} still reported: {found:?}");
    }
    assert!(!found.iter().any(|id| id.starts_with("rgfx::P::<From>::")));
    for kept in [
        "rgfx::Unused",
        "rgfx::Unused::<Drop>::drop",
        "rgfx::only_from_unused_drop",
        "rgfx::L::<Local>::local_m",
        "rgfx::Local::local_m",
    ] {
        assert!(found.contains(&kept), "{kept} missing from {found:?}");
    }

    // CLI는 같은 판정이고, 기본 실행은 지금처럼 그 메서드를 보고한다.
    let (code, text) = run(
        krate.path(),
        &["dead", "--retain-external-trait-impls", "--format", "json"],
    );
    assert_eq!(code, Some(0));
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let cli: Vec<&str> = json["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["id"].as_str().unwrap())
        .collect();
    assert_eq!(cli, found);
    let (_, plain) = run(krate.path(), &["dead"]);
    assert!(plain.contains("unreachable method: rgfx::Guard::<Drop>::drop\n"));
    assert!(plain.contains(
        "limitation: 5 unreachable methods implement traits declared outside the workspace"
    ));

    // explain은 같은 도달 규칙으로 경로를 보인다.
    let (code, path) = run(
        krate.path(),
        &[
            "dead",
            "--retain-external-trait-impls",
            "--explain",
            "rgfx::cleanup",
        ],
    );
    assert_eq!(code, Some(0));
    assert_eq!(
        path,
        "rgfx::main -> rgfx::Guard -> rgfx::Guard::<Drop>::drop -> rgfx::cleanup\n"
    );

    // MCP `rustograph_dead`의 retainExternalTraitImpls도 같은 판정이다.
    let mut child = Command::new(env!("CARGO_BIN_EXE_rustograph"))
        .args(["mcp", "--dir"])
        .arg(krate.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let call = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "rustograph_dead", "arguments": {"retainExternalTraitImpls": true}},
    });
    {
        use std::io::Write;
        writeln!(child.stdin.take().unwrap(), "{call}").unwrap();
    }
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    let reply: serde_json::Value =
        serde_json::from_slice(output.stdout.split(|&b| b == b'\n').next().unwrap()).unwrap();
    let body: serde_json::Value =
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(body["findings"], json["findings"]);
    assert_eq!(body["limitations"], json["limitations"]);
}

/// main 없는 lib — 공개 아이템 넷과 비공개 미사용 하나.
const LIB: &str = r#"
pub struct Config;
pub fn parse() -> Config { Config }
pub fn render(_: &Config) {}
pub mod nested { pub fn helper() {} }
fn private_unused() {}
"#;

/// 루트가 없는 lib 크레이트에서 기본 `dead`는 지금처럼 공개 API를 전부 unreachable로 보고하고
/// `--strict`는 1이다. 대신 텍스트·JSON 보고서가 그 수와 `--retain-public`을 알린다. `--retain-public`
/// 실행에서는 공개 아이템이 루트라 보고서도 limitation도 조용하다(비공개 미사용만 남는다).
#[test]
fn library_without_roots_points_at_retain_public() {
    let krate = Crate::new(
        "lib",
        &[("Cargo.toml", &manifest("rglib")), ("src/lib.rs", LIB)],
    );
    let hint = "4 unreachable symbols are exported items of crates with no retention root (rglib); callers outside this workspace are not in the graph, so if these crates are libraries used elsewhere, --retain-public (MCP: retainPublic) treats exported symbols as roots — otherwise these items have no caller in the workspace";
    let (code, json) = run(krate.path(), &["dead", "--strict", "--format", "json"]);
    assert_eq!(code, Some(1));
    let json: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(json["roots"], serde_json::json!([]));
    assert_eq!(json["findings"].as_array().unwrap().len(), 5);
    assert!(
        json["limitations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l == hint),
        "{}",
        json["limitations"]
    );
    let (_, text) = run(krate.path(), &["dead"]);
    assert!(text.contains("5 unreachable\n"), "{text}");
    assert!(text.contains(&format!("limitation: {hint}\n")), "{text}");

    let (code, kept) = run(krate.path(), &["dead", "--retain-public"]);
    assert_eq!(code, Some(0));
    assert_eq!(
        kept.lines()
            .filter(|l| l.starts_with("unreachable "))
            .collect::<Vec<_>>(),
        ["unreachable fn: rglib::private_unused"]
    );
    assert!(!kept.contains("--retain-public"), "{kept}");
}

/// 별칭으로 구현한 워크스페이스 트레이트(`use t::Tr as Alias; impl Alias for S`)는 외부 트레이트가 아니다 —
/// 보존 선택지를 켜도 그 impl과 그것만 부르는 함수를 보고하고, 기본 보고서의 외부 impl 계수에도 넣지
/// 않는다. 닿는 타입의 std `Display` impl은 계속 보존·계수되고, 만들지 않는 타입의 `Drop`은 보고된다.
#[test]
fn aliased_workspace_trait_impl_is_not_retained() {
    let krate = Crate::new(
        "alias",
        &[
            ("Cargo.toml", &manifest("traits")),
            (
                "src/main.rs",
                r#"
mod t { pub trait Tr { fn run(&self); } }
use t::Tr as Alias;
pub struct S;
impl Alias for S { fn run(&self) { only_from_alias_impl() } }
fn only_from_alias_impl() {}

mod g { pub struct G; impl Drop for G { fn drop(&mut self) { super::only_from_drop() } } }
#[allow(unused_imports)]
use g::G;
fn only_from_drop() {}

pub struct Shown;
impl std::fmt::Display for Shown {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { fmt_helper(); write!(f, "x") }
}
fn fmt_helper() {}

fn main() { let _s = S; let _w = Shown; }
"#,
            ),
        ],
    );
    let doc = symbols(krate.path());
    let rep = analysis::dead(&doc).unwrap();
    assert!(
        rep.limitations.iter().any(|l| l
            .starts_with("1 unreachable methods implement traits declared outside the workspace")),
        "{:?}",
        rep.limitations
    );
    let kept = analysis::dead_with_options(&doc, retaining_external_trait_impls()).unwrap();
    assert_eq!(
        ids(&kept),
        [
            "traits::S::<Alias>::run",
            "traits::g::G",
            "traits::g::G::<Drop>::drop",
            "traits::only_from_alias_impl",
            "traits::only_from_drop",
            "traits::t::Tr::run",
        ]
    );
}
