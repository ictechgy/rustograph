//! 이름 팬아웃의 의존 폐포 시험용 워크스페이스 — tests/integration.rs(syn)와 tests/semantic.rs가
//! 같은 입력으로 두 수확 모드를 본다(`#[path]`로 싣는다).
//!
//! 멤버와 의존은 이렇다.
//! - `app`(lib): `dep`에 의존하고, `opt`에는 optional 의존(기본 feature가 켜지 않아 cargo의 resolve
//!   그래프에 없다), `testdep`에는 dev 의존이다. 트레이트 `Hook`을 선언한다.
//! - `other`(lib): `app`에 의존한다(역방향) — `app::Hook`을 구현하고, `dep`과 같은 이름의 고유 메서드
//!   `get`과 트레이트 선언 메서드 `spin`을 둔다. app은 other의 타입을 이름 붙일 수 없다.
//! - `tool`(lib `tool` + bin `tool_cli`): 의존이 없다 — bin이 같은 패키지 lib의 메서드를 부른다.
//!
//! 호출 수신자는 정의되지 않은 타입이라 syn은 물론 의미 해석도 이름 팬아웃으로 돌아간다. `app::tuned`는
//! 기본 feature가 끈 `#[cfg(feature)]` 함수라 의미 해석 엔진이 못 보고 syn 폴백으로 간다 — 두 경로가 같은
//! 규칙으로 좁히는지 함께 본다.

use std::path::{Path, PathBuf};

/// (워크스페이스 기준 경로, 내용) 목록.
const FILES: &[(&str, &str)] = &[
    (
        "Cargo.toml",
        "[workspace]\nmembers = [\"app\", \"dep\", \"opt\", \"other\", \"testdep\", \"tool\"]\n\
         resolver = \"2\"\n",
    ),
    (
        "app/Cargo.toml",
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\ndep = { path = \"../dep\" }\nopt = { path = \"../opt\", optional = true }\n\n\
         [dev-dependencies]\ntestdep = { path = \"../testdep\" }\n",
    ),
    (
        "app/src/lib.rs",
        "pub trait Hook {\n    fn fire(&self);\n}\n\n\
         pub fn calls(x: Unknown) {\n    x.get();\n    x.spin();\n}\n\n\
         pub fn hooks<H: Hook>(h: H) {\n    h.fire();\n}\n\n\
         #[cfg(feature = \"opt\")]\npub fn tuned(x: Unknown) {\n    x.tune();\n    x.get();\n}\n\n\
         #[cfg(test)]\nmod tests {\n    pub fn probed(x: super::Unknown) {\n        x.probe();\n    }\n}\n",
    ),
    (
        "dep/Cargo.toml",
        "[package]\nname = \"dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    ),
    (
        "dep/src/lib.rs",
        "pub struct D;\n\nimpl D {\n    pub fn get(&self) -> u8 {\n        1\n    }\n}\n",
    ),
    (
        "opt/Cargo.toml",
        "[package]\nname = \"opt\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    ),
    (
        "opt/src/lib.rs",
        "pub struct P;\n\nimpl P {\n    pub fn tune(&self) {}\n}\n",
    ),
    (
        "other/Cargo.toml",
        "[package]\nname = \"other\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [dependencies]\napp = { path = \"../app\" }\n",
    ),
    (
        "other/src/lib.rs",
        "pub struct O;\n\nimpl O {\n    pub fn get(&self) -> u8 {\n        2\n    }\n}\n\n\
         pub trait Spin {\n    fn spin(&self);\n}\n\n\
         impl app::Hook for O {\n    fn fire(&self) {}\n}\n",
    ),
    (
        "testdep/Cargo.toml",
        "[package]\nname = \"testdep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    ),
    (
        "testdep/src/lib.rs",
        "pub struct T;\n\nimpl T {\n    pub fn probe(&self) {}\n}\n",
    ),
    (
        "tool/Cargo.toml",
        "[package]\nname = \"tool\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
         [[bin]]\nname = \"tool_cli\"\npath = \"src/main.rs\"\n",
    ),
    (
        "tool/src/lib.rs",
        "pub struct K;\n\nimpl K {\n    pub fn knob(&self) {}\n}\n",
    ),
    (
        "tool/src/main.rs",
        "fn turn(k: Unknown) {\n    k.knob();\n    k.get();\n}\n\nfn main() {}\n",
    ),
];

/// 시험 워크스페이스를 임시 디렉터리 `rg-fanout-<tag>-<pid>`에 새로 쓰고 그 경로를 돌려준다.
/// 같은 테스트 바이너리의 다른 테스트와 겹치지 않게 `tag`를 테스트마다 다르게 준다.
pub fn write_workspace(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rg-fanout-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in FILES {
        write_file(&dir, path, text);
    }
    dir
}

/// 워크스페이스 기준 상대 경로에 파일을 쓴다(중간 디렉터리 포함).
fn write_file(dir: &Path, path: &str, text: &str) {
    let file = dir.join(path);
    std::fs::create_dir_all(file.parent().expect("workspace files have a parent")).unwrap();
    std::fs::write(file, text).unwrap();
}

/// 의존 폐포 밖으로 좁혀져 후보가 사라진 호출 수의 limitation — app `x.spin()`(other의 트레이트 선언
/// 메서드뿐)과 tool_cli `k.get()`(dep·other의 고유 메서드뿐) 두 곳이다.
pub const OUT_OF_REACH: &str = "2 method calls name only inherent or trait-declared methods of \
     crates the calling crate does not depend on; no fan-out edges were added";
