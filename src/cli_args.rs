//! 명령행 인자 파서 — cli와 mcp가 공유하는 최하층 모듈.
//!
//! cli가 mcp를 디스패치하고 mcp가 cli의 파서를 쓰면 모듈 순환이 된다.
//! 파서를 별도 모듈로 내리면 cli → mcp → cli_args 한 방향이 유지된다.
//! 이름이 args가 아닌 이유: 지역 변수 `args`는 어디에나 있어서 이름 기반
//! 경로 해석이 지역 바인딩을 이 모듈로 오인하는 것을 피한다.

use crate::graph::{Document, Level};
use crate::{export, source};
use std::path::{Path, PathBuf};

/// 인자 묶음 — 플래그 값은 반복 가능해 Vec로 둔다.
/// mcp 명령이 같은 파서를 공유하므로 크레이트 안에서는 보인다.
pub(crate) struct Args {
    pub(crate) cmd: String,
    pub(crate) positional: Vec<String>,
    values: std::collections::BTreeMap<String, Vec<String>>,
    flags: std::collections::BTreeSet<String>,
}

impl Args {
    pub(crate) fn get(&self, key: &str) -> Option<&str> {
        self.values
            .get(key)
            .and_then(|v| v.last())
            .map(|s| s.as_str())
    }
    pub(crate) fn get_all(&self, key: &str) -> Vec<&str> {
        self.values
            .get(key)
            .map(|v| v.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default()
    }
    pub(crate) fn has(&self, key: &str) -> bool {
        self.flags.contains(key)
    }
    /// `allowed` 밖의 플래그를 하나 돌려준다 — 위치 인자는 보지 않는다.
    pub(crate) fn unsupported_flag(&self, allowed: &[&str]) -> Option<String> {
        self.values
            .keys()
            .chain(self.flags.iter())
            .find(|k| !allowed.contains(&k.as_str()))
            .map(|k| format!("--{k}"))
    }
    /// 값 플래그가 두 번 이상 주어졌는가 — 순회 명령은 반복 값을 조용히 덮지 않는다.
    pub(crate) fn repeated(&self, key: &str) -> bool {
        self.values.get(key).is_some_and(|v| v.len() > 1)
    }
    /// `allowed` 밖의 플래그나 위치 인자가 있으면 첫 번째를 사람이 읽는
    /// 형태로 돌려준다. 거부 목록이 아니라 허용 목록인 이유: 나중에
    /// 추가되는 플래그가 이 명령에서 조용히 무시되는 일을 막는다.
    pub(crate) fn unsupported(&self, allowed: &[&str]) -> Option<String> {
        if let Some(p) = self.positional.first() {
            return Some(format!("argument {p}"));
        }
        self.values
            .keys()
            .chain(self.flags.iter())
            .find(|k| !allowed.contains(&k.as_str()))
            .map(|k| format!("--{k}"))
    }
}

const VALUE_FLAGS: &[&str] = &[
    "dir",
    "level",
    "format",
    "out",
    "graph",
    "root",
    "explain",
    "config",
    "depth",
    "max",
    "focus",
    "target",
    "budget",
    "baseline",
    "roots-from",
    "max-depth",
    "max-reached",
    "revision",
    "generated-at",
    "role",
    "framework",
    "wrappers",
    "service",
];
const BOOL_FLAGS: &[&str] = &[
    "deps",
    "tests",
    "retain-public",
    "strict",
    "semantic",
    "exclude-tests",
    "no-cache",
    "write-baseline",
];

pub(crate) fn parse(args: &[String]) -> Result<Args, String> {
    let Some(cmd) = args.first() else {
        return Err("no command".to_string());
    };
    let mut a = Args {
        cmd: cmd.clone(),
        positional: Vec::new(),
        values: Default::default(),
        flags: Default::default(),
    };
    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        if arg == "--" {
            // `--` 뒤는 모두 위치 인자다 — `-`로 시작하는 정점 ID를 플래그로 읽지 않게 한다.
            a.positional.extend(args[i + 1..].iter().cloned());
            break;
        }
        if let Some(name) = arg.strip_prefix("--") {
            if BOOL_FLAGS.contains(&name) {
                a.flags.insert(name.to_string());
            } else if VALUE_FLAGS.contains(&name) {
                i += 1;
                let Some(v) = args.get(i) else {
                    return Err(format!("--{name} needs a value"));
                };
                a.values
                    .entry(name.to_string())
                    .or_default()
                    .push(v.clone());
            } else {
                return Err(format!("unknown flag --{name}"));
            }
        } else {
            a.positional.push(arg.clone());
        }
        i += 1;
    }
    Ok(a)
}

/// 그래프 문서를 얻는 명령이 모두 받는 플래그 — 문서 출처(`--dir`·`--graph`)와 USAGE의 공유 플래그.
/// 공유 플래그는 문서화된 계약이라, 명령이 그 효과를 쓰지 않아도(`cycles --tests`) 기존 호출을 깨지
/// 않도록 계속 받는다.
pub(crate) const DOCUMENT_FLAGS: &[&str] = &[
    "dir",
    "graph",
    "deps",
    "tests",
    "retain-public",
    "semantic",
    "no-cache",
    "focus",
    "target",
    "exclude-tests",
];

/// 명령이 쓰지 않는 플래그나 남는 위치 인자를 사용법 오류로 거부한다 — 순회 형식을 뺀 명령이 이
/// 검사 하나를 쓴다. `accepted`는 이 명령이 읽는 플래그 목록들(대개 [`DOCUMENT_FLAGS`]와 명령 고유
/// 플래그), `positionals`는 받는 위치 인자의 최대 개수다. 조용히 무시하면 사용자는 플래그가 적용됐다고
/// 믿는다 — `cycles --out`은 파일 없이 0으로 끝났고 `query --format mermaid`는 JSON을 냈다. 허용
/// 목록이라 나중에 추가되는 플래그도 그 명령이 읽기 전까지는 거부된다. 빠진 위치 인자는 명령마다
/// 고유한 안내가 있어 여기서 보지 않는다.
pub(crate) fn reject_unused(
    a: &Args,
    accepted: &[&[&str]],
    positionals: usize,
) -> Result<(), String> {
    if let Some(flag) = a.unsupported_flag(&accepted.concat()) {
        return Err(format!(
            "{} does not support {flag}; remove it (`rustograph help` lists the flags each command takes)",
            a.cmd
        ));
    }
    if let Some(extra) = a.positional.get(positionals) {
        return Err(format!(
            "{} does not take the argument {extra:?}; remove it (`rustograph help` lists the arguments each command takes)",
            a.cmd
        ));
    }
    Ok(())
}

/// 수확 또는 저장 문서 로드 — 모든 명령이 같은 경로로 문서를 얻는다.
/// --focus/--target/--exclude-tests 필터는 저장 문서에도 같은 순서로
/// 적용된다 — 필터는 문서 변환이라 수확 방식과 무관하다.
pub(crate) fn document_for(a: &Args, symbol_level: bool) -> Result<Document, String> {
    document_impl(a, symbol_level, a.has("deps"), a.has("semantic"), false).map(|(doc, _)| doc)
}

/// [`document_for`]와 같은 문서와, 문서를 수확했다면 그 수확이 읽은 입력을 돌려준다 —
/// 결과를 파일로 쓰는 명령(`graph --out`, `rules --write-baseline`)이 입력 보호 목록을
/// 위해 워크스페이스를 다시 수확하지 않게 한다. `want_inputs`가 false면 입력을 모으지 않는다.
pub(crate) fn document_with_inputs(
    a: &Args,
    symbol_level: bool,
    want_inputs: bool,
) -> Result<(Document, Option<source::HarvestInputs>), String> {
    document_impl(
        a,
        symbol_level,
        a.has("deps"),
        a.has("semantic"),
        want_inputs,
    )
}

fn document_impl(
    a: &Args,
    symbol_level: bool,
    include_deps: bool,
    semantic: bool,
    want_inputs: bool,
) -> Result<(Document, Option<source::HarvestInputs>), String> {
    if a.has("tests") && a.has("exclude-tests") {
        return Err("--tests and --exclude-tests are mutually exclusive".to_string());
    }
    // 저장 그래프를 읽으면 수확 옵션은 전부 무효다 — 조용히 무시하면
    // 사용자가 준 옵션이 적용됐다고 오해한다.
    if a.get("graph").is_some() {
        for flag in ["tests", "retain-public", "root", "semantic", "no-cache"] {
            if a.has(flag) || a.get(flag).is_some() {
                return Err(format!(
                    "--{flag} is ignored with --graph — drop one of them"
                ));
            }
        }
    }
    let (mut doc, inputs) = if let Some(f) = a.get("graph") {
        (export::load_file(Path::new(f))?, None)
    } else {
        let dir = PathBuf::from(a.get("dir").unwrap_or("."));
        let opts = source::Options {
            symbol_level,
            include_deps,
            tests: a.has("tests"),
            retain_public: a.has("retain-public"),
            extra_roots: a.get_all("root").iter().map(|s| s.to_string()).collect(),
            semantic,
            cache: !a.has("no-cache"),
        };
        if want_inputs {
            source::load_with_inputs(&dir, &opts)?
        } else {
            (source::load(&dir, &opts)?, None)
        }
    };
    if symbol_level && !matches!(a.cmd.as_str(), "graph" | "cycles" | "mcp" | "search") {
        doc.require_level(Level::Symbol)?;
    }
    if a.has("exclude-tests") {
        doc = doc.without_tests();
    }
    if let Some(t) = a.get("target") {
        // 권위 있는 팩트를 먼저 시도하고 rustc가 없거나 트리플을 모르면
        // 트리플 추정으로 폴백한다 — 모르는 것은 거짓이 아니라 미지다.
        doc = doc.for_target(t, &source::target_facts(t));
    }
    let focus: Vec<String> = a.get_all("focus").iter().map(|s| s.to_string()).collect();
    if !focus.is_empty() {
        doc = doc.focus(&focus);
    }
    Ok((doc, inputs))
}

/// 정수 인자 하나를 꺼낸다 — 없으면 기본값, 있으면 반드시 파싱돼야 한다.
/// `--max abc`나 음수를 조용히 기본값으로 되돌리면 사용자가 준 한계가
/// 무시된다 — 잘못된 값은 명시적 오류다.
pub(crate) fn usize_arg(a: &Args, key: &str, default: usize) -> Result<usize, String> {
    match a.get(key) {
        Some(s) => s
            .parse::<usize>()
            .map_err(|_| format!("invalid --{key} {s:?} — expected a non-negative integer")),
        None => Ok(default),
    }
}

/// query·impact·search가 한 번에 돌려주는 레코드 수(`--max`, MCP `max`)의 상한 — 10,000.
/// 계열 schemagraph MCP의 `MAX_RESULTS`와 같고 기본값(query·impact 200, search 20)의 50배다. 이보다 긴
/// 목록은 에이전트가 한 번에 읽을 답이 아니다. 방문 수도 이 값이 묶는다: 이웃 걷기는 보고 수가 `max`에
/// 닿으면 멈추므로 깊이 제한 없는 `--depth 0`도 정점을 `max`+1개보다 많이 방문하지 않는다.
pub(crate) const MAX_RESULTS: usize = 10_000;

/// query·impact 깊이(`--depth`, MCP `depth`)의 상한 — 64. 계열 schemagraph MCP의 `MAX_DEPTH`와 같다.
/// 0(깊이 제한 없음)은 상한 안이다. 64홉보다 먼 의존자를 원하면 0이 그 답이다(보고 수는 `max`가 묶는다).
pub(crate) const MAX_DEPTH: usize = 64;

/// paths가 보고하는 경로 수(`--max`, MCP `max`)의 상한 — 1,024. 계열 schemagraph MCP의 `MAX_PATHS`와
/// 같고 기본값 10의 100배다.
pub(crate) const MAX_PATHS: usize = 1_024;

/// paths 확장 예산(`--budget`, MCP `budget`)의 상한 — 1,000,000(기본값 50,000의 20배). 대기 부분 경로
/// 상태가 예산까지 쌓이므로 메모리와 시간이 예산에 비례한다. 30층×폭 6 완전 연결 DAG에서 100만은
/// release 0.15초·75MB다(상태 한 칸 40바이트). 상태마다 경로 전체를 복제하던 때는 같은 입력에서
/// 511MB였고, 리뷰 실측으로 1,000만은 7.1GB였다 — 상한이 없으면 호출 하나가 이만큼 쓴다.
pub(crate) const MAX_PATH_BUDGET: usize = 1_000_000;

/// 상한 오류가 인자를 부르는 표기 — CLI는 플래그(`--max`), MCP는 도구 인자 이름(`max`)이다.
#[derive(Clone, Copy)]
pub(crate) enum ArgSpelling {
    Flag,
    ToolArgument,
}

/// query·impact의 `depth`·`max`가 상한 안인지 본다 — CLI `query`·`impact`와 MCP query·impact 도구가 이
/// 함수 하나로 같은 값을 거부한다(CLI 종료 2, MCP isError).
pub(crate) fn check_neighbor_limits(
    spelling: ArgSpelling,
    depth: usize,
    max: usize,
) -> Result<(), String> {
    let hint = " (0 walks without a depth limit)";
    at_most(spelling, "depth", depth, MAX_DEPTH, hint)?;
    at_most(spelling, "max", max, MAX_RESULTS, "")
}

/// search의 `max`가 상한 안인지 본다 — CLI `search`와 MCP search 도구가 함께 쓴다.
pub(crate) fn check_search_limit(spelling: ArgSpelling, max: usize) -> Result<(), String> {
    at_most(spelling, "max", max, MAX_RESULTS, "")
}

/// paths의 `max`·`budget`이 상한 안인지 본다 — CLI `paths`와 MCP paths 도구가 함께 쓴다.
pub(crate) fn check_paths_limits(
    spelling: ArgSpelling,
    max: usize,
    budget: usize,
) -> Result<(), String> {
    at_most(spelling, "max", max, MAX_PATHS, "")?;
    at_most(spelling, "budget", budget, MAX_PATH_BUDGET, "")
}

/// 인자 하나가 `limit` 이하인지 본다. 넘는 값을 조용히 깎지 않는다 — 깎은 답은 요청한 크기의 답처럼
/// 읽히고, 그대로 받으면 호출 하나가 메모리·시간을 호출자 마음대로 쓴다. `hint`는 해결 방향에 덧붙인다.
fn at_most(
    spelling: ArgSpelling,
    name: &str,
    value: usize,
    limit: usize,
    hint: &str,
) -> Result<(), String> {
    if value <= limit {
        return Ok(());
    }
    let shown = match spelling {
        ArgSpelling::Flag => format!("--{name}"),
        ArgSpelling::ToolArgument => name.to_string(),
    };
    Err(format!(
        "{shown} {value} exceeds the limit of {limit}; pass a value from 0 to {limit}{hint}"
    ))
}

/// query·impact의 깊이 인자를 분석 함수에 넘길 깊이로 바꾼다 — CLI `--depth`와 MCP `depth`가 이 함수
/// 하나를 지난다. 0은 "깊이 제한 없음"(전체 폐포, 보고 수는 `max`가 묶는다)이다: MCP 도구 설명
/// ("0 = full closure")과 순회 형식(`--depth 0`은 상한 128)이 0을 "끝까지"로 문서화했는데 CLI만 빈
/// 결과를 내, 같은 `depth: 0, truncated: false`가 통로마다 반대 뜻이었다. 보고 JSON의 `depth`에는
/// 호출자가 요청값을 다시 싣는다(`usize::MAX`는 계약이 아니다). analysis 라이브러리 함수의 깊이 뜻
/// (0 = 이웃 없음)은 라이브러리 호출자를 위해 그대로 둔다.
pub(crate) fn walk_depth(requested: usize) -> usize {
    if requested == 0 {
        usize::MAX
    } else {
        requested
    }
}

/// `--level` 값을 Level로 변환한다 — 없으면 module이 기본이다.
pub(crate) fn level_of(a: &Args) -> Result<Level, String> {
    match a.get("level") {
        Some(l) => {
            Level::parse(l).ok_or_else(|| format!("unknown level {l} (crate|module|type|symbol)"))
        }
        None => Ok(Level::Module),
    }
}
