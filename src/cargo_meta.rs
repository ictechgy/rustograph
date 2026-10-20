//! `cargo metadata` 소비자 — 크레이트·워크스페이스 사실을 가져온다.
//!
//! cargo_metadata 크레이트를 쓰지 않고 JSON을 직접 파싱한다: 필요한 필드가
//! 적어 의존 하나를 아끼는 게 합리적이고, 스키마의 이 부분은 안정적이다.
//!
//! 툴체인 실행(`cargo metadata`, `rustc` 프로브)은 전부 [`toolchain_command`]를 지난다. cargo는
//! 작업 디렉터리와 그 조상의 `.cargo/config.toml`을, rustup 프록시는 rust-toolchain 파일을 읽으므로
//! 분석 대상 체크아웃 안에서 띄우면 그 체크아웃이 실행할 프로그램(래퍼, `build.rustc`, 툴체인
//! 디렉터리)을 고를 수 있었다. 작업 디렉터리는 바꾸지 않는다 — 저장소의 정당한 source
//! replacement·`[patch]`·레지스트리 설정이 그대로 적용돼야 cargo가 빌드하는 그래프와 같다.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// cargo 패키지 하나.
#[derive(Debug)]
pub struct Package {
    /// 크레이트 이름(`-`는 `_`로 정규화해 모듈 경로와 맞춘다).
    pub name: String,
    /// 패키지 버전 — 중복 버전 탐지에 쓴다.
    pub version: String,
    /// lib/bin 타깃의 엔트리 파일들.
    pub targets: Vec<Target>,
    /// 이 패키지가 워크스페이스 멤버인가.
    pub workspace_member: bool,
    /// proc-macro 크레이트인가 — 사용 증거가 derive/속성 경로라 호출
    /// 그래프에 안 잡힐 수 있어 deps 보고서가 약한 증거로 표시한다.
    /// targets에서 proc-macro 타깃을 걸러내기 전에 따로 잡아야 한다.
    pub proc_macro: bool,
    /// 선언된 optional 의존의 패키지 이름(정규화, 선언 순서). resolve 그래프(`dep_edges`)는 기본
    /// feature가 켜지 않은 optional 의존을 빼지만 수확은 `#[cfg(feature)]` 뒤 코드도 담으므로, 이름
    /// 팬아웃의 의존 폐포가 이 목록으로 그 의존까지 따른다.
    pub optional_deps: Vec<String>,
}

impl Package {
    /// 이 패키지의 그래프상 크레이트 정점 ID. 워크스페이스 멤버는
    /// lib/bin 타깃의 루트 모듈이 크레이트 정점을 겸하니 타깃 이름이
    /// ID다 — 패키지 이름은 `[lib] name`이 다르면 정점이 아니다.
    /// 외부 패키지는 이름이 곧 정점이다.
    pub fn crate_vertex(&self) -> String {
        self.targets
            .iter()
            .find(|t| t.kind == "lib")
            .or_else(|| self.targets.iter().find(|t| t.kind == "bin"))
            .map(|t| t.name.clone())
            .unwrap_or_else(|| self.name.clone())
    }
}

/// lib/bin 타깃 하나 — 모듈 트리의 루트가 된다.
#[derive(Debug)]
pub struct Target {
    /// 타깃 이름(= 루트 모듈 이름).
    pub name: String,
    /// "lib" | "bin" | "example" | "test" | "bench" ...
    pub kind: String,
    /// 엔트리 소스 파일.
    pub src: PathBuf,
}

/// 워크스페이스 의존 해석 결과 — `from → to` 크레이트 간선과 종류.
#[derive(Debug)]
pub struct DepEdge {
    pub from: String,
    pub to: String,
    /// ""(normal) | "dev" | "build".
    pub kind: String,
    /// 코드가 실제로 쓰는 라이브러리 이름 — `foo = { package = "real" }`의
    /// rename을 거친 이름이라 use 경로의 첫 세그먼트와 같다.
    pub lib_name: String,
}

/// 수확에 필요한 메타데이터 전부.
#[derive(Debug)]
pub struct Metadata {
    pub packages: Vec<Package>,
    /// 패키지 ID → 패키지 인덱스.
    pub by_id: BTreeMap<String, usize>,
    /// resolve의 크레이트 간선(resolve가 없으면 빈 Vec).
    pub dep_edges: Vec<DepEdge>,
    pub workspace_root: PathBuf,
    /// resolve가 없을 때 등의 실측 한계.
    pub limitations: Vec<String>,
}

#[derive(Deserialize)]
struct RawMetadata {
    packages: Vec<RawPackage>,
    resolve: Option<RawResolve>,
    workspace_members: Vec<String>,
    workspace_root: String,
}

#[derive(Deserialize)]
struct RawPackage {
    id: String,
    name: String,
    version: String,
    targets: Vec<RawTarget>,
    /// 선언된 의존(Cargo.toml 그대로) — optional 의존만 쓴다.
    #[serde(default)]
    dependencies: Vec<RawDeclaredDep>,
}

/// 패키지가 선언한 의존 하나 — `name`은 rename 전 패키지 이름이다.
#[derive(Deserialize)]
struct RawDeclaredDep {
    name: String,
    #[serde(default)]
    optional: bool,
}

#[derive(Deserialize)]
struct RawTarget {
    name: String,
    kind: Vec<String>,
    src_path: String,
}

#[derive(Deserialize)]
struct RawResolve {
    nodes: Vec<RawNode>,
}

#[derive(Deserialize)]
struct RawNode {
    id: String,
    deps: Vec<RawDep>,
}

#[derive(Deserialize)]
struct RawDep {
    pkg: String,
    /// 코드에서 보이는 라이브러리 이름 — rename을 반영한다.
    name: Option<String>,
    dep_kinds: Vec<RawDepKind>,
}

#[derive(Deserialize)]
struct RawDepKind {
    kind: Option<String>,
}

/// 크레이트 이름을 모듈 경로용으로 정규화한다 — `my-crate`는 Rust에서 `my_crate`다.
pub fn normalize_name(name: &str) -> String {
    name.replace('-', "_")
}

/// `rustc --print cfg --target`의 원시 출력 — 타깃의 권위 있는 cfg
/// 팩트다. rustc가 없거나 트리플을 모르면 None — 호출자가 트리플
/// 추정으로 폴백한다. 반환값은 해석하지 않은 줄 목록이다 — 파싱은
/// 순수 도메인(cfgeval)의 일이라 여기선 그대로 넘긴다.
pub fn rustc_cfg_lines(triple: &str) -> Option<Vec<String>> {
    // 툴체인을 안전하게 정하지 못했으면 rustc를 띄우지 않는다 — rustc가 없을 때와 같은 None이라
    // 호출자가 트리플 추정으로 폴백한다(추정이 모르는 조건은 미지로 남아 limitation으로 센다).
    let out = toolchain_command(Tool::Rustc)
        .ok()?
        .args(["--print", "cfg", "--target", triple])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .collect(),
    )
}

/// `rustc -vV`의 원시 출력 — 툴체인 버전은 수확 결과를 바꾸는
/// 빌드 입력이라 캐시 지문에 넣는다. rustc가 없으면 None — 호출자는
/// 지문을 만들 수 없으니 캐시를 쓰지 않는다.
pub fn rustc_version() -> Option<String> {
    // 툴체인을 정하지 못했으면 rustc가 없을 때처럼 None이다 — 캐시 없이 수확한다.
    let out = toolchain_command(Tool::Rustc)
        .ok()?
        .arg("-vV")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

#[cfg(test)]
thread_local! {
    /// 이 스레드가 `cargo metadata`를 실행한 횟수 — 한 보고서가 메타데이터를 두 번 읽지
    /// 않는지 단위 테스트가 센다(테스트 스레드마다 따로).
    static LOADS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// 테스트용 — 이 스레드에서 지금까지 `cargo metadata`를 실행한 횟수.
#[cfg(test)]
pub(crate) fn loads() -> usize {
    LOADS.with(std::cell::Cell::get)
}

/// `cargo metadata`를 실행해 워크스페이스 사실을 읽는다.
/// dir이 cargo 프로젝트가 아니면 오류 — 빈 그래프로 속이지 않는다.
/// 툴체인은 분석 대상 설정이 고르지 못하게 정한 것을 쓴다([`toolchain_command`]). 그 때문에 작업
/// 디렉터리의 rust-toolchain 파일을 쓰지 않았으면 `limitations`에 그 사실을 더한다.
pub fn load(dir: &Path) -> Result<Metadata, String> {
    #[cfg(test)]
    LOADS.with(|c| c.set(c.get() + 1));
    let toolchain =
        process_toolchain().map_err(|e| format!("failed to run cargo metadata: {e}"))?;
    let out = toolchain
        .command_for(Tool::Cargo)
        .map_err(|e| format!("failed to run cargo metadata: {e}"))?
        .args(["metadata", "--format-version", "1", "--manifest-path"])
        .arg(dir.join("Cargo.toml"))
        .output()
        .map_err(|e| format!("failed to run cargo metadata: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "cargo metadata failed in {}: {}",
            dir.display(),
            stderr.trim()
        ));
    }
    let mut metadata = parse(&out.stdout)?;
    if toolchain.ignored_toolchain_file {
        metadata
            .limitations
            .push(TOOLCHAIN_FILE_IGNORED.to_string());
    }
    Ok(metadata)
}

/// `cargo metadata`가 resolve 그래프를 주지 않았다는 limitation — 의존 간선을 모른다는 뜻이라, 의존
/// 폐포로 후보를 좁히는 이름 팬아웃이 이 문장으로 좁히기를 끈다(모르는 의존을 없다고 보지 않는다).
pub(crate) const NO_RESOLVE_GRAPH: &str =
    "cargo metadata returned no resolve graph; dependency edges unavailable";

/// 작업 디렉터리(와 그 조상)의 rust-toolchain 파일을 쓰지 않았다는 limitation — 경로는 싣지 않는다
/// (문서가 다른 머신으로 옮겨진다). 해결 방향은 툴체인을 직접 고르는 것이다.
const TOOLCHAIN_FILE_IGNORED: &str = "toolchain-file-ignored: a rust-toolchain file in the \
     working directory or one of its parents was not used; cargo metadata ran with the toolchain \
     rustup picks where no such file exists (set RUSTUP_TOOLCHAIN=<channel> to choose the \
     toolchain)";

/// 툴체인 명령의 종류 — [`toolchain_command`]가 실행 파일과 환경을 고른다.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tool {
    Cargo,
    Rustc,
}

/// 분석 대상 설정과 무관하게 정한 툴체인 — 프로세스마다 한 번 정하고 모든 cargo·rustc 실행이 쓴다.
#[derive(Debug, PartialEq, Eq)]
struct Toolchain {
    /// PATH의 절대 경로 항목에서 찾은 cargo — 없으면 None(실행할 때 원인을 알린다).
    cargo: Option<PathBuf>,
    /// cargo가 쓰고 rustograph의 프로브도 쓰는 rustc — 사용자가 준 `RUSTC`(준 그대로) 또는 분석 대상
    /// 밖에서 정한 절대 경로. 없으면 None.
    rustc: Option<OsString>,
    /// 사용자가 `RUSTUP_TOOLCHAIN`을 주지 않아 rustograph가 고정한 툴체인 이름 — rustup이 없거나
    /// 사용자가 줬으면 None이다(사용자 값은 환경에서 그대로 물려받는다).
    pinned_toolchain: Option<String>,
    /// 고정 때문에 작업 디렉터리(와 그 조상)의 rust-toolchain 파일을 쓰지 않았는가.
    ignored_toolchain_file: bool,
}

impl Toolchain {
    /// `tool`을 띄울 명령. 래퍼 두 변수는 항상 빈 값이다 — 빈 값이 설정 파일의
    /// `build.rustc-wrapper`·`build.rustc-workspace-wrapper`를 끈다(cargo 문서 environment-variables,
    /// 그리고 직접 확인). `RUSTC`는 `build.rustc`보다 앞서고, 고정한 `RUSTUP_TOOLCHAIN`은 rustup
    /// 프록시가 rust-toolchain 파일을 찾지 않게 한다(rustup의 override 순서: 환경 변수가 파일보다 앞선다).
    /// cargo도 rustc가 정해져야 띄운다 — `RUSTC`가 없으면 cargo가 설정 파일의 `build.rustc`로 돌아간다.
    fn command_for(&self, tool: Tool) -> Result<Command, String> {
        let rustc = self.rustc.as_deref().ok_or_else(|| not_on_path("rustc"))?;
        let program: &OsStr = match tool {
            Tool::Cargo => self
                .cargo
                .as_deref()
                .map(Path::as_os_str)
                .ok_or_else(|| not_on_path("cargo"))?,
            Tool::Rustc => rustc,
        };
        let mut command = Command::new(program);
        command
            .env("RUSTC_WRAPPER", "")
            .env("RUSTC_WORKSPACE_WRAPPER", "")
            .env("RUSTC", rustc);
        if let Some(name) = &self.pinned_toolchain {
            command.env("RUSTUP_TOOLCHAIN", name);
        }
        Ok(command)
    }
}

/// 툴체인 프로그램을 찾지 못한 오류 — 절대 경로 항목만 본다는 사실과 해결 방향을 싣는다.
fn not_on_path(tool: &str) -> String {
    format!(
        "{tool} was not found in an absolute PATH entry (empty and relative entries such as `.` \
         are skipped); install Rust or add the directory that holds {tool} to PATH"
    )
}

/// cargo·rustc를 분석 대상 설정이 고른 프로그램 없이 띄울 명령 — 툴체인 실행은 모두 이 함수를
/// 지난다. 호출마다 따로 만들면 한쪽만 래퍼를 끄는 어긋남이 생긴다.
fn toolchain_command(tool: Tool) -> Result<Command, String> {
    process_toolchain()?.command_for(tool)
}

/// 이 프로세스의 툴체인 — 처음 쓸 때 한 번 정한다. MCP 서버처럼 오래 사는 프로세스도 같은 값을
/// 쓰고, rustup 질의(프로세스 한두 개, 수 ms)는 한 번만 든다.
static PROCESS_TOOLCHAIN: OnceLock<Result<Toolchain, String>> = OnceLock::new();

fn process_toolchain() -> Result<&'static Toolchain, String> {
    PROCESS_TOOLCHAIN
        .get_or_init(|| resolve_toolchain(&ToolchainInputs::from_env()))
        .as_ref()
        .map_err(String::clone)
}

/// 툴체인 해석의 입력 — 프로세스 환경을 한 번 읽은 값이다. 테스트는 가짜 PATH와 임시 디렉터리를 준다.
struct ToolchainInputs {
    /// `PATH` 값(None은 PATH 없음).
    path: Option<OsString>,
    /// 사용자가 준 `RUSTUP_TOOLCHAIN` — 빈 값은 없음으로 본다(rustup도 그렇게 읽는다).
    rustup_toolchain: Option<OsString>,
    /// 사용자가 준 `RUSTC` — 빈 값은 없음으로 본다.
    rustc: Option<OsString>,
    /// rustograph의 작업 디렉터리 — rustup 프록시가 rust-toolchain 파일을 찾기 시작했을 곳이다.
    working_dir: Option<PathBuf>,
    /// rustup에 물을 중립 디렉터리를 만들 곳(`std::env::temp_dir()`).
    scratch: PathBuf,
}

impl ToolchainInputs {
    fn from_env() -> ToolchainInputs {
        ToolchainInputs {
            path: std::env::var_os("PATH"),
            rustup_toolchain: non_empty_var("RUSTUP_TOOLCHAIN"),
            rustc: non_empty_var("RUSTC"),
            // 작업 디렉터리를 모르면 rustup도 툴체인 파일을 찾지 못한다 — 무시한 파일이 없다고 본다.
            working_dir: std::env::current_dir().ok(),
            scratch: std::env::temp_dir(),
        }
    }
}

fn non_empty_var(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|value| !value.is_empty())
}

/// 툴체인을 분석 대상 밖에서 정한다. cargo·rustc는 PATH의 절대 경로 항목에서 찾는다. 그것이 rustup
/// 프록시가 아니면(Homebrew·배포판 툴체인) 찾은 rustc를 그대로 고정한다. 프록시면 사용자가 주지 않은
/// 값만 전용 중립 디렉터리에서 rustup에 묻는다: `RUSTUP_TOOLCHAIN`이 없으면 `rustup show active-toolchain`,
/// `RUSTC`가 없으면 그 툴체인의 `rustup which rustc`. 두 변수를 따로 보는 것은 `RUSTUP_TOOLCHAIN`만 준
/// 사용자에게도 `build.rustc`가 남지 않게 하려는 것이다.
fn resolve_toolchain(inputs: &ToolchainInputs) -> Result<Toolchain, String> {
    let path = inputs.path.as_deref();
    let cargo = find_program(path, "cargo", is_executable_file);
    let rustc_on_path = find_program(path, "rustc", is_executable_file);
    let Some(rustup) = cargo
        .iter()
        .chain(&rustc_on_path)
        .find_map(|program| rustup_behind(program))
    else {
        return Ok(Toolchain {
            rustc: inputs
                .rustc
                .clone()
                .or_else(|| rustc_on_path.map(PathBuf::into_os_string)),
            cargo,
            pinned_toolchain: None,
            ignored_toolchain_file: false,
        });
    };
    let rustc_is_proxy = rustc_on_path
        .as_deref()
        .is_none_or(|program| rustup_behind(program).is_some());
    let ask_rustc = inputs.rustc.is_none() && rustc_is_proxy;
    let (pinned_toolchain, asked_rustc) = with_neutral_dir(&inputs.scratch, |dir| {
        ask_rustup(&rustup, dir, inputs.rustup_toolchain.as_deref(), ask_rustc)
    })?;
    let rustc = inputs
        .rustc
        .clone()
        .or_else(|| asked_rustc.map(PathBuf::into_os_string))
        .or_else(|| rustc_on_path.map(PathBuf::into_os_string));
    let ignored_toolchain_file = pinned_toolchain.is_some() && has_toolchain_file(inputs);
    Ok(Toolchain {
        cargo,
        rustc,
        pinned_toolchain,
        ignored_toolchain_file,
    })
}

/// 작업 디렉터리(와 그 조상)에 rustup 프록시가 읽었을 rust-toolchain 파일이 있는가. 확인하지 못한
/// 항목이 있으면 없다고 본다 — 이 값은 알림(limitation)일 뿐이고 툴체인은 이미 고정했다.
fn has_toolchain_file(inputs: &ToolchainInputs) -> bool {
    inputs
        .working_dir
        .as_deref()
        .is_some_and(|dir| matches!(toolchain_file_from(dir), Ok(Some(_))))
}

/// 중립 디렉터리 `dir`에서 rustup에 묻는다 — 고정할 툴체인 이름(사용자가 줬으면 None)과, `ask_rustc`면
/// 그 툴체인(사용자 값 또는 고정값)의 rustc 절대 경로.
fn ask_rustup(
    rustup: &Path,
    dir: &Path,
    user_toolchain: Option<&OsStr>,
    ask_rustc: bool,
) -> Result<(Option<String>, Option<PathBuf>), String> {
    let (pinned, chosen) = match user_toolchain {
        Some(user) => (None, user.to_os_string()),
        None => {
            let name = active_toolchain(rustup, dir)?;
            (Some(name.clone()), OsString::from(name))
        }
    };
    let rustc = if ask_rustc {
        Some(which_rustc(rustup, dir, &chosen)?)
    } else {
        None
    };
    Ok((pinned, rustc))
}

/// 중립 디렉터리의 활성 툴체인 이름 — 툴체인 파일이 없는 곳이라 사용자의 기본 툴체인이다.
fn active_toolchain(rustup: &Path, dir: &Path) -> Result<String, String> {
    let mut command = rustup_command(rustup, dir);
    command
        .env_remove("RUSTUP_TOOLCHAIN")
        .args(["show", "active-toolchain"]);
    parse_active_toolchain(&rustup_stdout(command, "show active-toolchain")?)
}

/// `toolchain`의 rustc 절대 경로(`rustup which rustc`) — 프록시를 거치지 않는 실제 컴파일러다.
fn which_rustc(rustup: &Path, dir: &Path, toolchain: &OsStr) -> Result<PathBuf, String> {
    let mut command = rustup_command(rustup, dir);
    command
        .env("RUSTUP_TOOLCHAIN", toolchain)
        .args(["which", "rustc"]);
    let printed = rustup_stdout(command, "which rustc")?;
    let rustc = PathBuf::from(printed.trim_end_matches(['\r', '\n']));
    if rustc.is_absolute() {
        Ok(rustc)
    } else {
        Err(format!(
            "`rustup which rustc` printed {printed:?}, not an absolute path; set RUSTC to the \
             rustc that rustograph should run"
        ))
    }
}

/// 중립 디렉터리에서 도는 rustup 명령 — 입력을 막아 설치 확인 같은 질문에 멈추지 않게 한다.
fn rustup_command(rustup: &Path, dir: &Path) -> Command {
    let mut command = Command::new(rustup);
    command.current_dir(dir).stdin(Stdio::null());
    command
}

/// rustup 명령의 표준 출력 — 실패하면 원인(rustup의 오류)과 해결 방향(두 변수를 직접 준다)을 싣는다.
fn rustup_stdout(mut command: Command, what: &str) -> Result<String, String> {
    let advice = "set RUSTUP_TOOLCHAIN (for example RUSTUP_TOOLCHAIN=stable) and RUSTC to the \
                  toolchain that rustograph should run";
    let out = command
        .output()
        .map_err(|e| format!("failed to run `rustup {what}`: {e}; {advice}"))?;
    if !out.status.success() {
        return Err(format!(
            "`rustup {what}` failed: {}; {advice}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    String::from_utf8(out.stdout)
        .map_err(|_| format!("`rustup {what}` printed text that is not UTF-8; {advice}"))
}

/// `rustup show active-toolchain` 출력(`stable-aarch64-apple-darwin (default)`)의 첫 낱말이 이름이다.
/// 구분자가 든 값(경로 툴체인)·`.`·`..`·제어 문자는 받지 않는다 — 중립 디렉터리의 활성 툴체인은
/// 이름으로 설치된 기본 툴체인이어야 하고, 경로 툴체인은 rust-toolchain 파일이 고른 것이다.
fn parse_active_toolchain(printed: &str) -> Result<String, String> {
    let name = printed.split_whitespace().next().unwrap_or_default();
    let plain = !matches!(name, "" | "." | "..")
        && !name.contains(['/', '\\'])
        && !name.chars().any(char::is_control);
    if plain {
        Ok(name.to_string())
    } else {
        Err(format!(
            "rustup reported the active toolchain as {name:?}, which is not a toolchain name; set \
             RUSTUP_TOOLCHAIN=<channel> to choose the toolchain"
        ))
    }
}

/// `scratch` 아래 새 빈 디렉터리에서 `run`을 부르고 지운다. rustup은 작업 디렉터리에서 위로 올라가며
/// rust-toolchain 파일을 찾으므로, 그 조상에 그런 파일이 있으면(공유 임시 디렉터리라면 다른 사용자가
/// 둔 것일 수 있다) 묻지 않고 거부한다. 지우지 못한 것도 오류다 — 빈 채로 만든 디렉터리가 비지
/// 않았다는 뜻이라 그 사이의 답을 믿지 않는다.
fn with_neutral_dir<T>(
    scratch: &Path,
    run: impl FnOnce(&Path) -> Result<T, String>,
) -> Result<T, String> {
    let dir = create_neutral_dir(scratch)?;
    let answer = refuse_toolchain_file_above(&dir).and_then(|()| run(&dir));
    let removed = std::fs::remove_dir(&dir).map_err(|e| {
        format!(
            "could not remove {} after resolving the Rust toolchain in it: {e}; remove it and \
             run again",
            dir.display()
        )
    });
    match (answer, removed) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(e), Ok(())) | (Ok(_), Err(e)) => Err(e),
        (Err(e), Err(cleanup)) => Err(format!("{e} (and {cleanup})")),
    }
}

/// 중립 디렉터리 이름을 고르는 시도 횟수 — 같은 pid의 옛 프로세스가 남긴 이름을 건너뛸 만큼이면 된다.
const NEUTRAL_DIR_ATTEMPTS: u32 = 16;

/// 이 프로세스 전용의 새 디렉터리 — 이미 있는 이름은 쓰지 않는다(남이 미리 만든 디렉터리에서 묻지 않게).
fn create_neutral_dir(scratch: &Path) -> Result<PathBuf, String> {
    for attempt in 0..NEUTRAL_DIR_ATTEMPTS {
        let dir = scratch.join(format!(
            "rustograph-toolchain-{}-{attempt}",
            std::process::id()
        ));
        match create_private_dir(&dir) {
            Ok(()) => return Ok(dir),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => {
                return Err(format!(
                    "could not create {} to resolve the Rust toolchain in: {e}; point TMPDIR at a \
                     writable directory or set RUSTUP_TOOLCHAIN and RUSTC",
                    dir.display()
                ))
            }
        }
    }
    Err(format!(
        "could not create a new directory under {} to resolve the Rust toolchain in; remove \
         leftover rustograph-toolchain-* directories there or set RUSTUP_TOOLCHAIN and RUSTC",
        scratch.display()
    ))
}

/// 소유자만 쓰는 새 디렉터리를 만든다(unix 0700) — 있으면 `AlreadyExists`다.
#[cfg(unix)]
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(dir)
}

/// 소유자만 쓰는 새 디렉터리를 만든다 — 있으면 `AlreadyExists`다.
#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir(dir)
}

/// 중립 디렉터리의 조상에 rustup이 읽을 rust-toolchain 파일이 있거나 확인할 수 없으면 오류다.
fn refuse_toolchain_file_above(dir: &Path) -> Result<(), String> {
    let advice = "point TMPDIR elsewhere or set RUSTUP_TOOLCHAIN and RUSTC explicitly";
    let real = dir
        .canonicalize()
        .map_err(|e| format!("could not resolve {}: {e}; {advice}", dir.display()))?;
    match toolchain_file_from(&real) {
        Ok(None) => Ok(()),
        Ok(Some(file)) => Err(format!(
            "{} would choose the toolchain rustup reports for {}; {advice}",
            file.display(),
            dir.display()
        )),
        Err(e) => Err(format!("{e}; {advice}")),
    }
}

/// rustup이 읽는 툴체인 파일 이름 — 한 디렉터리에서 이 순서로 본다.
const TOOLCHAIN_FILES: [&str; 2] = ["rust-toolchain", "rust-toolchain.toml"];

/// `start`와 그 조상에서 가장 가까운 툴체인 파일 — rustup처럼 가까운 디렉터리부터 본다. 링크·디렉터리도
/// 있는 것으로 센다(보수적). 확인하지 못한 항목(NotFound 밖의 오류)은 `Err`다.
fn toolchain_file_from(start: &Path) -> Result<Option<PathBuf>, String> {
    for dir in start.ancestors() {
        for name in TOOLCHAIN_FILES {
            let candidate = dir.join(name);
            match std::fs::symlink_metadata(&candidate) {
                Ok(_) => return Ok(Some(candidate)),
                // 없으면 다음 이름·다음 조상을 본다.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(format!("could not check {}: {e}", candidate.display())),
            }
        }
    }
    Ok(None)
}

/// `program`(PATH에서 찾은 cargo·rustc)이 rustup 프록시면 그 rustup의 경로. 프록시는 rustup을
/// 가리키는 링크이거나(`~/.cargo/bin/cargo -> rustup`) rustup과 같은 디렉터리의 하드 링크·복사본이다.
/// 프록시가 아니면(Homebrew·배포판 rustc) None이다.
fn rustup_behind(program: &Path) -> Option<PathBuf> {
    let file = format!("rustup{}", std::env::consts::EXE_SUFFIX);
    if let Ok(real) = program.canonicalize() {
        if real.file_name() == Some(OsStr::new(&file)) {
            return Some(real);
        }
    }
    let sibling = program.with_file_name(&file);
    is_executable_file(&sibling).then_some(sibling)
}

/// PATH가 없을 때 툴체인 프로그램을 찾는 디렉터리 — 이름만 넘긴 `Command::new`가 쓰던 libc 기본 탐색
/// 경로(macOS `_PATH_DEFPATH`는 `/usr/bin:/bin`)와 같은 두 디렉터리다. Windows에는 기본값이 없다.
#[cfg(unix)]
const TOOL_SEARCH_PATH: Option<&str> = Some("/bin:/usr/bin");
#[cfg(not(unix))]
const TOOL_SEARCH_PATH: Option<&str> = None;

/// PATH 값(`None`은 PATH 없음)에서 `name` 실행 파일을 찾는다 — 셸처럼 앞 항목이 이긴다. 빈 항목과
/// `.`·`bin` 같은 상대 항목은 건너뛴다: 이름만 넘긴 `Command::new`는 그런 항목을 rustograph의 작업
/// 디렉터리 기준으로 풀어, 체크아웃 안에서 띄우면 체크아웃에 든 `cargo`를 실행한다(직접 확인했다).
/// `is_executable`은 후보 검사다(테스트가 파일 시스템 대신 주입한다).
fn find_program(
    path: Option<&OsStr>,
    name: &str,
    is_executable: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let path = path.or(TOOL_SEARCH_PATH.map(OsStr::new))?;
    let file = format!("{name}{}", std::env::consts::EXE_SUFFIX);
    std::env::split_paths(path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(&file))
        .find(|candidate| is_executable(candidate))
}

/// 실행할 수 있는 일반 파일인가(링크는 따라간다) — unix는 실행 비트까지 본다. 없거나 읽을 수 없는
/// 후보는 execvp처럼 건너뛴다(오류가 아니라 다음 PATH 항목을 볼 이유다).
fn is_executable_file(candidate: &Path) -> bool {
    let Ok(found) = std::fs::metadata(candidate) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        found.is_file() && found.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        found.is_file()
    }
}

fn parse(bytes: &[u8]) -> Result<Metadata, String> {
    let raw: RawMetadata =
        serde_json::from_slice(bytes).map_err(|e| format!("bad cargo metadata JSON: {e}"))?;
    let members: std::collections::BTreeSet<&str> =
        raw.workspace_members.iter().map(|s| s.as_str()).collect();
    // deps[].name이 없을 때의 폴백용 — packages를 소비하기 전에 뺀다.
    let raw_names: BTreeMap<String, String> = raw
        .packages
        .iter()
        .map(|p| (p.id.clone(), p.name.clone()))
        .collect();
    let mut by_id = BTreeMap::new();
    let mut packages = Vec::new();
    for p in raw.packages {
        by_id.insert(p.id.clone(), packages.len());
        // proc-macro 타깃은 아래 필터에서 버려진다 — 걸러내기 전에
        // 패키지 수준 플래그로 따로 잡는다.
        let proc_macro = p
            .targets
            .iter()
            .any(|t| t.kind.iter().any(|k| k == "proc-macro"));
        let optional_deps = p
            .dependencies
            .iter()
            .filter(|d| d.optional)
            .map(|d| normalize_name(&d.name))
            .collect();
        packages.push(Package {
            workspace_member: members.contains(p.id.as_str()),
            name: normalize_name(&p.name),
            version: p.version,
            proc_macro,
            optional_deps,
            targets: p
                .targets
                .into_iter()
                .filter(|t| {
                    t.kind
                        .iter()
                        .any(|k| matches!(k.as_str(), "lib" | "bin" | "example" | "test" | "bench"))
                })
                .map(|t| Target {
                    name: normalize_name(&t.name),
                    kind: t.kind.first().cloned().unwrap_or_default(),
                    src: PathBuf::from(t.src_path),
                })
                .collect(),
        });
    }
    let mut dep_edges = Vec::new();
    let mut limitations = Vec::new();
    match raw.resolve {
        Some(resolve) => {
            for n in resolve.nodes {
                for d in n.deps {
                    // deps[].name이 없으면 패키지 이름이 곧 lib 이름이다.
                    let lib = d
                        .name
                        .clone()
                        .or_else(|| raw_names.get(&d.pkg).cloned())
                        .unwrap_or_default();
                    for k in d.dep_kinds {
                        dep_edges.push(DepEdge {
                            from: n.id.clone(),
                            to: d.pkg.clone(),
                            kind: k.kind.unwrap_or_default(),
                            lib_name: normalize_name(&lib),
                        });
                    }
                }
            }
        }
        None => limitations.push(NO_RESOLVE_GRAPH.to_string()),
    }
    Ok(Metadata {
        packages,
        by_id,
        dep_edges,
        workspace_root: PathBuf::from(raw.workspace_root),
        limitations,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 테스트 전용 빈 디렉터리(실패한 실행이 남긴 것은 먼저 지운다).
    fn scratch_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rg-cargo-meta-{tag}-{}", std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn find_program_skips_empty_and_relative_path_entries() {
        let present = |candidate: &Path| {
            [
                Path::new("cargo"),
                Path::new("./cargo"),
                Path::new("bin/cargo"),
                Path::new("/second/cargo"),
            ]
            .iter()
            .any(|known| candidate == known.with_extension(std::env::consts::EXE_EXTENSION))
        };
        let path = std::env::join_paths(["", ".", "bin", "/first", "/second"]).unwrap();
        assert_eq!(
            find_program(Some(&path), "cargo", present),
            Some(PathBuf::from("/second/cargo").with_extension(std::env::consts::EXE_EXTENSION))
        );
        let relative_only = std::env::join_paths(["", ".", "bin"]).unwrap();
        assert_eq!(find_program(Some(&relative_only), "cargo", present), None);
    }

    #[cfg(unix)]
    #[test]
    fn find_program_without_path_looks_in_the_default_directories() {
        let seen = std::cell::RefCell::new(Vec::new());
        let found = find_program(None, "rustc", |candidate: &Path| {
            seen.borrow_mut().push(candidate.to_path_buf());
            false
        });
        assert_eq!(found, None);
        assert_eq!(
            seen.into_inner(),
            [PathBuf::from("/bin/rustc"), PathBuf::from("/usr/bin/rustc")]
        );
    }

    #[test]
    fn parse_active_toolchain_reads_a_name_and_refuses_paths() {
        for (printed, name) in [
            (
                "stable-aarch64-apple-darwin (default)\n",
                "stable-aarch64-apple-darwin",
            ),
            (
                "nightly-2026-01-01 (overridden by environment variable RUSTUP_TOOLCHAIN)",
                "nightly-2026-01-01",
            ),
            ("my-linked-toolchain\n", "my-linked-toolchain"),
        ] {
            assert_eq!(parse_active_toolchain(printed), Ok(name.to_string()));
        }
        for printed in [
            "",
            "\n",
            "/home/someone/toolchain (overridden by '/tmp/rust-toolchain.toml')",
            "C:\\toolchain (default)",
            "..",
            ".",
            "bad\u{7}name (default)",
        ] {
            let refused = parse_active_toolchain(printed).unwrap_err();
            assert!(
                refused.contains("set RUSTUP_TOOLCHAIN"),
                "{printed:?}: {refused}"
            );
        }
    }

    #[test]
    fn toolchain_commands_always_turn_off_the_configured_wrappers() {
        let envs = |command: &Command| -> Vec<(String, Option<String>)> {
            command
                .get_envs()
                .map(|(k, v)| {
                    (
                        k.to_string_lossy().into_owned(),
                        v.map(|v| v.to_string_lossy().into_owned()),
                    )
                })
                .collect()
        };
        let pinned = Toolchain {
            cargo: Some(PathBuf::from("/tools/cargo")),
            rustc: Some(OsString::from("/tools/rustc")),
            pinned_toolchain: Some("stable".to_string()),
            ignored_toolchain_file: false,
        };
        for tool in [Tool::Cargo, Tool::Rustc] {
            let command = pinned.command_for(tool).unwrap();
            let mut got = envs(&command);
            got.sort();
            assert_eq!(
                got,
                [
                    ("RUSTC".to_string(), Some("/tools/rustc".to_string())),
                    ("RUSTC_WORKSPACE_WRAPPER".to_string(), Some(String::new())),
                    ("RUSTC_WRAPPER".to_string(), Some(String::new())),
                    ("RUSTUP_TOOLCHAIN".to_string(), Some("stable".to_string())),
                ]
            );
        }
        assert_eq!(
            pinned.command_for(Tool::Cargo).unwrap().get_program(),
            OsStr::new("/tools/cargo")
        );
        assert_eq!(
            pinned.command_for(Tool::Rustc).unwrap().get_program(),
            OsStr::new("/tools/rustc")
        );
        // rustup이 없는 툴체인은 RUSTUP_TOOLCHAIN을 주지 않는다 — 사용자 값이 있으면 그대로 물려받는다.
        let unpinned = Toolchain {
            pinned_toolchain: None,
            ..pinned
        };
        let mut got = envs(&unpinned.command_for(Tool::Cargo).unwrap());
        got.sort();
        assert_eq!(
            got,
            [
                ("RUSTC".to_string(), Some("/tools/rustc".to_string())),
                ("RUSTC_WORKSPACE_WRAPPER".to_string(), Some(String::new())),
                ("RUSTC_WRAPPER".to_string(), Some(String::new())),
            ]
        );
        // rustc를 정하지 못하면 cargo도 띄우지 않는다 — RUSTC 없이는 cargo가 `build.rustc`를 따른다.
        let without_rustc = Toolchain {
            rustc: None,
            ..unpinned
        };
        for tool in [Tool::Cargo, Tool::Rustc] {
            let missing = without_rustc.command_for(tool).unwrap_err();
            assert!(
                missing.contains("rustc was not found in an absolute PATH entry"),
                "{tool:?}: {missing}"
            );
        }
    }

    /// 가짜 rustup과 그 프록시가 든 bin 디렉터리 — rustup은 호출마다 `작업 디렉터리|RUSTUP_TOOLCHAIN|인자`를
    /// 기록하고 `show active-toolchain`에 `active`, `which rustc`에 `which`를 답한다.
    #[cfg(unix)]
    struct FakeRustup {
        root: PathBuf,
        bin: PathBuf,
        log: PathBuf,
    }

    #[cfg(unix)]
    impl FakeRustup {
        fn new(tag: &str, active: &str, which: &str) -> FakeRustup {
            use std::os::unix::fs::PermissionsExt;
            let root = scratch_dir(tag).canonicalize().unwrap();
            let bin = root.join("bin");
            std::fs::create_dir_all(&bin).unwrap();
            let log = root.join("rustup.log");
            let script = format!(
                "#!/bin/sh\necho \"$(pwd -P)|${{RUSTUP_TOOLCHAIN-<unset>}}|$*\" >> '{}'\n\
                 case \"$1 $2\" in\n\
                 \"show active-toolchain\") printf '%s\\n' '{active}' ;;\n\
                 \"which rustc\") printf '%s\\n' '{which}' ;;\n\
                 *) exit 2 ;;\nesac\n",
                log.display()
            );
            std::fs::write(bin.join("rustup"), script).unwrap();
            std::fs::set_permissions(bin.join("rustup"), std::fs::Permissions::from_mode(0o755))
                .unwrap();
            for proxy in ["cargo", "rustc"] {
                std::os::unix::fs::symlink("rustup", bin.join(proxy)).unwrap();
            }
            std::fs::create_dir_all(root.join("scratch")).unwrap();
            std::fs::create_dir_all(root.join("checkout")).unwrap();
            FakeRustup { root, bin, log }
        }

        fn inputs(&self) -> ToolchainInputs {
            ToolchainInputs {
                path: Some(self.bin.clone().into_os_string()),
                rustup_toolchain: None,
                rustc: None,
                working_dir: Some(self.root.join("checkout")),
                scratch: self.root.join("scratch"),
            }
        }

        /// 기록된 rustup 호출(`작업 디렉터리|RUSTUP_TOOLCHAIN|인자`).
        fn rustup_calls(&self) -> Vec<String> {
            match std::fs::read_to_string(&self.log) {
                Ok(text) => text.lines().map(str::to_string).collect(),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(e) => panic!("cannot read {}: {e}", self.log.display()),
            }
        }

        /// 중립 디렉터리가 남지 않았는가 — scratch가 비어 있어야 한다.
        fn scratch_is_empty(&self) -> bool {
            std::fs::read_dir(self.root.join("scratch"))
                .unwrap()
                .next()
                .is_none()
        }
    }

    #[cfg(unix)]
    impl Drop for FakeRustup {
        fn drop(&mut self) {
            if let Err(e) = std::fs::remove_dir_all(&self.root) {
                eprintln!("could not remove {}: {e}", self.root.display());
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn rustup_proxies_are_resolved_outside_the_working_directory() {
        let fake = FakeRustup::new(
            "proxy",
            "fake-stable (default)",
            "/toolchains/fake/bin/rustc",
        );
        std::fs::write(
            fake.root.join("checkout/rust-toolchain.toml"),
            "[toolchain]\n",
        )
        .unwrap();
        let resolved = resolve_toolchain(&fake.inputs()).unwrap();
        assert_eq!(
            resolved,
            Toolchain {
                cargo: Some(fake.bin.join("cargo")),
                rustc: Some(OsString::from("/toolchains/fake/bin/rustc")),
                pinned_toolchain: Some("fake-stable".to_string()),
                ignored_toolchain_file: true,
            }
        );
        let neutral = fake.root.join("scratch");
        let calls = fake.rustup_calls();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(
            calls[0].ends_with("|<unset>|show active-toolchain"),
            "{calls:?}"
        );
        assert!(calls[1].ends_with("|fake-stable|which rustc"), "{calls:?}");
        for call in &calls {
            assert!(
                call.starts_with(&format!("{}/rustograph-toolchain-", neutral.display())),
                "rustup must run in a neutral directory: {call}"
            );
        }
        assert!(fake.scratch_is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn toolchain_choices_from_the_environment_are_kept() {
        let fake = FakeRustup::new(
            "user",
            "fake-stable (default)",
            "/toolchains/fake/bin/rustc",
        );
        std::fs::write(
            fake.root.join("checkout/rust-toolchain.toml"),
            "[toolchain]\n",
        )
        .unwrap();
        // 사용자가 툴체인을 골랐으면 묻지 않고, 그 툴체인의 rustc만 찾는다 — 툴체인 파일은 사용자가 덮었다.
        let chosen = ToolchainInputs {
            rustup_toolchain: Some(OsString::from("mine")),
            ..fake.inputs()
        };
        let resolved = resolve_toolchain(&chosen).unwrap();
        assert_eq!(resolved.pinned_toolchain, None);
        assert!(!resolved.ignored_toolchain_file);
        assert_eq!(
            resolved.rustc,
            Some(OsString::from("/toolchains/fake/bin/rustc"))
        );
        let calls = fake.rustup_calls();
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert!(calls[0].ends_with("|mine|which rustc"), "{calls:?}");
        // 사용자가 rustc를 골랐으면 그대로 쓰고 툴체인만 고정한다.
        let given = ToolchainInputs {
            rustc: Some(OsString::from("/opt/my-rustc")),
            ..fake.inputs()
        };
        let resolved = resolve_toolchain(&given).unwrap();
        assert_eq!(resolved.rustc, Some(OsString::from("/opt/my-rustc")));
        assert_eq!(resolved.pinned_toolchain, Some("fake-stable".to_string()));
        assert!(resolved.ignored_toolchain_file);
        let calls = fake.rustup_calls();
        assert_eq!(calls.len(), 2, "{calls:?}");
        assert!(
            calls[1].ends_with("|<unset>|show active-toolchain"),
            "{calls:?}"
        );
        assert!(fake.scratch_is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn toolchains_without_rustup_pin_the_rustc_on_path() {
        use std::os::unix::fs::PermissionsExt;
        let root = scratch_dir("plain").canonicalize().unwrap();
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        for tool in ["cargo", "rustc"] {
            std::fs::write(bin.join(tool), "#!/bin/sh\nexit 0\n").unwrap();
            std::fs::set_permissions(bin.join(tool), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        std::fs::write(root.join("rust-toolchain.toml"), "[toolchain]\n").unwrap();
        let inputs = ToolchainInputs {
            path: Some(bin.clone().into_os_string()),
            rustup_toolchain: None,
            rustc: None,
            working_dir: Some(root.clone()),
            scratch: root.clone(),
        };
        // rustup 프록시가 없으니 툴체인 파일을 읽는 프로그램도 없다 — limitation을 달지 않는다.
        assert_eq!(
            resolve_toolchain(&inputs).unwrap(),
            Toolchain {
                cargo: Some(bin.join("cargo")),
                rustc: Some(bin.join("rustc").into_os_string()),
                pinned_toolchain: None,
                ignored_toolchain_file: false,
            }
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_toolchain_file_above_the_neutral_directory_is_refused() {
        let fake = FakeRustup::new(
            "shared",
            "fake-stable (default)",
            "/toolchains/fake/bin/rustc",
        );
        std::fs::write(fake.root.join("rust-toolchain.toml"), "[toolchain]\n").unwrap();
        let refused = resolve_toolchain(&fake.inputs()).unwrap_err();
        assert!(refused.contains("rust-toolchain.toml"), "{refused}");
        assert!(
            refused.contains("set RUSTUP_TOOLCHAIN and RUSTC"),
            "{refused}"
        );
        assert_eq!(fake.rustup_calls(), Vec::<String>::new());
        assert!(fake.scratch_is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn a_path_toolchain_reported_by_rustup_is_refused() {
        let fake = FakeRustup::new(
            "path-name",
            "/elsewhere/toolchain (overridden by rust-toolchain.toml)",
            "/elsewhere/toolchain/bin/rustc",
        );
        let refused = resolve_toolchain(&fake.inputs()).unwrap_err();
        assert!(refused.contains("not a toolchain name"), "{refused}");
        assert_eq!(fake.rustup_calls().len(), 1);
        assert!(fake.scratch_is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn failing_rustup_queries_explain_how_to_choose_the_toolchain() {
        let fake = FakeRustup::new("failing", "unused", "relative/rustc");
        // `which rustc`가 절대 경로가 아니면 쓰지 않는다.
        let chosen = ToolchainInputs {
            rustup_toolchain: Some(OsString::from("mine")),
            ..fake.inputs()
        };
        let refused = resolve_toolchain(&chosen).unwrap_err();
        assert!(
            refused.contains("not an absolute path; set RUSTC"),
            "{refused}"
        );
        // 모르는 하위 명령처럼 rustup이 실패하면 그 사실과 해결 방향을 싣는다.
        std::fs::write(
            fake.bin.join("rustup"),
            "#!/bin/sh\necho 'error: no default toolchain is configured' >&2\nexit 1\n",
        )
        .unwrap();
        let refused = resolve_toolchain(&fake.inputs()).unwrap_err();
        assert!(
            refused.contains("`rustup show active-toolchain` failed: error: no default toolchain"),
            "{refused}"
        );
        assert!(refused.contains("RUSTUP_TOOLCHAIN=stable"), "{refused}");
        assert!(fake.scratch_is_empty());
    }

    #[test]
    fn a_neutral_directory_that_did_not_stay_empty_is_an_error() {
        let scratch = scratch_dir("not-empty");
        let refused = with_neutral_dir(&scratch, |dir| {
            std::fs::write(dir.join("left-behind"), "x").map_err(|e| e.to_string())
        })
        .unwrap_err();
        assert!(refused.contains("could not remove"), "{refused}");
        std::fs::remove_dir_all(&scratch).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn rustup_behind_recognizes_linked_and_sibling_proxies() {
        use std::os::unix::fs::PermissionsExt;
        let root = scratch_dir("behind").canonicalize().unwrap();
        let executable = |path: &Path| {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        // 링크 프록시(`cargo -> rustup`)는 링크 대상이 rustup이다.
        executable(&root.join("linked/rustup"));
        std::os::unix::fs::symlink("rustup", root.join("linked/cargo")).unwrap();
        assert_eq!(
            rustup_behind(&root.join("linked/cargo")),
            Some(root.join("linked/rustup"))
        );
        // 하드 링크·복사 프록시는 rustup과 같은 디렉터리에 있다.
        executable(&root.join("copied/rustup"));
        executable(&root.join("copied/cargo"));
        assert_eq!(
            rustup_behind(&root.join("copied/cargo")),
            Some(root.join("copied/rustup"))
        );
        // rustup이 없는 디렉터리의 cargo는 프록시가 아니다(Homebrew·배포판 툴체인).
        executable(&root.join("plain/cargo"));
        assert_eq!(rustup_behind(&root.join("plain/cargo")), None);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn toolchain_file_from_finds_the_nearest_file() {
        let root = scratch_dir("nearest").canonicalize().unwrap();
        let deep = root.join("a/b/c");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(root.join("rust-toolchain.toml"), "[toolchain]\n").unwrap();
        assert_eq!(
            toolchain_file_from(&deep),
            Ok(Some(root.join("rust-toolchain.toml")))
        );
        // 가까운 디렉터리가 이긴다 — 한 디렉터리에서는 `rust-toolchain`을 먼저 본다(rustup 순서).
        std::fs::write(root.join("a/rust-toolchain.toml"), "[toolchain]\n").unwrap();
        std::fs::write(root.join("a/rust-toolchain"), "stable\n").unwrap();
        assert_eq!(
            toolchain_file_from(&deep),
            Ok(Some(root.join("a/rust-toolchain")))
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    /// 선언된 optional 의존은 resolve 그래프에 없어도(기본 feature가 켜지 않음) 패키지의
    /// `optional_deps`로 남는다 — 이름 팬아웃의 의존 폐포가 `#[cfg(feature)]` 뒤 코드의 호출을 잃지
    /// 않게 한다. 이름은 모듈 경로처럼 정규화하고, optional이 아닌 선언은 싣지 않는다. 선언 목록이 없는
    /// 옛 출력도 읽힌다.
    #[test]
    fn parse_keeps_declared_optional_dependencies() {
        let json = r#"{
            "packages": [
                {"id": "app 0.1.0", "name": "app", "version": "0.1.0",
                 "targets": [{"name": "app", "kind": ["lib"], "src_path": "/w/app/src/lib.rs"}],
                 "dependencies": [
                    {"name": "dep", "optional": false},
                    {"name": "opt-in", "optional": true},
                    {"name": "testdep", "kind": "dev"}
                 ]},
                {"id": "dep 0.1.0", "name": "dep", "version": "0.1.0",
                 "targets": [{"name": "dep", "kind": ["lib"], "src_path": "/w/dep/src/lib.rs"}]}
            ],
            "resolve": {"nodes": [
                {"id": "app 0.1.0", "deps": [
                    {"pkg": "dep 0.1.0", "name": "dep", "dep_kinds": [{"kind": null}]}
                ]},
                {"id": "dep 0.1.0", "deps": []}
            ]},
            "workspace_members": ["app 0.1.0", "dep 0.1.0"],
            "workspace_root": "/w"
        }"#;
        let meta = parse(json.as_bytes()).unwrap();
        assert_eq!(meta.packages[0].optional_deps, ["opt_in"]);
        assert!(meta.packages[1].optional_deps.is_empty());
        assert_eq!(meta.dep_edges.len(), 1);
        assert!(meta.limitations.is_empty());
    }
}
