//! 명령행 인터페이스 — 인자 해석과 종료 코드 계약.
//!
//! 종료 코드: 0 정상, 1 strict 위반 발견, 2 사용법/분석 오류.
//! 플래그 파서는 외부 크레이트 없이 직접 만든다 — 명령이 적고 계약이 단순해서다.

use crate::cli_args::{self, Args};
use crate::{analysis, config, export, mcp, rules, sarif};
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
  rustograph query ID [--depth N] [--max N]
  rustograph impact ID [--depth N] [--max N]
  rustograph mcp [--dir DIR] [--graph FILE] [--config FILE] [--deps] [--tests]
  rustograph version

exit codes: 0 ok · 1 strict violation found · 2 usage/analysis error";

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
    let a = cli_args::parse(args).map_err(|e| format!("{e}\n{USAGE}"))?;
    match a.cmd.as_str() {
        "version" => {
            writeln!(out, "rustograph {VERSION}").ok();
            Ok(0)
        }
        "graph" => cmd_graph(&a, out),
        "cycles" => cmd_cycles(&a, out),
        "dead" => cmd_dead(&a, out),
        "rules" => cmd_rules(&a, out),
        "query" => cmd_query(&a, out, false),
        "impact" => cmd_query(&a, out, true),
        "mcp" => mcp::cmd(&a, &mut std::io::stdin().lock(), out, err),
        "-h" | "--help" | "help" => {
            writeln!(out, "{USAGE}").ok();
            Ok(0)
        }
        other => Err(format!("unknown command {other}\n{USAGE}")),
    }
}

fn cmd_graph(a: &Args, out: &mut dyn Write) -> Result<i32, String> {
    let level = cli_args::level_of(a)?;
    // 수확은 항상 최대 깊이로 — 얕은 레벨은 투영이다.
    let doc = cli_args::document_for(a, true)?.view(level);
    let text = match a.get("format").unwrap_or("json") {
        "json" => export::to_json(&doc),
        "mermaid" => export::to_mermaid(&doc),
        f => return Err(format!("unknown format {f} (json|mermaid)")),
    };
    match a.get("out") {
        Some(p) => {
            let path = Path::new(p);
            if a.get("format").unwrap_or("json") == "json" {
                export::save_file(&doc, path)?;
            } else {
                std::fs::write(path, &text).map_err(|e| format!("cannot write {p}: {e}"))?;
            }
            writeln!(out, "wrote {p}").ok();
        }
        None => {
            write!(out, "{text}").ok();
        }
    }
    Ok(0)
}

fn cmd_cycles(a: &Args, out: &mut dyn Write) -> Result<i32, String> {
    let level = cli_args::level_of(a)?;
    let doc = cli_args::document_for(a, true)?.view(level);
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
        match analysis::explain_path(&doc, target) {
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
    let report = analysis::dead(&doc);
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
    let cfg = match a.get("config") {
        Some(f) => {
            let src = std::fs::read_to_string(f).map_err(|e| format!("cannot read {f}: {e}"))?;
            serde_yml::from_str(&src).map_err(|e| format!("bad {f}: {e}"))?
        }
        None => match config::load(&dir)? {
            Some(c) => c,
            None => return Err(format!("no .rustograph.yml in {}", dir.display())),
        },
    };
    let doc = cli_args::document_for(a, true)?;
    let report = rules::check(&doc, &cfg);
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

fn cmd_query(a: &Args, out: &mut dyn Write, reverse: bool) -> Result<i32, String> {
    let Some(id) = a.positional.first() else {
        return Err("query/impact needs a vertex ID".to_string());
    };
    let depth: usize = a.get("depth").and_then(|s| s.parse().ok()).unwrap_or(1);
    let max: usize = a.get("max").and_then(|s| s.parse().ok()).unwrap_or(200);
    let doc = cli_args::document_for(a, true)?;
    let exists = doc.vertex_ids().contains(id.as_str());
    if !exists {
        return Err(format!("vertex {id} not found"));
    }
    if reverse {
        let r = analysis::impact(&doc, id, depth, max);
        write!(out, "{}", export::to_json(&r)).ok();
    } else {
        let r = analysis::query(&doc, id, depth, max);
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
}
