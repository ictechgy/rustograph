//! 부모·자식 모듈 사이의 순환(`super::` 호출 등)을 `cycles --exclude-nested-modules`로 뺄 수 있는지 —
//! 기본 출력은 그대로인지 — 실제 바이너리로 검증한다.

use std::path::{Path, PathBuf};
use std::process::Command;

/// 부모 `m`이 자식 `m::inner`를 부르고 자식이 `super::helper`를 부르는 중첩 순환과, 형제 모듈
/// `a`·`b` 사이의 순환이 하나씩 있는 패키지.
fn workspace(tag: &str, with_siblings: bool) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "rustograph-module-cycles-{tag}-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("src/m")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='modcyc'\nversion='0.1.0'\nedition='2021'\n[workspace]\n",
    )
    .unwrap();
    let siblings = if with_siblings {
        "mod a { pub fn fa() { crate::b::fb() } pub fn fa2() {} }\n\
         mod b { pub fn fb() { crate::a::fa2() } }\n"
    } else {
        ""
    };
    std::fs::write(
        root.join("src/main.rs"),
        format!("mod m;\n{siblings}fn main() {{ m::uses_child(); }}\n"),
    )
    .unwrap();
    std::fs::write(
        root.join("src/m.rs"),
        "pub mod inner;\npub fn helper() {}\npub fn uses_child() { inner::call(); }\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/m/inner.rs"),
        "pub fn call() { super::helper(); }\n",
    )
    .unwrap();
    root
}

fn run(dir: &Path, args: &[&str]) -> (Option<i32>, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_rustograph"))
        .args(args)
        .arg("--dir")
        .arg(dir)
        .output()
        .expect("rustograph binary must start");
    (
        out.status.code(),
        String::from_utf8(out.stdout).unwrap(),
        String::from_utf8(out.stderr).unwrap(),
    )
}

#[test]
fn nested_module_cycles_are_noted_and_excluded_only_on_request() {
    let dir = workspace("both", true);
    // 기본 출력은 두 순환을 그대로 보고하고, 텍스트에만 중첩 순환 수와 플래그를 적는다.
    let (code, text, _) = run(&dir, &["cycles", "--level", "module", "--strict"]);
    assert_eq!(code, Some(1));
    assert!(
        text.starts_with(
            "2 cycle(s)\n  cycle: modcyc::a -> modcyc::b\n  cycle: modcyc::m -> modcyc::m::inner\n"
        ),
        "{text}"
    );
    assert!(text.contains("  note: 1 of these cycles involve only a module and its own descendants (e.g. a child calling `super::`); --exclude-nested-modules leaves them out\n"), "{text}");
    let (_, json, _) = run(&dir, &["cycles", "--level", "module", "--format", "json"]);
    let json: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(json["cycles"].as_array().unwrap().len(), 2);
    assert!(json.get("excluded_nested").is_none());

    // 플래그를 주면 형제 순환만 남고, 뺀 수를 싣는다. 형제 순환이 남아 strict는 여전히 1이다.
    let flag = ["cycles", "--level", "module", "--exclude-nested-modules"];
    let (code, text, _) = run(&dir, &[&flag[..], &["--strict"]].concat());
    assert_eq!(code, Some(1));
    assert!(text.starts_with("1 cycle(s)\n  cycle: modcyc::a -> modcyc::b\n  note: 1 cycles involving only a module and its own descendants were excluded (--exclude-nested-modules)\n"), "{text}");
    let (_, json, _) = run(&dir, &[&flag[..], &["--format", "json"]].concat());
    let json: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        json["cycles"][0]["members"],
        serde_json::json!(["modcyc::a", "modcyc::b"])
    );
    assert_eq!(json["cycles"].as_array().unwrap().len(), 1);
    assert_eq!(json["excluded_nested"], 1);
    std::fs::remove_dir_all(&dir).unwrap();

    // 중첩 순환뿐이면 플래그를 준 strict 게이트가 통과한다.
    let dir = workspace("nested", false);
    let (code, text, _) = run(&dir, &[&flag[..], &["--strict"]].concat());
    assert_eq!(code, Some(0), "{text}");
    assert!(text.starts_with("0 cycle(s)\n"), "{text}");
    // 모듈 레벨이 아니면 쓰지 않는 플래그라 사용법 오류다.
    let (code, _, err) = run(
        &dir,
        &["cycles", "--level", "symbol", "--exclude-nested-modules"],
    );
    assert_eq!(code, Some(2));
    assert!(err.contains("--exclude-nested-modules"), "{err}");
    std::fs::remove_dir_all(&dir).unwrap();
}
