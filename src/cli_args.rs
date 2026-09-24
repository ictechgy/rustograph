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
}

const VALUE_FLAGS: &[&str] = &[
    "dir", "level", "format", "out", "graph", "root", "explain", "config", "depth", "max", "focus",
    "target", "budget", "baseline",
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

/// 수확 또는 저장 문서 로드 — 모든 명령이 같은 경로로 문서를 얻는다.
/// --focus/--target/--exclude-tests 필터는 저장 문서에도 같은 순서로
/// 적용된다 — 필터는 문서 변환이라 수확 방식과 무관하다.
pub(crate) fn document_for(a: &Args, symbol_level: bool) -> Result<Document, String> {
    document_impl(a, symbol_level, a.has("deps"), a.has("semantic"))
}

fn document_impl(
    a: &Args,
    symbol_level: bool,
    include_deps: bool,
    semantic: bool,
) -> Result<Document, String> {
    if a.has("tests") && a.has("exclude-tests") {
        return Err("--tests and --exclude-tests are mutually exclusive".to_string());
    }
    let mut doc = if let Some(f) = a.get("graph") {
        export::load_file(Path::new(f))?
    } else {
        let dir = PathBuf::from(a.get("dir").unwrap_or("."));
        source::load(
            &dir,
            &source::Options {
                symbol_level,
                include_deps,
                tests: a.has("tests"),
                retain_public: a.has("retain-public"),
                extra_roots: a.get_all("root").iter().map(|s| s.to_string()).collect(),
                semantic,
                cache: !a.has("no-cache"),
            },
        )?
    };
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
    Ok(doc)
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

/// `--level` 값을 Level로 변환한다 — 없으면 module이 기본이다.
pub(crate) fn level_of(a: &Args) -> Result<Level, String> {
    match a.get("level") {
        Some(l) => {
            Level::parse(l).ok_or_else(|| format!("unknown level {l} (crate|module|type|symbol)"))
        }
        None => Ok(Level::Module),
    }
}
