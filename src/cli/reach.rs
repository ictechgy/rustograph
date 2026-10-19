//! `reach`·`impact --format language-traversal` — isthmus `language-traversal` v1 문서를 낸다.
//!
//! isthmus `trace`가 relation-use 사실의 `symbol.usr`와 핸들러·호출부 도달을 문자열 그대로 잇는다.
//! 계열 규칙(pythograph·tsograph·cartograph)과 같은 종료 코드를 쓴다: 사용법 오류는 표준 출력을
//! 비운 채 64, 그래프 정점이 아닌 root가 섞이면 나머지 root로 순회한 문서를 쓰고 64, 수확 실패는 2.

use crate::cli_args::{self, Args};
use crate::graph::{Document, Edge, Level, Vertex};
use crate::traversal::{self, Direction};
use crate::{export, source};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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
            && (args
                .windows(2)
                .any(|w| w[0] == "--format" && w[1] == "language-traversal")
                // `=` 표기도 같은 요청이다 — 파싱이 실패해도 순회 명령의 64 계약을 따른다.
                || args.iter().any(|a| a == "--format=language-traversal")))
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
/// 판정은 저장 문서의 ID 검사(`export::load_file`)와 같은 graph 규칙 하나다.
fn has_forbidden(s: &str) -> bool {
    s.chars().any(crate::graph::is_forbidden_id_char)
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
    // 오프셋도 시각이다 — +99:99처럼 범위 밖이면 isthmus가 문서 전체를 거부한다.
    let offset_ok = || {
        let hh = std::str::from_utf8(&zone[1..3]).ok()?.parse::<u32>().ok()?;
        let mm = std::str::from_utf8(&zone[4..6]).ok()?.parse::<u32>().ok()?;
        Some(hh <= 23 && mm <= 59)
    };
    rest == "Z"
        || (zone.len() == 6
            && (zone[0] == b'+' || zone[0] == b'-')
            && zone[3] == b':'
            && zone[1..3].iter().chain(&zone[4..6]).all(u8::is_ascii_digit)
            && offset_ok() == Some(true))
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
    let (revision, unverified) = revision_for(opts, project);
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
    limitations.extend(unverified.map(|Unverified(note)| note));
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
            revision,
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
/// JSON은 `export::to_json`과 같은 바이트(pretty + 끝 개행)를 해셔로 흘려 보낸다 — 문서를
/// 복제하고 JSON 문자열을 만든 뒤 그 사본에 패딩까지 붙이면 간선 수백만 그래프에서 이 함수
/// 하나가 문서 크기의 몇 배를 더 썼다.
fn graph_revision(doc: &Document, project: &Path) -> String {
    let root = project.display().to_string();
    let mut hasher = traversal::sha256::Sha256::new();
    // 해셔 쓰기는 실패하지 않고, 문서는 문자열 키 구조체뿐이라 직렬화도 실패할 수 없다
    // (`export::to_json`의 expect와 같은 근거).
    serde_json::to_writer_pretty(&mut hasher, &RootedGraph::new(doc, &root))
        .expect("graph document must serialize");
    hasher.update(b"\n");
    hasher.finalize_hex()
}

/// root만 바꾼 그래프 문서의 차용 뷰 — graphRevision이 해시하는 JSON이다. 필드 이름·순서·
/// 생략 규칙이 `graph::Document`의 serde 정의와 같아야 `graph --level symbol` 출력과 같은
/// 바이트가 된다(tests/traversal.rs가 해시로 대조한다).
#[derive(Serialize)]
struct RootedGraph<'a> {
    version: u32,
    tool: &'a str,
    level: Level,
    root: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    workspace: Option<&'a str>,
    roots: &'a [String],
    vertices: &'a [Vertex],
    edges: &'a [Edge],
    #[serde(skip_serializing_if = "<[String]>::is_empty")]
    limitations: &'a [String],
}

impl<'a> RootedGraph<'a> {
    /// 문서를 `..` 없이 구조 분해한다 — `Document`에 필드가 늘면 여기서 컴파일이 멈춰, 새 필드가
    /// 해시 입력에서 조용히 빠지지 않는다.
    fn new(doc: &'a Document, root: &'a str) -> RootedGraph<'a> {
        let Document {
            version,
            tool,
            level,
            root: _,
            workspace,
            roots,
            vertices,
            edges,
            limitations,
        } = doc;
        RootedGraph {
            version: *version,
            tool,
            level: *level,
            root,
            workspace: workspace.as_deref(),
            roots,
            vertices,
            edges,
            limitations,
        }
    }
}

/// 문서의 revision — `--revision`, 없으면 깨끗한 작업 트리의 git HEAD. 두 번째 값은 작업 트리를
/// 일부러 보지 않아 revision을 싣지 못한 이유다(limitation으로 간다).
fn revision_for(opts: &Options, project: &Path) -> (Option<String>, Option<Unverified>) {
    match &opts.revision {
        Some(given) => (Some(given.clone()), None),
        None => match git_revision(project) {
            Ok(head) => (head, None),
            Err(unverified) => (None, Some(unverified)),
        },
    }
}

/// 작업 트리를 일부러 보지 않아 revision을 싣지 못한 이유 — 순회 문서의 `revision-unverified:`
/// limitation 문구다. 더러운 트리나 git·저장소가 없는 경우는 revision이 없다는 것 자체가 신호라
/// 이 값을 만들지 않는다(없는 것과 확인하지 못한 것을 가른다).
struct Unverified(String);

impl Unverified {
    /// `-c`로 덮을 수 없는 filter 드라이버 이름이 설정에 있다. 이름 원문은 싣지 않는다 — 신뢰하지
    /// 않는 설정 값이고 제어 문자가 섞일 수 있다.
    fn driver_name() -> Unverified {
        Unverified(
            "revision-unverified: the git configuration defines a filter driver whose name contains '=', a control character or invalid UTF-8, which git cannot be told to skip, so no git command read the work tree; pass --revision to record the analyzed revision"
                .to_string(),
        )
    }

    /// index가 서브모듈(gitlink)을 기록한다 — 서브모듈 작업 트리를 보려면 git이 그 안에서 서브모듈
    /// 저장소 자신의 설정을 따른다.
    fn submodules(count: usize) -> Unverified {
        Unverified(format!(
            "revision-unverified: the work tree has {count} git submodule(s); rustograph does not run git inside them (that would follow their repositories' configuration, such as filter commands), so it cannot tell whether the analyzed source is HEAD; pass --revision to record the analyzed revision"
        ))
    }
}

/// 작업 트리가 깨끗할 때만 git HEAD를 돌려준다. 커밋하지 않은 변경·추적하지 않는 파일이
/// 있으면 HEAD는 분석한 소스가 아니므로 싣지 않는다(`Ok(None)`, git이 없거나 저장소가 아니거나
/// git이 실패해도 같다) — isthmus trace가 revision 없는 분석을 `analysis-revision-unknown`으로
/// 드러낸다. 모든 호출이 `git`의 격리를 지난다. 그래서 git-lfs처럼 filter로 정규화되던 파일은 stat
/// 캐시가 낡으면 바뀐 것으로 보이고, 그때는 깨끗하다고 주장하지 않는다(fail-safe, DECISIONS D1).
/// index에 서브모듈(gitlink)이 있으면 status를 돌리지 않는다 — 서브모듈 작업 트리가 깨끗한지 보려면
/// git이 그 안에서 서브모듈 저장소 자신의 설정(filter 등)을 따르므로, 깨끗함을 증명할 수 없어 `Err`로
/// 이유를 알린다.
fn git_revision(project: &Path) -> Result<Option<String>, Unverified> {
    // index만 읽는다 — 작업 트리 파일을 읽지 않아 filter를 거치지 않는다.
    let Some(entries) = git(project, &["ls-files", "--stage", "-z"])? else {
        return Ok(None);
    };
    let submodules = count_gitlinks(&entries);
    if submodules > 0 {
        return Err(Unverified::submodules(submodules));
    }
    let status = [
        "status",
        "--porcelain=v1",
        "--untracked-files=normal",
        "--ignore-submodules=all",
    ];
    let Some(changes) = git(project, &status)? else {
        return Ok(None);
    };
    if !changes.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    let Some(head) = git(project, &["rev-parse", "--verify", "HEAD"])? else {
        return Ok(None);
    };
    Ok(object_id(&head))
}

/// `git rev-parse` 출력이 커밋 id(소문자 16진 40자 SHA-1 또는 64자 SHA-256)면 그 문자열이다.
fn object_id(output: &[u8]) -> Option<String> {
    let id = std::str::from_utf8(output).ok()?.trim();
    let hex = id
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
    (hex && (id.len() == 40 || id.len() == 64)).then(|| id.to_string())
}

/// `git ls-files --stage -z` 출력에서 gitlink(mode 160000, 서브모듈) 경로 수를 센다. 레코드는
/// `<mode> <object> <stage>\t<path>`이고, 충돌 중인 gitlink는 stage마다 한 줄이라 경로로 센다.
fn count_gitlinks(entries: &[u8]) -> usize {
    entries
        .split(|&b| b == 0)
        .filter(|entry| entry.starts_with(b"160000 "))
        .filter_map(|entry| {
            let tab = entry.iter().position(|&b| b == b'\t')?;
            Some(&entry[tab + 1..])
        })
        .collect::<BTreeSet<&[u8]>>()
        .len()
}

/// 분석 대상 작업 트리에서 git 명령 하나를 실행한다. 분석 대상 `.git`(과 사용자·시스템 설정)이 정한
/// 명령을 이 읽기 전용 조회가 실행하지 않게 격리한다(DECISIONS D1): 바로 앞에서 설정된 filter 드라이버
/// 이름을 다시 모아(`filter_driver_names`) 이름마다 명령을 빈 값으로 덮는다(`analysis_command`).
/// 조회를 호출마다 다시 하는 것은 앞선 호출 뒤에 바뀐 설정도 따라가기 위해서다. 실행할 git도 분석 대상
/// 파일이 고르지 못하게 PATH의 절대 경로 항목에서만 찾는다(`git_program`). 모든 git 호출이 이 함수를
/// 지난다 — 호출마다 따로 쓰면 한쪽만 설정의 명령을 실행하는 어긋남이 생긴다.
/// `Ok(Some)`은 성공한 명령의 표준 출력, `Ok(None)`은 git 없음·저장소 아님·명령이나 설정 조회 실패
/// (revision이 없다는 것이 신호다), `Err`는 덮을 수 없는 드라이버 이름이 있어 실행하지 않은 경우다
/// (fail-closed).
fn git(project: &Path, args: &[&str]) -> Result<Option<Vec<u8>>, Unverified> {
    let Some(program) = git_program() else {
        return Ok(None);
    };
    let Some(drivers) = filter_driver_names(&program, project)? else {
        return Ok(None);
    };
    Ok(run_git(analysis_command(&program, project, &drivers, args)))
}

/// PATH가 없을 때 git을 찾는 디렉터리 — 이름만 넘긴 `Command::new("git")`은 libc의 기본 탐색 경로를
/// 썼다(macOS `_PATH_DEFPATH`는 `/usr/bin:/bin`). 같은 두 디렉터리를 계열 kartograph·pythograph와 같은
/// 순서로 본다. Windows에는 기본값이 없다.
#[cfg(unix)]
const DEFAULT_SEARCH_PATH: Option<&str> = Some("/bin:/usr/bin");
#[cfg(not(unix))]
const DEFAULT_SEARCH_PATH: Option<&str> = None;

/// 실행할 git의 절대 경로 — PATH(없으면 `DEFAULT_SEARCH_PATH`)의 절대 경로 항목에서 처음 찾은 실행
/// 파일이다. 찾지 못하면 None이다(git이 없는 것과 같다).
fn git_program() -> Option<PathBuf> {
    find_git(std::env::var_os("PATH").as_deref(), is_executable_file)
}

/// PATH 값(`None`은 PATH 없음)에서 git 실행 파일을 찾는다 — 셸처럼 앞 항목이 이긴다. 빈 항목과 `.`·`bin`
/// 같은 상대 항목은 건너뛴다. git을 `current_dir(project)`로 띄우므로, 이름만 넘기면 unix std는 자식이
/// 작업 디렉터리를 옮긴 뒤 PATH를 찾아 그런 항목이 분석 대상 디렉터리 기준으로 풀린다 — 저장소에 심은
/// `git`이 실행된다(macOS에서 빈 항목·`.`·`bin`으로 확인했다). Windows std는 부모의 현재 디렉터리
/// 기준으로 풀어 체크아웃 안에서 띄우면 같은 일이 생긴다. 그래서 찾은 절대 경로로 띄운다.
/// `is_executable`은 후보 검사다(테스트가 파일 시스템 대신 주입한다).
fn find_git(path: Option<&OsStr>, is_executable: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    let path = path.or(DEFAULT_SEARCH_PATH.map(OsStr::new))?;
    let file = format!("git{}", std::env::consts::EXE_SUFFIX);
    std::env::split_paths(path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(&file))
        .find(|candidate| is_executable(candidate))
}

/// 실행할 수 있는 일반 파일인가(링크는 따라간다) — unix는 실행 비트까지 본다. 없거나 읽을 수 없거나
/// 실행할 수 없는 후보는 execvp처럼 건너뛴다(오류가 아니라 다음 PATH 항목을 볼 이유다).
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

/// git이 이 디렉터리에서 읽는 설정 전체(시스템·사용자·저장소·worktree·include·`-c`)에 정의된 filter
/// 드라이버 이름을 모은다. `Ok(None)`은 조회 실패다(git 없음, 설정 오류 — 그때는 분석용 git도 이
/// 저장소에서 돌지 않는다). 덮을 수 없는 이름이 하나라도 있으면 `Err`다 — 그 드라이버는 실행될 수 있다.
/// D1 문구의 `--get-regexp` 대신 이름 목록 전체를 바이트로 읽는다: UTF-8 로캘의 git 정규식은 깨진
/// UTF-8 이름을 결과에서 빼지만 속성 대조는 바이트로 해 그 드라이버를 실행한다(계열 isthmus 실험).
fn filter_driver_names(program: &Path, project: &Path) -> Result<Option<Vec<String>>, Unverified> {
    let mut listing = git_command(program, project);
    listing.args(["config", "-z", "--includes", "--name-only", "--list"]);
    match run_git(listing) {
        Some(keys) => parse_filter_driver_names(&keys).map(Some),
        None => Ok(None),
    }
}

/// `git config -z --name-only --list` 출력(키마다 NUL로 끝난다)에서 드라이버 이름을 정렬·중복 제거해
/// 뽑는다 — 정렬은 `-c` 인자 순서를 결정적으로 만든다. 덮을 수 없는 이름이 있으면 `Err`다.
fn parse_filter_driver_names(keys: &[u8]) -> Result<Vec<String>, Unverified> {
    let mut drivers = BTreeSet::new();
    for key in keys.split(|&b| b == 0) {
        let Some(name) = filter_driver_name(key) else {
            continue;
        };
        let name = std::str::from_utf8(name)
            .ok()
            .filter(|n| can_override(n))
            .ok_or_else(Unverified::driver_name)?;
        drivers.insert(name.to_string());
    }
    Ok(drivers.into_iter().collect())
}

/// 설정 키가 filter 드라이버의 명령 키(`filter.<이름>.clean|smudge|process`)면 그 이름(바이트)이다.
/// 이름은 `filter.`과 마지막 `.<키>` 사이 전체다 — 점이 들어갈 수 있고 빈 이름(`[filter ""]`)도 git이
/// 쓴다. git은 섹션·변수 이름을 소문자로 주지만 둘 다 대소문자를 가리지 않는 이름이라 그렇게 비교한다.
/// 이름(subsection)은 대소문자를 가린다. subsection 없는 `filter.clean`은 드라이버가 아니다.
fn filter_driver_name(key: &[u8]) -> Option<&[u8]> {
    const SECTION: &[u8] = b"filter.";
    if !key.get(..SECTION.len())?.eq_ignore_ascii_case(SECTION) {
        return None;
    }
    let rest = &key[SECTION.len()..];
    let dot = rest.iter().rposition(|&b| b == b'.')?;
    let variable = &rest[dot + 1..];
    ["clean", "smudge", "process"]
        .iter()
        .any(|command| variable.eq_ignore_ascii_case(command.as_bytes()))
        .then_some(&rest[..dot])
}

/// 이름을 `-c filter.<이름>.<키>=`로 덮을 수 있는가 — git은 `-c`를 첫 `=`에서 키와 값으로 나누므로
/// `=`가 든 이름은 다른 키가 된다. 제어 문자(와 U+2028/2029)가 든 이름은 정상 설정에서 나오지 않아
/// 인자로 옮기지 않는다(id와 같은 금지 문자 판정).
fn can_override(name: &str) -> bool {
    !name.contains('=') && !has_forbidden(name)
}

/// 모든 git 호출(드라이버 조회 포함)의 공통 부분 — 분석 대상 설정이 명령을 실행하거나 저장소를 고치지
/// 못하게 끈다: fsmonitor 훅, untracked cache 기록, log 계열의 서명 확인(gpg 실행). 선택 잠금을 꺼
/// index를 다시 쓰지 않고(그래서 `post-index-change` 훅도 돌지 않는다), 프롬프트를 막고 메시지를 C
/// 로캘로 고정한다. `GIT_CONFIG`는 `git config`만 따르는 옛 변수라(그 파일만 읽는다) 드라이버 조회가
/// 분석용 명령과 다른 설정을 보지 않게 뺀다. `program`은 `git_program`이 찾은 절대 경로다.
fn git_command(program: &Path, project: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "-c",
            "log.showSignature=false",
        ])
        .current_dir(project)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .env_remove("GIT_CONFIG")
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    command
}

/// 분석용 git 명령 — 공통 부분 뒤에 드라이버마다 명령 세 키를 빈 값으로, required를 false로 덮는다.
/// 빈 명령은 git이 실행하지 않고(filter 없이 원문을 쓴다), required=false라야 "filter 실패"로 끝나지
/// 않는다. `--attr-source`는 `.git/info/attributes`를 막지 못해 쓰지 않는다.
fn analysis_command(program: &Path, project: &Path, drivers: &[String], args: &[&str]) -> Command {
    let mut command = git_command(program, project);
    for name in drivers {
        for setting in ["clean=", "smudge=", "process=", "required=false"] {
            command.arg("-c").arg(format!("filter.{name}.{setting}"));
        }
    }
    command.args(args);
    command
}

/// 명령을 실행해 성공하면 표준 출력을 돌려준다 — git이 없거나 실패하면 None이다.
fn run_git(mut command: Command) -> Option<Vec<u8>> {
    let out = command.output().ok()?;
    out.status.success().then_some(out.stdout)
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
            "2026-09-30T00:00:00+99:99",
            "2026-09-30T00:00:00-00:60",
            "2026-09-30T00:00:00+24:00",
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

    /// 옛 graphRevision — 문서를 복제해 root를 바꾸고 pretty JSON 전체를 만든 뒤 해시했다.
    /// 해시를 흘려 계산하는 지금 구현이 같은 바이트를 해시하는지 재는 기준이다.
    fn graph_revision_by_copy(doc: &Document, project: &Path) -> String {
        let mut canonical = doc.clone();
        canonical.root = project.display().to_string();
        traversal::sha256::hex_digest(export::to_json(&canonical).as_bytes())
    }

    /// 선택 필드를 모두 비운 문서와 모두 채운 문서 — JSON 이스케이프가 필요한 문자를 섞는다.
    fn sample_documents() -> Vec<Document> {
        use crate::graph::{document, Edge, EdgeKind, Kind, Level};
        let bare = document(
            Level::Module,
            ".".into(),
            None,
            vec![],
            vec![],
            vec![],
            vec![],
        );
        let full_vertex = Vertex {
            id: "c::m::\"q\"\\é😀".into(),
            kind: Kind::Method,
            krate: "c".into(),
            module: "c::m".into(),
            position: Some("src/lib.rs:7".into()),
            exported: true,
            generated: true,
            cfg: Some("feature = \"x\"".into()),
            unsafe_: true,
        };
        let plain_vertex = Vertex {
            id: "c::main".into(),
            kind: Kind::Fn,
            krate: "c".into(),
            module: "c".into(),
            position: None,
            exported: false,
            generated: false,
            cfg: None,
            unsafe_: false,
        };
        let mut odd_edge = Edge::maybe(
            "c::main".into(),
            full_vertex.id.clone(),
            EdgeKind::Signature,
        );
        odd_edge.cfg = Some("unix".into());
        odd_edge.unsafe_ = true;
        let full = document(
            Level::Symbol,
            "/elsewhere".into(),
            Some("/ws \"root\"".into()),
            vec!["c::main".into()],
            vec![full_vertex.clone(), plain_vertex],
            vec![
                Edge::new("c::main".into(), full_vertex.id.clone(), EdgeKind::Call),
                odd_edge,
            ],
            vec!["limit: \"x\"\n\ttab\u{1}".into()],
        );
        vec![bare, full]
    }

    #[test]
    fn graph_revision_hashes_the_same_bytes_as_the_cloned_document_json() {
        let project = Path::new("/tmp/project \"p\"/é");
        for doc in sample_documents() {
            assert_eq!(
                graph_revision(&doc, project),
                graph_revision_by_copy(&doc, project)
            );
        }
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

    #[test]
    fn filter_driver_names_come_from_every_command_key() {
        // git이 주는 키 목록 그대로 — 점이 든 이름, 대소문자를 가리는 이름, 빈 이름, 같은 드라이버의
        // 여러 키를 모은다. subsection 없는 `filter.clean`, 명령 키가 아닌 `required`·`cleanup`,
        // 다른 섹션은 드라이버가 아니다.
        let keys = b"core.bare\0filter.lfs.clean\0filter.lfs.smudge\0filter.lfs.process\0\
filter.lfs.required\0filter.evil.name.clean\0filter.Evil.process\0filter..smudge\0\
filter.clean\0filter.x.cleanup\0myfilter.y.clean\0FILTER.Up.CLEAN\0";
        let drivers = parse_filter_driver_names(keys).ok().unwrap();
        assert_eq!(drivers, ["", "Evil", "Up", "evil.name", "lfs"]);
        assert_eq!(
            parse_filter_driver_names(b"").ok().unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn filter_driver_names_git_cannot_override_are_refused() {
        // `=`는 -c가 키를 가르는 자리이고, 깨진 UTF-8·제어 문자(NEL 포함)는 인자로 옮기지 않는다.
        for keys in [
            &b"filter.ok.clean\0filter.a=b.clean\0"[..],
            b"filter.\xff\xfe.smudge\0",
            b"filter.a\tb.process\0",
            b"filter.a\xc2\x85b.clean\0",
        ] {
            let Err(Unverified(note)) = parse_filter_driver_names(keys) else {
                panic!("{keys:?} must be refused");
            };
            assert!(note.starts_with("revision-unverified: "), "{note}");
            assert!(!note.contains("a=b"), "the driver name is not echoed");
        }
        // 명령 키가 아니면 이름이 이상해도 드라이버가 아니다.
        assert!(parse_filter_driver_names(b"filter.a=b.required\0").is_ok());
    }

    #[test]
    fn analysis_commands_override_every_listed_driver() {
        let drivers = ["evil.name".to_string(), String::new()];
        let program = Path::new("/opt/git/bin/git");
        let command = analysis_command(program, Path::new("/p"), &drivers, &["status"]);
        // 이름 `git`이 아니라 `git_program`이 고른 절대 경로를 띄운다.
        assert_eq!(command.get_program(), program.as_os_str());
        let args: Vec<&str> = command.get_args().map(|a| a.to_str().unwrap()).collect();
        let mut expected = vec![
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.untrackedCache=false",
            "-c",
            "log.showSignature=false",
        ];
        for setting in [
            "filter.evil.name.clean=",
            "filter.evil.name.smudge=",
            "filter.evil.name.process=",
            "filter.evil.name.required=false",
            "filter..clean=",
            "filter..smudge=",
            "filter..process=",
            "filter..required=false",
        ] {
            expected.extend(["-c", setting]);
        }
        expected.push("status");
        assert_eq!(args, expected);
        assert_eq!(command.get_current_dir(), Some(Path::new("/p")));
        let envs: BTreeMap<_, _> = command.get_envs().collect();
        let value = |k: &str| envs.get(std::ffi::OsStr::new(k)).copied();
        assert_eq!(
            value("GIT_OPTIONAL_LOCKS"),
            Some(Some(std::ffi::OsStr::new("0")))
        );
        assert_eq!(value("LC_ALL"), Some(Some(std::ffi::OsStr::new("C"))));
        // 드라이버 조회가 분석용 명령과 같은 설정을 읽도록 `git config` 전용 변수를 뺀다.
        assert_eq!(value("GIT_CONFIG"), Some(None));
    }

    #[test]
    fn git_is_found_only_in_absolute_path_entries() {
        // 상대 항목이 분석 대상 디렉터리 기준으로 풀리는 실제 실행은 tests/traversal.rs가 본다. 여기서는
        // 어떤 후보를 어떤 순서로 보는지만 본다 — 파일 검사를 주입해 본 후보를 적는다.
        let base = std::env::temp_dir();
        let (first, second) = (base.join("first"), base.join("second"));
        let exe_in = |dir: &Path| dir.join(format!("git{}", std::env::consts::EXE_SUFFIX));
        let entries = [
            Path::new(""),
            Path::new("."),
            Path::new("bin"),
            &first,
            &second,
        ];
        let path = std::env::join_paths(entries).unwrap();
        let checked = std::cell::RefCell::new(Vec::new());
        let found = find_git(Some(&path), |candidate| {
            checked.borrow_mut().push(candidate.to_path_buf());
            candidate == exe_in(&second)
        });
        // 빈 항목·`.`·`bin`은 후보로 보지도 않고, 앞 항목이 이긴다.
        assert_eq!(found, Some(exe_in(&second)));
        assert_eq!(checked.into_inner(), [exe_in(&first), exe_in(&second)]);
        let relative_only = std::env::join_paths([Path::new("."), Path::new("bin")]).unwrap();
        assert_eq!(find_git(Some(&relative_only), |_| true), None);
        // PATH가 있지만 비어 있으면 빈 항목 하나다 — 기본 디렉터리로 돌아가지 않는다.
        assert_eq!(find_git(Some(OsStr::new("")), |_| true), None);
    }

    #[cfg(unix)]
    #[test]
    fn git_search_without_path_uses_the_libc_default_directories() {
        let checked = std::cell::RefCell::new(Vec::new());
        let found = find_git(None, |candidate| {
            checked.borrow_mut().push(candidate.to_path_buf());
            false
        });
        assert_eq!(found, None);
        let defaults = [PathBuf::from("/bin/git"), PathBuf::from("/usr/bin/git")];
        assert_eq!(checked.into_inner(), defaults);
    }

    #[cfg(unix)]
    #[test]
    fn git_candidates_are_executable_regular_files() {
        use std::os::unix::fs::PermissionsExt;
        let base =
            std::env::temp_dir().join(format!("rustograph-git-candidate-{}", std::process::id()));
        if base.exists() {
            // 같은 pid로 중단된 이전 실행의 잔재 — 링크를 다시 만들 수 있게 비운다.
            std::fs::remove_dir_all(&base).unwrap();
        }
        std::fs::create_dir_all(base.join("dir/git")).unwrap();
        let program = base.join("git");
        std::fs::write(&program, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!is_executable_file(&program), "no execute bit");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(is_executable_file(&program));
        let alias = base.join("alias");
        std::os::unix::fs::symlink(&program, &alias).unwrap();
        assert!(is_executable_file(&alias), "links are followed");
        assert!(!is_executable_file(&base.join("dir/git")), "a directory");
        assert!(!is_executable_file(&base.join("missing")));
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn gitlinks_are_counted_once_per_path() {
        // 충돌 중인 gitlink는 stage마다 한 줄이다. 경로가 숫자로 시작하는 일반 파일은 gitlink가 아니다.
        let records = [
            "100644 aaaa 0\tsrc/lib.rs",
            "160000 bbbb 0\tvendored",
            "160000 cccc 1\tclash",
            "160000 dddd 3\tclash",
            "100644 eeee 0\t160000 x",
        ];
        let entries: Vec<u8> = records.iter().flat_map(|r| r.bytes().chain([0])).collect();
        assert_eq!(count_gitlinks(&entries), 2);
        assert_eq!(count_gitlinks(b""), 0);
    }

    #[test]
    fn object_ids_are_full_lowercase_hex() {
        let sha1 = "0123456789abcdef0123456789abcdef01234567";
        assert_eq!(
            object_id(format!("{sha1}\n").as_bytes()).as_deref(),
            Some(sha1)
        );
        assert_eq!(object_id(&[b'a'; 64]).map(|id| id.len()), Some(64));
        for bad in [&b"0123ABCDEF"[..], b"", b"\xff", &[b'a'; 39]] {
            assert_eq!(object_id(bad), None, "{bad:?}");
        }
    }
}
