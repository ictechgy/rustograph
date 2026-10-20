//! 명령행 인터페이스 — 인자 해석과 종료 코드 계약.
//!
//! 종료 코드: 0 정상, 1 strict 위반 발견, 2 사용법/분석 오류.
//! 플래그 파서는 외부 크레이트 없이 직접 만든다 — 명령이 적고 계약이 단순해서다.

mod reach;

use crate::cli_args::{self, Args};
use crate::{analysis, config, deps, export, mcp, rules, sarif, source, traversal};
use std::io::Write;
use std::path::{Path, PathBuf};

/// CLI가 스스로 보고하는 버전 — Cargo 패키지 버전과 같다.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

const USAGE: &str = "\
rustograph — queryable dependency graph for Rust codebases

usage:
  rustograph graph [--level crate|module|type|symbol] [--format json|mermaid]
                   [--dir DIR] [--deps] [--tests] [--out FILE]
  rustograph cycles [--level L] [--strict] [--format text|json]
  rustograph dead [--strict] [--explain ID] [--root ID] [--retain-public] [--tests]
  rustograph rules [--strict] [--format text|json|sarif] [--config FILE]
                   [--baseline FILE] [--write-baseline]
  rustograph deps [--strict] [--format text|json]
  rustograph query ID [--depth N] [--max N]
  rustograph impact ID [--depth N] [--max N]
  rustograph reach ID... [--roots-from FILE|-] [--max-depth N] [--max-reached N]
                   [--revision REV] [--generated-at TIMESTAMP]
  rustograph impact --format language-traversal ID... (same options as reach)
  rustograph paths FROM TO [--max N] [--budget N]
  rustograph search QUERY [--max N]
  rustograph mcp [--dir DIR] [--graph FILE] [--config FILE] [--deps] [--tests]
  rustograph schema [--dir DIR] [--out FILE]
  rustograph routes --role server [--dir DIR] [--out FILE] [--framework axum|actix]
  rustograph routes --role client [--dir DIR] [--out FILE] [--wrappers FILE]
                   [--service NAME]
  rustograph version | --version

shared flags: --deps --tests --retain-public --semantic --no-cache
  --focus PATH --target TRIPLE --exclude-tests
  --semantic resolves method calls and macro expansions with rust-analyzer
  semantics (requires a build with `--features semantic`; results cached
  in .rustograph/semantic-cache.json unless --no-cache)

query / impact --depth 0 walks without a depth limit (the whole closure, still
bounded by --max); the report keeps \"depth\": 0.

reach / impact --format language-traversal emit an isthmus language-traversal
v1 document (dependencies / dependents) for many roots in one pass; ids that
are not graph vertices are listed without symbol and the command exits 64.

exit codes: 0 ok · 1 strict violation found · 2 usage/analysis error
  (traversal output: 64 usage error with empty stdout, or root-not-found)";

/// 명령을 실행하고 종료 코드를 돌려준다.
/// os::exit 대신 반환값을 쓰는 것은 종료 코드 계약을 테스트하기 위함이다.
pub fn run(args: &[String], stdout: &mut dyn Write, stderr: &mut dyn Write) -> i32 {
    match run_inner(args, stdout, stderr) {
        Ok(code) => code,
        Err(e) => {
            let _ = writeln!(stderr, "error: {e}");
            2
        }
    }
}

fn run_inner(args: &[String], out: &mut dyn Write, err: &mut dyn Write) -> Result<i32, String> {
    let a = match cli_args::parse(args) {
        Ok(a) => a,
        // 순회 명령은 계열 계약대로 사용법 오류가 64다 — 전역 파서 실패도 마찬가지.
        Err(e) if reach::is_traversal_argv(args) => {
            let _ = writeln!(err, "error: {e}");
            return Ok(reach::EXIT_USAGE);
        }
        Err(e) => return Err(format!("{e}\n{USAGE}")),
    };
    match a.cmd.as_str() {
        // `--version`은 계열 관례(isthmus verify-installed-compatibility가
        // `<tool> --version`의 마지막 semver를 읽는다)를 위한 별칭이다 — 출력은 하위 명령과 같다.
        "version" | "--version" => {
            writeln!(out, "rustograph {VERSION}").ok();
            Ok(0)
        }
        "graph" => cmd_graph(&a, out),
        "cycles" => cmd_cycles(&a, out),
        "dead" => cmd_dead(&a, out),
        "rules" => cmd_rules(&a, out),
        "deps" => cmd_deps(&a, out),
        "query" => cmd_query(&a, out, false),
        "impact" if a.get("format") == Some("language-traversal") => {
            cmd_traversal(&a, traversal::Direction::Dependents, out, err)
        }
        "impact" => cmd_query(&a, out, true),
        "reach" => cmd_traversal(&a, traversal::Direction::Dependencies, out, err),
        "paths" => cmd_paths(&a, out),
        "search" => cmd_search(&a, out),
        "schema" => cmd_schema(&a, out),
        "routes" => cmd_routes(&a, out),
        "mcp" => mcp::cmd(&a, &mut std::io::stdin().lock(), out, err),
        "-h" | "--help" | "help" => {
            writeln!(out, "{USAGE}").ok();
            Ok(0)
        }
        other => Err(format!("unknown command {other}\n{USAGE}")),
    }
}

/// `reach`·`impact --format language-traversal` — 순회 문서를 낸다.
/// 사용법 오류는 표준 출력을 비운 채 64, 수확 실패는 2다.
fn cmd_traversal(
    a: &Args,
    direction: traversal::Direction,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<i32, String> {
    match reach::cmd(a, VERSION, direction, &mut std::io::stdin().lock(), out) {
        Ok(code) => {
            if code == reach::EXIT_USAGE {
                let _ = writeln!(
                    err,
                    "error: some roots are not rustograph graph vertices (root-not-found); the document lists them without symbol"
                );
            }
            Ok(code)
        }
        Err(reach::Failure::Usage(m)) => {
            let _ = writeln!(err, "error: {m}");
            Ok(reach::EXIT_USAGE)
        }
        Err(reach::Failure::Analysis(m)) => Err(m),
    }
}

/// `schema` — isthmus persistence 도메인의 bridge-facts 문서를 낸다.
/// 그래프 문서가 아니라 교환 문서라 export 직렬화만 재사용한다.
fn cmd_schema(a: &Args, out: &mut dyn Write) -> Result<i32, String> {
    // 교환 문서는 그래프 옵션(--semantic·--graph·--level 등)과 무관하다 —
    // 조용히 무시하면 사용자가 옵션이 적용됐다고 오해한다.
    if let Some(bad) = a.unsupported(&["dir", "out"]) {
        return Err(format!(
            "schema takes only --dir and --out — {bad} is not supported"
        ));
    }
    let dir = PathBuf::from(a.get("dir").unwrap_or("."));
    let doc = source::schema::facts(&dir, VERSION)?;
    let text = export::to_json(&doc);
    match a.get("out") {
        Some(p) => {
            write_output(a, Path::new(p), &text, &[], None)?;
            writeln!(out, "wrote {p}").ok();
        }
        None => {
            write!(out, "{text}").ok();
        }
    }
    Ok(0)
}

/// `routes --role server|client` — isthmus http 도메인 문서를 낸다. 서버는 axum·
/// actix-web `route-decl`, 클라이언트는 reqwest·ureq·선언된 래퍼의 `route-call`이다.
/// `--role`은 필수다 — 생략을 한쪽으로 읽으면 명령의 뜻이 역할에 따라 흔들린다.
fn cmd_routes(a: &Args, out: &mut dyn Write) -> Result<i32, String> {
    let text = match a.get("role") {
        Some("server") => routes_server(a)?,
        Some("client") => routes_client(a)?,
        Some(other) => {
            return Err(format!(
                "routes --role {other} is not supported; use --role server or --role client"
            ))
        }
        None => return Err("routes requires --role server or --role client".to_string()),
    };
    match a.get("out") {
        Some(p) => {
            write_output(a, Path::new(p), &text, &[], None)?;
            writeln!(out, "wrote {p}").ok();
        }
        None => {
            write!(out, "{text}").ok();
        }
    }
    Ok(0)
}

/// 서버 route-decl 문서(JSON 텍스트).
fn routes_server(a: &Args) -> Result<String, String> {
    if let Some(bad) = a.unsupported(&["dir", "out", "role", "framework"]) {
        return Err(format!(
            "routes --role server takes only --dir, --out and --framework — {bad} is not supported"
        ));
    }
    let framework = match a.get("framework") {
        None => None,
        Some(f) => Some(
            source::routes::Framework::parse(f)
                .ok_or_else(|| format!("unknown --framework {f} (axum|actix)"))?,
        ),
    };
    let dir = PathBuf::from(a.get("dir").unwrap_or("."));
    let opts = source::routes::RouteOptions { framework };
    let doc = source::routes::routes(&dir, VERSION, &opts)?;
    Ok(export::to_json(&doc))
}

/// 클라이언트 route-call 문서(JSON 텍스트).
fn routes_client(a: &Args) -> Result<String, String> {
    if let Some(bad) = a.unsupported(&["dir", "out", "role", "wrappers", "service"]) {
        return Err(format!(
            "routes --role client takes only --dir, --out, --wrappers and --service — {bad} is not supported"
        ));
    }
    let wrappers = match a.get("wrappers") {
        None => Vec::new(),
        Some(p) => {
            let text = std::fs::read_to_string(p)
                .map_err(|e| format!("cannot read --wrappers {p}: {e}"))?;
            source::routes::wrappers::parse(&text).map_err(|e| format!("--wrappers {p}: {e}"))?
        }
    };
    let service = match a.get("service") {
        Some(s) if s.is_empty() || s.chars().any(char::is_control) => {
            return Err("--service must be a non-empty name without control characters".into())
        }
        other => other.map(str::to_string),
    };
    let dir = PathBuf::from(a.get("dir").unwrap_or("."));
    let opts = source::routes::client::ClientOptions { wrappers, service };
    let doc = source::routes::client::client_routes(&dir, VERSION, &opts)?;
    Ok(export::to_json(&doc))
}

/// 출력 형식과 무관하게 그래프 입력과 수확 원본을 보호한다. `harvested`는 결과 문서를 만든
/// 수확이 읽은 입력이다 — 없으면(schema·routes, semantic 캐시 적중) 보호 목록을 위해
/// 모듈 트리를 다시 찾는다.
fn write_output(
    a: &Args,
    path: &Path,
    text: &str,
    extra: &[PathBuf],
    harvested: Option<&source::HarvestInputs>,
) -> Result<(), String> {
    let mut protected = extra.to_vec();
    let directory = Path::new(a.get("dir").unwrap_or("."));
    protected.extend(
        [".rustograph.yml", ".rustograph.yaml"]
            .into_iter()
            .map(|name| directory.join(name)),
    );
    for flag in ["graph", "wrappers", "config", "roots-from"] {
        if let Some(input) = a.get(flag) {
            protected.push(PathBuf::from(input));
        }
    }
    if a.get("graph").is_none() {
        let listed = match harvested {
            Some(inputs) => inputs.protected_files(),
            None => source::input_files(directory)?,
        };
        if path.exists() {
            if let Some(refusal) = replacement_refusal(&listed) {
                return Err(refusal);
            }
        }
        protected.extend(listed.files);
    }
    export::write_file(path, text.as_bytes(), &protected)
}

/// 보호 목록에 빠진 입력이 있을 수 있으면 기존 출력을 교체하지 않는 이유 — 기존 파일이
/// 그 빠진 입력일 수 있다. 새 경로와 표준 출력은 덮어쓸 원본이 없어 막지 않는다.
fn replacement_refusal(listed: &source::ProtectedInputs) -> Option<String> {
    if let Some(first) = listed.unloaded_modules.first() {
        return Some(format!(
            "cannot safely replace an existing output: {} module file(s) could not be read or parsed (first: {}), so the modules and include inputs they declare are unknown; fix them, choose a new output path or write to stdout",
            listed.unloaded_modules.len(),
            first.display()
        ));
    }
    listed.unresolved_includes.then(|| {
        "cannot safely replace an existing output with unresolved include paths; choose a new output path or write to stdout".to_string()
    })
}

fn cmd_graph(a: &Args, out: &mut dyn Write) -> Result<i32, String> {
    let level = cli_args::level_of(a)?;
    let format = a.get("format").unwrap_or("json");
    // 수확은 항상 최대 깊이로 — 얕은 레벨은 투영이다.
    let (doc, inputs) = cli_args::document_with_inputs(a, true, a.get("out").is_some())?;
    let mut doc = doc.view(level)?;
    // `--graph` 입력은 그 문서의 기록을 그대로 옮긴다(다시 판독하지 않는다).
    if a.get("graph").is_none() {
        doc.revision = recorded_revision(a, format);
    }
    let text = match format {
        "json" => export::to_json(&doc),
        "mermaid" => export::to_mermaid(&doc),
        f => return Err(format!("unknown format {f} (json|mermaid)")),
    };
    match a.get("out") {
        Some(p) => {
            let path = Path::new(p);
            write_output(
                a,
                path,
                &text,
                &export::document_inputs(&doc),
                inputs.as_ref(),
            )?;
            writeln!(out, "wrote {p}").ok();
        }
        None => {
            write!(out, "{text}").ok();
        }
    }
    Ok(0)
}

/// 수확한 그래프에 기록할 출처 revision — 파일로 저장하는 JSON 스냅샷(`--out`)에만 싣는다. 순회 명령이
/// `--graph`로 그 스냅샷을 읽을 때 지금의 HEAD 대신 이 값을 쓴다. 표준 출력 문서는 지금과 같은 바이트라
/// git을 실행하지 않고, Mermaid에는 실을 자리가 없다. 값은 `--dir` 작업 트리가 깨끗할 때의 git HEAD다
/// (순회 문서의 revision과 같은 판정). 캐시에서 온 문서도 이 값으로 덮어 수확 경로가 출처를 정한다.
fn recorded_revision(a: &Args, format: &str) -> Option<String> {
    if format != "json" || a.get("out").is_none() {
        return None;
    }
    reach::clean_head(Path::new(a.get("dir").unwrap_or(".")))
}

fn cmd_cycles(a: &Args, out: &mut dyn Write) -> Result<i32, String> {
    let level = cli_args::level_of(a)?;
    let doc = cli_args::document_for(a, true)?.view(level)?;
    let rep = analysis::cycles(&doc);
    match a.get("format").unwrap_or("text") {
        "json" => write!(out, "{}", export::to_json(&rep)).ok(),
        "text" => {
            writeln!(out, "{} cycle(s)", rep.cycles.len()).ok();
            for c in &rep.cycles {
                writeln!(out, "  cycle: {}", c.members.join(" -> ")).ok();
            }
            if rep.excluded_tentative > 0 {
                writeln!(
                    out,
                    "  note: {} speculative method-fan-out edges excluded from cycle detection",
                    rep.excluded_tentative
                )
                .ok();
            }
            for l in &doc.limitations {
                writeln!(out, "  limitation: {l}").ok();
            }
            Some(())
        }
        f => return Err(format!("unknown format {f} (text|json)")),
    };
    Ok(if a.has("strict") && !rep.cycles.is_empty() {
        1
    } else {
        0
    })
}

fn cmd_dead(a: &Args, out: &mut dyn Write) -> Result<i32, String> {
    // dead는 항상 심볼 레벨 — 도달성은 심볼에서만 의미 있다.
    let doc = cli_args::document_for(a, true)?;
    if let Some(target) = a.get("explain") {
        let target = require_id(&doc, target)?;
        match analysis::explain_path(&doc, &target) {
            Some(path) => {
                writeln!(out, "{}", path.join(" -> ")).ok();
                return Ok(0);
            }
            None => {
                writeln!(out, "{target}: not reachable from retention roots").ok();
                return Ok(0);
            }
        }
    }
    let report = analysis::dead(&doc)?;
    match a.get("format").unwrap_or("text") {
        "json" => write!(out, "{}", export::to_json(&report)).ok(),
        "text" => {
            for f in &report.findings {
                writeln!(
                    out,
                    "{} {}: {}",
                    f.state,
                    format!("{:?}", f.kind).to_lowercase(),
                    f.id
                )
                .ok();
            }
            writeln!(out, "{} unreachable", report.findings.len()).ok();
            for l in report.limitations.iter().chain(doc.limitations.iter()) {
                writeln!(out, "limitation: {l}").ok();
            }
            Some(())
        }
        f => return Err(format!("unknown format {f} (text|json)")),
    };
    Ok(if a.has("strict") && !report.findings.is_empty() {
        1
    } else {
        0
    })
}

fn cmd_rules(a: &Args, out: &mut dyn Write) -> Result<i32, String> {
    let dir = PathBuf::from(a.get("dir").unwrap_or("."));
    // --config이 주어지면 기준선 상대 경로의 기준은 그 파일의 디렉터리다.
    let cfg_dir = a
        .get("config")
        .map(|f| Path::new(f).parent().unwrap_or(dir.as_path()).to_path_buf())
        .unwrap_or_else(|| dir.clone());
    let cfg: config::Config = match a.get("config") {
        // 명시 --config는 실행한 사람이 고른 입력이라 파이프·프로세스 치환도 읽는다(상한은 같다).
        Some(f) => config::load_file(Path::new(f), config::FileKinds::AnyReadable)?,
        None => match config::load(&dir)? {
            Some(c) => c,
            None => return Err(format!("no .rustograph.yml in {}", dir.display())),
        },
    };
    let (doc, inputs) = cli_args::document_with_inputs(a, true, a.has("write-baseline"))?;
    let mut report = rules::check(&doc, &cfg);
    // 기준선 경로 — --baseline이 cfg.baseline을 이긴다.
    let baseline_path = a
        .get("baseline")
        .map(PathBuf::from)
        .or_else(|| cfg.baseline.as_ref().map(|b| cfg_dir.join(b)));
    // --write-baseline: 현재 위반을 기준선으로 기록하고 끝낸다 —
    // 레거시 도입의 첫 단계는 "기존 위반을 얼리는 것"이다.
    if a.has("write-baseline") {
        let Some(path) = &baseline_path else {
            return Err(
                "--write-baseline needs a baseline path — set `baseline:` in .rustograph.yml \
                 or pass --baseline FILE"
                    .to_string(),
            );
        };
        write_output(
            a,
            path,
            &rules::Baseline::render(&report.violations),
            &export::document_inputs(&doc),
            inputs.as_ref(),
        )?;
        writeln!(
            out,
            "wrote baseline {} ({} violations)",
            path.display(),
            report.violations.len()
        )
        .ok();
        return Ok(0);
    }
    if let Some(path) = &baseline_path {
        // 설정의 baseline:은 분석 대상 저장소가 고른다 — 장치·FIFO는 열지 않는다. 명시
        // --baseline은 실행한 사람이 고른 입력이라 파이프도 읽는다. 둘 다 상한까지만 읽는다.
        let kinds = if a.get("baseline").is_some() {
            config::FileKinds::AnyReadable
        } else {
            config::FileKinds::RegularOnly
        };
        match config::read_bounded_text(path, config::MAX_FILE_BYTES, kinds) {
            Ok(src) => rules::apply_baseline(&mut report, &rules::Baseline::parse(&src)),
            // 명시 --baseline은 없는 파일을 가리키면 오타일 가능성이 높다 — 오류.
            // cfg.baseline은 NotFound만 도입 전으로 본다 — 권한 오류나
            // 디렉터리를 조용히 넘기면 억제가 안 먹힌 채로 지나간다.
            Err(e) if a.get("baseline").is_some() || e.kind() != std::io::ErrorKind::NotFound => {
                return Err(format!("cannot read baseline {}: {e}", path.display()))
            }
            Err(_) => {}
        }
    }
    match a.get("format").unwrap_or("text") {
        "json" => write!(out, "{}", export::to_json(&report)).ok(),
        "sarif" => write!(out, "{}", sarif::rules_sarif(&doc, &report.violations)).ok(),
        "text" => {
            for v in &report.violations {
                writeln!(
                    out,
                    "[{}] {} -> {} (component '{}' may not depend on '{}')",
                    v.rule, v.from, v.to, v.component, v.forbidden
                )
                .ok();
            }
            writeln!(out, "{} violations", report.violations.len()).ok();
            if report.baselined > 0 {
                writeln!(
                    out,
                    "{} violations suppressed by baseline",
                    report.baselined
                )
                .ok();
            }
            if report.stale_baseline > 0 {
                writeln!(
                    out,
                    "{} baseline entries no longer violate — regenerate the baseline",
                    report.stale_baseline
                )
                .ok();
            }
            if report.skipped_tentative > 0 {
                writeln!(
                    out,
                    "{} speculative method-fan-out edges not counted as violations",
                    report.skipped_tentative
                )
                .ok();
            }
            if !report.unmapped.is_empty() {
                writeln!(out, "{} unmapped:", report.unmapped.len()).ok();
                for u in &report.unmapped {
                    writeln!(out, "  {u}").ok();
                }
            }
            Some(())
        }
        f => return Err(format!("unknown format {f} (text|json|sarif)")),
    };
    Ok(if a.has("strict") && !report.violations.is_empty() {
        1
    } else {
        0
    })
}

/// 정점 ID를 정확히 해석한다 — 애매한 문자열을 조용히 맞추지 않고
/// 후보를 열거해 사용자가 다시 지정하게 한다(에이전트 오용 방지).
fn require_id(doc: &crate::graph::Document, id: &str) -> Result<String, String> {
    analysis::resolve_id(doc, id).map_err(|cands| {
        if cands.is_empty() {
            format!("vertex {id} not found")
        } else {
            format!(
                "vertex {id} not found — did you mean:\n  {}",
                cands.join("\n  ")
            )
        }
    })
}

fn cmd_paths(a: &Args, out: &mut dyn Write) -> Result<i32, String> {
    let Some(from) = a.positional.first() else {
        return Err("usage: paths FROM TO".to_string());
    };
    let Some(to) = a.positional.get(1) else {
        return Err("usage: paths FROM TO".to_string());
    };
    let max = cli_args::usize_arg(a, "max", 10)?;
    let budget = cli_args::usize_arg(a, "budget", 50_000)?;
    let doc = cli_args::document_for(a, true)?;
    let from = require_id(&doc, from)?;
    let to = require_id(&doc, to)?;
    let report = analysis::paths(&doc, &from, &to, max, budget);
    write!(out, "{}", export::to_json(&report)).ok();
    Ok(0)
}

fn cmd_search(a: &Args, out: &mut dyn Write) -> Result<i32, String> {
    let Some(q) = a.positional.first() else {
        return Err("usage: search QUERY".to_string());
    };
    let max = cli_args::usize_arg(a, "max", 20)?;
    let doc = cli_args::document_for(a, true)?;
    write!(out, "{}", export::to_json(&analysis::search(&doc, q, max))).ok();
    Ok(0)
}

fn cmd_deps(a: &Args, out: &mut dyn Write) -> Result<i32, String> {
    // deps는 자체 syn 수확으로 문서를 만든다 — 문서 필터나 저장
    // 그래프를 받으면 사용 증거가 지워져 거짓 미사용이 나온다.
    for flag in [
        "graph",
        "focus",
        "target",
        "exclude-tests",
        "tests",
        "retain-public",
        "root",
        "semantic",
        "no-cache",
        "level",
        "depth",
    ] {
        if a.get(flag).is_some() || a.has(flag) {
            return Err(format!(
                "deps needs the complete workspace graph — --{flag} is not supported"
            ));
        }
    }
    let dir = PathBuf::from(a.get("dir").unwrap_or("."));
    let report = deps::report(&dir)?;
    match a.get("format").unwrap_or("text") {
        "json" => write!(out, "{}", export::to_json(&report)).ok(),
        "text" => {
            for u in &report.unused {
                writeln!(
                    out,
                    "unused: {} -> {} (kind: {}{})",
                    u.package,
                    u.dep,
                    u.kind,
                    if u.proc_macro { ", proc-macro" } else { "" }
                )
                .ok();
            }
            for d in &report.duplicates {
                writeln!(
                    out,
                    "duplicate: {} has versions {}",
                    d.name,
                    d.versions.join(", ")
                )
                .ok();
            }
            for l in &report.limitations {
                writeln!(out, "limitation: {l}").ok();
            }
            writeln!(
                out,
                "{} unused, {} duplicated",
                report.unused.len(),
                report.duplicates.len()
            )
            .ok()
        }
        f => return Err(format!("unknown --format {f}")),
    };
    Ok(
        if a.has("strict") && (!report.unused.is_empty() || !report.duplicates.is_empty()) {
            1
        } else {
            0
        },
    )
}

fn cmd_query(a: &Args, out: &mut dyn Write, reverse: bool) -> Result<i32, String> {
    let Some(id) = a.positional.first() else {
        return Err("query/impact needs a vertex ID".to_string());
    };
    let depth = cli_args::usize_arg(a, "depth", 1)?;
    let max = cli_args::usize_arg(a, "max", 200)?;
    let doc = cli_args::document_for(a, true)?;
    let id = require_id(&doc, id)?;
    // 0은 깊이 제한 없음이다(MCP와 같은 해석 함수) — 보고에는 요청한 깊이를 싣는다.
    let walk = cli_args::walk_depth(depth);
    if reverse {
        let mut r = analysis::impact(&doc, &id, walk, max);
        r.depth = depth;
        write!(out, "{}", export::to_json(&r)).ok();
    } else {
        let mut r = analysis::query(&doc, &id, walk, max);
        r.depth = depth;
        write!(out, "{}", export::to_json(&r)).ok();
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_cli(args: &[&str]) -> (i32, String, String) {
        let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let mut out = Vec::new();
        let mut err = Vec::new();
        let code = run(&argv, &mut out, &mut err);
        (
            code,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[test]
    fn version_prints() {
        let (code, out, _) = run_cli(&["version"]);
        assert_eq!(code, 0);
        assert!(out.contains("rustograph"));
    }

    #[test]
    fn version_flag_matches_version_command() {
        // 별칭이 따로 놀면 설치 버전 대조가 하위 명령과 다른 값을 읽게 된다.
        let (code, flag_out, _) = run_cli(&["--version"]);
        let (_, command_out, _) = run_cli(&["version"]);
        assert_eq!(code, 0);
        assert_eq!(flag_out, command_out);
        assert_eq!(flag_out, format!("rustograph {VERSION}\n"));
    }

    #[test]
    fn no_command_is_usage_error() {
        let (code, _, err) = run_cli(&[]);
        assert_eq!(code, 2);
        assert!(err.contains("usage"));
    }

    #[test]
    fn unknown_flag_is_usage_error() {
        let (code, _, err) = run_cli(&["graph", "--bogus"]);
        assert_eq!(code, 2);
        assert!(err.contains("unknown flag"));
    }

    #[test]
    fn unknown_level_is_usage_error() {
        let (code, _, err) = run_cli(&["graph", "--level", "bogus"]);
        assert_eq!(code, 2);
        assert!(err.contains("unknown level"));
    }

    #[test]
    fn unknown_format_rejected() {
        // graph --format bogus는 값 검증 에러(2).
        let (code, _, _) = run_cli(&["graph", "--format", "bogus", "--dir", "nonexistent-dir"]);
        assert_eq!(code, 2);
    }

    #[test]
    fn missing_dir_is_error() {
        let (code, _, err) = run_cli(&["graph", "--dir", "no-such-dir-xyz"]);
        assert_eq!(code, 2);
        assert!(err.contains("error"));
    }

    #[test]
    fn query_needs_id() {
        let (code, _, err) = run_cli(&["query", "--dir", "no-such-dir-xyz"]);
        assert_eq!(code, 2);
        assert!(err.contains("vertex ID"));
    }

    /// `graph --out`·`rules --write-baseline`은 문서를 만든 수확이 읽은 입력으로 보호 목록을
    /// 만든다 — 보호 목록 때문에 워크스페이스를 다시 수확하면 대형 저장소에서 시간과 누수 AST
    /// 메모리가 두 배였다. 표준 출력 실행과 같은 수의 파일만 파싱해야 한다.
    #[test]
    fn file_outputs_reuse_the_documents_harvest() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture");
        let dir = fixture.to_str().unwrap();
        let tmp = std::env::temp_dir().join(format!("rustograph-cli-out-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        std::fs::create_dir_all(&tmp).unwrap();
        let rules_config = tmp.join("rules.yml");
        std::fs::write(&rules_config, "components: {}\ndeps: {}\n").unwrap();
        let parsed_by = |args: &[&str]| {
            let before = source::parsed_files();
            let (code, _, err) = run_cli(args);
            assert_eq!(code, 0, "{args:?}: {err}");
            source::parsed_files() - before
        };
        let stdout = parsed_by(&["graph", "--level", "symbol", "--dir", dir]);
        assert!(stdout > 0, "the harvest must parse the fixture");
        // fixture에는 풀 수 없는 include!가 있어 기존 출력 교체가 거부된다 — 새 경로에 쓴다.
        let (graph_out, baseline) = (tmp.join("graph.json"), tmp.join("baseline.txt"));
        let (graph_out, baseline) = (graph_out.to_str().unwrap(), baseline.to_str().unwrap());
        let rules_config = rules_config.to_str().unwrap();
        let to_file = parsed_by(&[
            "graph", "--level", "symbol", "--dir", dir, "--out", graph_out,
        ]);
        let to_baseline = parsed_by(&[
            "rules",
            "--write-baseline",
            "--dir",
            dir,
            "--config",
            rules_config,
            "--baseline",
            baseline,
        ]);
        assert_eq!((to_file, to_baseline), (stdout, stdout));
        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
