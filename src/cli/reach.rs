//! `reach`·`impact --format language-traversal` — isthmus `language-traversal` v1 문서를 낸다.
//!
//! isthmus `trace`가 relation-use 사실의 `symbol.usr`와 핸들러·호출부 도달을 문자열 그대로 잇는다.
//! 계열 규칙(pythograph·tsograph·cartograph)과 같은 종료 코드를 쓴다: 사용법 오류는 표준 출력을
//! 비운 채 64, 그래프 정점이 아닌 root가 섞이면 나머지 root로 순회한 문서를 쓰고 64, 수확 실패는 2.

use crate::cli_args::{self, Args};
use crate::graph::{Document, Vertex};
use crate::traversal::{self, Direction};
use crate::{export, source};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// 사용법 오류 종료 코드(BSD sysexits `EX_USAGE`) — 계열 순회 명령과 같다.
pub(crate) const EXIT_USAGE: i32 = 64;

/// `--roots-from` 입력 상한(바이트).
const MAX_ROOTS_INPUT: u64 = 16 * 1024 * 1024;
/// `--revision` 최대 길이(문자).
const MAX_REVISION_CHARS: usize = 256;

/// 순회 명령이 받는 플래그 — 허용 목록이라 새 플래그가 조용히 무시되지 않는다.
const ALLOWED: &[&str] = &[
    "dir",
    "graph",
    "deps",
    "tests",
    "semantic",
    "no-cache",
    "focus",
    "target",
    "exclude-tests",
    "format",
    "depth",
    "max-depth",
    "max-reached",
    "roots-from",
    "revision",
    "generated-at",
];

/// 순회 명령 실패 — 사용법(64, 표준 출력 비움)과 분석(2)을 가른다.
pub(crate) enum Failure {
    Usage(String),
    Analysis(String),
}

/// 인자 목록이 순회 명령인가 — 전역 파서가 실패해도 64 계약을 지키기 위해 본다.
pub(crate) fn is_traversal_argv(args: &[String]) -> bool {
    args.first().is_some_and(|c| c == "reach")
        || (args.first().is_some_and(|c| c == "impact")
            && args
                .windows(2)
                .any(|w| w[0] == "--format" && w[1] == "language-traversal"))
}

/// 순회 명령을 실행한다. 반환값은 종료 코드다.
/// 도구 버전은 호출자가 넘긴다 — `crate::cli::VERSION`을 직접 읽으면 cli ↔ cli::reach
/// 모듈 순환이 된다(자기 분석 `cycles --strict`).
pub(crate) fn cmd(
    a: &Args,
    version: &'static str,
    direction: Direction,
    stdin: &mut dyn Read,
    out: &mut dyn Write,
) -> Result<i32, Failure> {
    let opts = Options::parse(a, direction, stdin)?;
    let doc = cli_args::document_for(a, true).map_err(Failure::Analysis)?;
    let project = std::fs::canonicalize(a.get("dir").unwrap_or("."))
        .map_err(|e| Failure::Analysis(format!("cannot resolve --dir: {e}")))?;
    let document = build(&doc, &opts, &project, version);
    write!(out, "{}", export::to_json(&document.body)).ok();
    Ok(if document.missing > 0 { EXIT_USAGE } else { 0 })
}

/// 검증한 순회 옵션.
struct Options {
    direction: Direction,
    roots: Vec<String>,
    max_depth: usize,
    max_reached: usize,
    revision: Option<String>,
    generated_at: String,
}

impl Options {
    /// 인자를 검증한다 — 수확 전에 끝나야 사용법 오류가 표준 출력을 비운 채 64가 된다.
    fn parse(a: &Args, direction: Direction, stdin: &mut dyn Read) -> Result<Options, Failure> {
        if let Some(bad) = a.unsupported_flag(ALLOWED) {
            return Err(Failure::Usage(format!(
                "{bad} is not supported by traversal output"
            )));
        }
        if let Some(k) = ALLOWED.iter().find(|k| a.repeated(k)) {
            return Err(Failure::Usage(format!("--{k} may be given only once")));
        }
        let format = a.get("format").unwrap_or("language-traversal");
        if format != "language-traversal" {
            return Err(Failure::Usage(format!(
                "--format {format} is not supported — traversal output is language-traversal"
            )));
        }
        let roots = collect_roots(a, stdin).map_err(Failure::Usage)?;
        Ok(Options {
            direction,
            roots,
            max_depth: depth_of(a).map_err(Failure::Usage)?,
            max_reached: bounded(a.get("max-reached"), traversal::MAX_REACHED, "max-reached")
                .map_err(Failure::Usage)?,
            revision: revision_of(a).map_err(Failure::Usage)?,
            generated_at: match a.get("generated-at") {
                Some(t) if is_timestamp(t) => t.to_string(),
                Some(_) => {
                    return Err(Failure::Usage(
                        "--generated-at must be an RFC 3339 timestamp such as 2026-09-30T00:00:00Z"
                            .to_string(),
                    ))
                }
                None => source::schema::rfc3339_utc_now(),
            },
        })
    }
}

/// 깊이 — `--max-depth N`(1~128) 또는 impact와 같은 `--depth N`(0은 상한 128). 기본은 128.
fn depth_of(a: &Args) -> Result<usize, String> {
    match (a.get("max-depth"), a.get("depth")) {
        (Some(_), Some(_)) => Err("give either --max-depth or --depth, not both".to_string()),
        (Some(v), None) => bounded(Some(v), traversal::MAX_DEPTH, "max-depth"),
        (None, Some("0")) => Ok(traversal::MAX_DEPTH),
        (None, Some(v)) => bounded(Some(v), traversal::MAX_DEPTH, "depth"),
        (None, None) => Ok(traversal::MAX_DEPTH),
    }
}

/// 1~limit 정수 인자(없으면 limit).
fn bounded(value: Option<&str>, limit: usize, name: &str) -> Result<usize, String> {
    let Some(v) = value else {
        return Ok(limit);
    };
    match v.parse::<usize>() {
        Ok(n) if (1..=limit).contains(&n) && !v.starts_with('+') => Ok(n),
        _ => Err(format!("--{name} takes an integer from 1 to {limit}")),
    }
}

/// `--revision` — 없으면 작업 트리가 깨끗할 때의 git HEAD.
fn revision_of(a: &Args) -> Result<Option<String>, String> {
    match a.get("revision") {
        Some(r) if r.is_empty() || r.chars().count() > MAX_REVISION_CHARS || has_forbidden(r) => {
            Err(format!(
                "--revision must be 1-{MAX_REVISION_CHARS} characters without control characters"
            ))
        }
        Some(r) => Ok(Some(r.to_string())),
        None => Ok(None),
    }
}

/// id·revision에 올 수 없는 문자다(제어 문자, U+2028/2029) — isthmus의 안전 문자열 규칙과 같다.
fn has_forbidden(s: &str) -> bool {
    s.chars().any(|c| {
        let u = c as u32;
        u <= 0x1f || (0x7f..=0x9f).contains(&u) || u == 0x2028 || u == 0x2029
    })
}

/// 위치 인자 다음에 `--roots-from`의 id를 잇고, 중복은 처음 나온 순서로 하나만 남긴다
/// (그 순서가 `reached[].roots` 인덱스의 뜻이다).
fn collect_roots(a: &Args, stdin: &mut dyn Read) -> Result<Vec<String>, String> {
    let mut ids: Vec<String> = a.positional.clone();
    if let Some(src) = a.get("roots-from") {
        ids.extend(parse_roots_text(&read_roots(src, stdin)?)?);
    }
    let mut seen = BTreeSet::new();
    ids.retain(|id| seen.insert(id.clone()));
    if ids.is_empty() {
        return Err(
            "give at least one symbol id as an argument or through --roots-from".to_string(),
        );
    }
    if ids.iter().any(|id| id.is_empty() || has_forbidden(id)) {
        return Err("symbol ids must be non-empty and free of control characters".to_string());
    }
    if ids.len() > traversal::MAX_ROOTS {
        return Err(format!(
            "at most {} roots are allowed per run; split the roots into several runs",
            traversal::MAX_ROOTS
        ));
    }
    Ok(ids)
}

/// `--roots-from` 입력을 읽는다(`-`는 표준 입력). 원문은 오류에 싣지 않는다.
fn read_roots(src: &str, stdin: &mut dyn Read) -> Result<String, String> {
    let mut text = String::new();
    let res = if src == "-" {
        stdin.take(MAX_ROOTS_INPUT + 1).read_to_string(&mut text)
    } else {
        std::fs::File::open(src).and_then(|f| f.take(MAX_ROOTS_INPUT + 1).read_to_string(&mut text))
    };
    res.map_err(|_| {
        "--roots-from could not be read as UTF-8 text; pass a readable file or - for stdin"
            .to_string()
    })?;
    if text.len() as u64 > MAX_ROOTS_INPUT {
        return Err(
            "--roots-from is larger than 16 MiB; split the roots into several runs".to_string(),
        );
    }
    Ok(text)
}

/// JSON 문자열 배열 또는 bridge-facts 문서(사실의 `symbol.usr`)에서 id를 꺼낸다.
fn parse_roots_text(text: &str) -> Result<Vec<String>, String> {
    let bad =
        || "--roots-from must be a JSON array of strings or a bridge-facts document".to_string();
    let value: serde_json::Value = serde_json::from_str(text).map_err(|_| bad())?;
    if let Some(items) = value.as_array() {
        return items
            .iter()
            .map(|v| v.as_str().map(str::to_string).ok_or_else(bad))
            .collect();
    }
    if value["format"] == "bridge-facts" {
        if let Some(facts) = value["facts"].as_array() {
            return Ok(facts
                .iter()
                .filter_map(|f| f["symbol"]["usr"].as_str().map(str::to_string))
                .collect());
        }
    }
    Err(bad())
}

/// RFC 3339 타임스탬프인지 본다 — isthmus bridge-facts의 generatedAt 문법과 같다.
fn is_timestamp(s: &str) -> bool {
    let b = s.as_bytes();
    let digits = |r: std::ops::Range<usize>| {
        b.get(r.clone())
            .filter(|x| x.iter().all(u8::is_ascii_digit))
            .and_then(|x| {
                std::str::from_utf8(x)
                    .ok()
                    .and_then(|t| t.parse::<u32>().ok())
            })
    };
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || b[10] != b'T'
        || b[13] != b':'
        || b[16] != b':'
    {
        return false;
    }
    let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(se)) = (
        digits(0..4),
        digits(5..7),
        digits(8..10),
        digits(11..13),
        digits(14..16),
        digits(17..19),
    ) else {
        return false;
    };
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if !(1..=12).contains(&mo)
        || d == 0
        || d > days[mo as usize - 1]
        || h > 23
        || mi > 59
        || se > 59
    {
        return false;
    }
    let mut rest = &s[19..];
    if let Some(frac) = rest.strip_prefix('.') {
        let n = frac.bytes().take_while(u8::is_ascii_digit).count();
        if n == 0 {
            return false;
        }
        rest = &frac[n..];
    }
    let zone = rest.as_bytes();
    rest == "Z"
        || (zone.len() == 6
            && (zone[0] == b'+' || zone[0] == b'-')
            && zone[3] == b':'
            && zone[1..3].iter().chain(&zone[4..6]).all(u8::is_ascii_digit))
}

/// 순회 문서와 정점이 아닌 root 수.
struct Built {
    body: TraversalDocument,
    missing: usize,
}

/// 계약의 문서 — 키 순서는 계약 문서의 나열 순서다.
#[derive(Serialize)]
struct TraversalDocument {
    format: &'static str,
    version: u8,
    tool: Tool,
    #[serde(rename = "generatedAt")]
    generated_at: String,
    platform: &'static str,
    project: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    revision: Option<String>,
    #[serde(rename = "graphRevision")]
    graph_revision: String,
    direction: &'static str,
    roots: Vec<RootEntry>,
    reached: Vec<ReachedEntry>,
    #[serde(rename = "rootsTruncated", skip_serializing_if = "std::ops::Not::not")]
    roots_truncated: bool,
    truncated: bool,
    #[serde(rename = "truncationReasons", skip_serializing_if = "Vec::is_empty")]
    truncation_reasons: Vec<&'static str>,
    limitations: Vec<String>,
}

#[derive(Serialize)]
struct Tool {
    name: &'static str,
    version: &'static str,
}

#[derive(Serialize)]
struct RootEntry {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    symbol: Option<Symbol>,
}

#[derive(Serialize)]
struct ReachedEntry {
    symbol: Symbol,
    via: String,
    depth: usize,
    roots: Vec<usize>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    relationships: Vec<String>,
    evidence: &'static str,
}

#[derive(Serialize)]
struct Symbol {
    usr: String,
    #[serde(rename = "qualifiedName")]
    qualified_name: String,
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    location: Option<Location>,
}

#[derive(Serialize)]
struct Location {
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<u64>,
}

/// 정점 id → 대표 정점(같은 id의 cfg 변형 중 정렬상 첫째) — 이름·종류·위치 출처.
fn vertex_map(doc: &Document) -> BTreeMap<&str, &Vertex> {
    let mut map = BTreeMap::new();
    for v in &doc.vertices {
        map.entry(v.id.as_str()).or_insert(v);
    }
    map
}

/// 문서를 조립한다. 정점인 root로만 순회하고 root 인덱스를 요청 순서로 옮긴다.
fn build(doc: &Document, opts: &Options, project: &Path, version: &'static str) -> Built {
    let vertices = vertex_map(doc);
    let resolved: Vec<String> = opts
        .roots
        .iter()
        .filter(|r| vertices.contains_key(r.as_str()))
        .cloned()
        .collect();
    let missing = opts.roots.len() - resolved.len();
    let position: BTreeMap<&str, usize> = opts
        .roots
        .iter()
        .enumerate()
        .map(|(i, r)| (r.as_str(), i))
        .collect();
    let mapping: Vec<usize> = resolved.iter().map(|r| position[r.as_str()]).collect();
    let outcome = if resolved.is_empty() {
        None
    } else {
        Some(traversal::traverse(
            doc,
            &traversal::Request {
                roots: &resolved,
                direction: opts.direction,
                max_depth: opts.max_depth,
                max_reached: opts.max_reached,
                evidence_memory_bytes: traversal::EVIDENCE_MEMORY_BYTES,
            },
        ))
    };
    let mut locator = Locator::new(project);
    let mut reasons: Vec<&'static str> = outcome
        .as_ref()
        .map(|o| o.truncation_reasons.clone())
        .unwrap_or_default();
    if missing > 0 {
        reasons.push("root-not-found");
    }
    reasons.sort_unstable();
    reasons.dedup();
    let mut limitations = doc.limitations.clone();
    if missing > 0 {
        limitations.push(format!(
            "root-not-found: {missing} requested root id(s) are not rustograph graph vertices; they are listed in roots without symbol"
        ));
    }
    if outcome.as_ref().is_some_and(|o| o.evidence_approximated) {
        limitations.push(
            "evidence-approximated: per-root evidence comparison exceeded its memory budget; some symbols are reported candidate conservatively"
                .to_string(),
        );
    }
    let roots = opts
        .roots
        .iter()
        .map(|id| RootEntry {
            id: id.clone(),
            symbol: vertices
                .get(id.as_str())
                .map(|v| symbol_of(v, &mut locator)),
        })
        .collect();
    let reached = outcome
        .as_ref()
        .map(|o| {
            o.reached
                .iter()
                .map(|r| ReachedEntry {
                    symbol: symbol_of(vertices[r.id.as_str()], &mut locator),
                    via: r.via.clone(),
                    depth: r.depth,
                    roots: r.roots.iter().map(|&i| mapping[i]).collect(),
                    relationships: r.relationships.clone(),
                    evidence: r.evidence.as_str(),
                })
                .collect()
        })
        .unwrap_or_default();
    Built {
        body: TraversalDocument {
            format: "language-traversal",
            version: 1,
            tool: Tool {
                name: "rustograph",
                version,
            },
            generated_at: opts.generated_at.clone(),
            platform: "rust",
            project: project.display().to_string(),
            revision: opts.revision.clone().or_else(|| git_revision(project)),
            graph_revision: graph_revision(doc, project),
            direction: opts.direction.as_str(),
            roots,
            reached,
            roots_truncated: outcome.as_ref().is_some_and(|o| o.roots_truncated),
            truncated: !reasons.is_empty(),
            truncation_reasons: reasons,
            limitations,
        },
        missing,
    }
}

/// 정점의 계약 심볼 — usr와 qualifiedName은 정점 id 그대로다.
fn symbol_of(v: &Vertex, locator: &mut Locator) -> Symbol {
    Symbol {
        usr: v.id.clone(),
        qualified_name: v.id.clone(),
        kind: serde_json::to_value(v.kind)
            .ok()
            .and_then(|k| k.as_str().map(str::to_string))
            .unwrap_or_default(),
        location: v.position.as_deref().and_then(|p| locator.locate(p)),
    }
}

/// 정점 위치(`file:line`)를 project 상대 위치로 바꾼다 — 파일 정규화는 캐시한다.
struct Locator {
    project: PathBuf,
    cache: BTreeMap<String, Option<String>>,
}

impl Locator {
    fn new(project: &Path) -> Locator {
        Locator {
            project: project.to_path_buf(),
            cache: BTreeMap::new(),
        }
    }

    /// project 밖(외부 크레이트·`#[path]`로 벗어난 파일)이면 위치를 싣지 않는다.
    fn locate(&mut self, position: &str) -> Option<Location> {
        let (file, line) = position.rsplit_once(':')?;
        let project = &self.project;
        let rel = self
            .cache
            .entry(file.to_string())
            .or_insert_with(|| {
                let canon = std::fs::canonicalize(file).ok()?;
                let rel = canon.strip_prefix(project).ok()?;
                let text = rel.to_str()?.replace('\\', "/");
                (!text.is_empty()).then_some(text)
            })
            .clone()?;
        Some(Location {
            path: rel,
            line: line.parse::<u64>().ok().filter(|n| *n > 0),
        })
    }
}

/// 그래프 산출물의 신원 — root를 project로 맞춘 그래프 JSON의 SHA-256.
/// `--dir .`과 절대 경로처럼 같은 그래프를 다른 문자열로 연 실행이 같은 값을 내야
/// isthmus가 같은 플랫폼 분석끼리 stale로 오판하지 않는다.
fn graph_revision(doc: &Document, project: &Path) -> String {
    let mut canonical = doc.clone();
    canonical.root = project.display().to_string();
    traversal::sha256::hex_digest(export::to_json(&canonical).as_bytes())
}

/// 작업 트리가 깨끗할 때만 git HEAD를 돌려준다. 커밋하지 않은 변경·추적하지 않는 파일이
/// 있으면 HEAD는 분석한 소스가 아니므로 싣지 않는다 — isthmus trace가 revision 없는 분석을
/// `analysis-revision-unknown`으로 드러낸다. 선택 잠금·fsmonitor 없이 실행해 저장소를 고치지 않는다.
fn git_revision(project: &Path) -> Option<String> {
    let status = git(
        project,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=normal",
            "--ignore-submodules=none",
        ],
    )?;
    if !status.trim().is_empty() {
        return None;
    }
    let head = git(project, &["rev-parse", "--verify", "HEAD"])?;
    let head = head.trim();
    let hex = head
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    (hex && (head.len() == 40 || head.len() == 64)).then(|| head.to_string())
}

/// git 명령 하나 — 실패(없음·저장소 아님)는 None이다. revision 없음이 곧 신호다.
fn git(project: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
        ])
        .args(args)
        .current_dir(project)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_follow_bridge_grammar() {
        for ok in [
            "2026-09-30T00:00:00Z",
            "2024-02-29T23:59:59.123Z",
            "2026-09-30T00:00:00+09:00",
        ] {
            assert!(is_timestamp(ok), "{ok}");
        }
        for bad in [
            "2026-09-30",
            "2025-02-29T00:00:00Z",
            "2026-13-01T00:00:00Z",
            "2026-09-30T24:00:00Z",
            "2026-09-30T00:00:00",
            "2026-09-30T00:00:00.Z",
            "2026-09-30T00:00:00+0900",
            "２０２６-09-30T00:00:00Z",
        ] {
            assert!(!is_timestamp(bad), "{bad}");
        }
    }

    #[test]
    fn roots_text_reads_arrays_and_bridge_facts() {
        assert_eq!(
            parse_roots_text(r#"["a","b"]"#).unwrap(),
            vec!["a".to_string(), "b".to_string()]
        );
        let facts = r#"{"format":"bridge-facts","facts":[{"symbol":{"usr":"c::f"}},{"kind":"x"}]}"#;
        assert_eq!(parse_roots_text(facts).unwrap(), vec!["c::f".to_string()]);
        assert!(parse_roots_text(r#"[1]"#).is_err());
        assert!(parse_roots_text(r#"{"format":"other"}"#).is_err());
        assert!(parse_roots_text("not json").is_err());
    }

    #[test]
    fn forbidden_characters_cover_contract_set() {
        assert!(has_forbidden("a\u{0}"));
        assert!(has_forbidden("a\u{85}"));
        assert!(has_forbidden("a\u{2028}"));
        assert!(!has_forbidden("c::<T as Tr>::m"));
    }

    #[test]
    fn traversal_argv_detection() {
        let v = |s: &[&str]| s.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert!(is_traversal_argv(&v(&["reach", "x"])));
        assert!(is_traversal_argv(&v(&[
            "impact",
            "--format",
            "language-traversal"
        ])));
        assert!(!is_traversal_argv(&v(&["impact", "x"])));
        assert!(!is_traversal_argv(&v(&[])));
    }
}
