//! 라우팅 오라클 공용 부분 — 사실 읽기, 요청 계획, 결과 판정, 기록.
//!
//! 판정 기준: "rustograph 사실만으로 isthmus 방식(세그먼트 매칭·method·끝 슬래시·
//! 구체성 또는 등록 순서)으로 고른 핸들러"가 "프레임워크가 실제로 디스패치한
//! 핸들러"와 같아야 한다. 요청은 프로세스 안에서만 보낸다(네트워크 없음).

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

/// routes 문서의 사실 하나(오라클이 쓰는 필드만).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Fact {
    pub method: String,
    pub channel: String,
    #[serde(rename = "pathAnchor")]
    pub path_anchor: String,
    #[serde(rename = "trailingSlash", skip_serializing_if = "Option::is_none")]
    pub trailing_slash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order: Option<(String, u64)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usr: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not", default)]
    pub narrowed: bool,
    #[serde(rename = "paramConstraints", skip_serializing_if = "Vec::is_empty", default)]
    pub constraints: Vec<(usize, String, Option<String>)>,
}

/// routes 문서에서 정적 사실을 읽는다(dynamic 제외).
pub fn load_facts(doc: &Value) -> (String, Vec<Fact>) {
    let dispatch = doc["dispatch"].as_str().unwrap_or("specificity").to_string();
    let mut out = Vec::new();
    for f in doc["facts"].as_array().into_iter().flatten() {
        if f["dynamic"].as_bool() == Some(true) {
            continue;
        }
        out.push(Fact {
            method: f["method"].as_str().unwrap_or_default().to_string(),
            channel: f["channel"].as_str().unwrap_or_default().to_string(),
            path_anchor: f["pathAnchor"].as_str().unwrap_or_default().to_string(),
            trailing_slash: f["trailingSlash"].as_str().map(str::to_string),
            order: f.get("order").and_then(|o| {
                Some((o["group"].as_str()?.to_string(), o["index"].as_u64()?))
            }),
            usr: f.pointer("/symbol/usr").and_then(Value::as_str).map(str::to_string),
            narrowed: f["narrowed"].as_bool().unwrap_or(false),
            constraints: f["paramConstraints"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|c| {
                    (
                        c["segment"].as_u64().unwrap_or(0) as usize,
                        c["kind"].as_str().unwrap_or_default().to_string(),
                        c["pattern"].as_str().map(str::to_string),
                    )
                })
                .collect(),
        });
    }
    (dispatch, out)
}

/// 한 요청.
#[derive(Clone, Debug, Serialize)]
pub struct Probe {
    /// fact·trailing·expected·negative.
    pub kind: &'static str,
    pub method: String,
    pub path: String,
    /// 기대(ground truth) 핸들러 — expected·negative 요청만.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truth: Option<Option<String>>,
    /// 이 요청을 만든 사실(fact·trailing).
    #[serde(skip)]
    pub fact: Option<usize>,
}

/// 사실만으로 고른 결과.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "usr")]
pub enum Predicted {
    /// 어느 사실도 맞지 않음.
    None,
    /// usr 없는 사실(서비스 등)이 받음.
    Unnamed,
    /// 이 usr의 사실이 받음.
    Handler(String),
    /// 구체성 동률 — 모호.
    Ambiguous,
}

/// 실제 응답의 해석.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "body")]
pub enum Actual {
    /// 404·405 — 핸들러에 닿지 않음.
    NoMatch,
    /// 알려진 핸들러 ID를 본문으로 돌려줌.
    Handler(String),
    /// 그 밖의 성공 응답(서비스 등).
    Other(String),
}

/// 요청 계획 — 사실마다 표본 경로, 끝 슬래시 뒤집기, 기대·음성 요청.
pub fn plan(facts: &[Fact], expected: &[(&str, &str, &str)], negatives: &[(&str, &str)]) -> Vec<Probe> {
    let mut out = Vec::new();
    for (i, f) in facts.iter().enumerate() {
        if f.path_anchor != "root" {
            continue;
        }
        let path = sample(f);
        let methods: Vec<String> = if f.method == "ANY" {
            vec!["GET".into(), "POST".into(), "DELETE".into()]
        } else {
            vec![f.method.clone()]
        };
        for m in &methods {
            out.push(Probe {
                kind: "fact",
                method: m.clone(),
                path: path.clone(),
                truth: None,
                fact: Some(i),
            });
            if f.trailing_slash.is_some() && path != "/" {
                let toggled = match path.strip_suffix('/') {
                    Some(p) => p.to_string(),
                    None => format!("{path}/"),
                };
                out.push(Probe {
                    kind: "trailing",
                    method: m.clone(),
                    path: toggled,
                    truth: None,
                    fact: Some(i),
                });
            }
        }
    }
    for (m, p, h) in expected {
        out.push(Probe {
            kind: "expected",
            method: m.to_string(),
            path: p.to_string(),
            truth: Some(Some(h.to_string())),
            fact: None,
        });
    }
    for (m, p) in negatives {
        out.push(Probe {
            kind: "negative",
            method: m.to_string(),
            path: p.to_string(),
            truth: Some(None),
            fact: None,
        });
    }
    out
}

/// 사실의 표본 경로 — 파라미터를 제약에 맞는 값으로 채운다.
pub fn sample(f: &Fact) -> String {
    let segs: Vec<&str> = f.channel[1..].split('/').collect();
    let mut out = String::new();
    for (i, seg) in segs.iter().enumerate() {
        out.push('/');
        if *seg == "{**}" {
            out.push_str("a/b");
            continue;
        }
        match seg.find("{}") {
            None => out.push_str(seg),
            Some(at) => {
                let value = match f.constraints.iter().find(|c| c.0 == i) {
                    Some((_, k, _)) if k == "int" => "7".to_string(),
                    Some((_, k, _)) if k == "slug" => "ab-c".to_string(),
                    Some((_, k, _)) if k == "uuid" => "123e4567-e89b-12d3-a456-426614174000".to_string(),
                    Some((_, _, Some(p))) => regex_sample(p),
                    _ => "x1".to_string(),
                };
                out.push_str(&seg[..at]);
                out.push_str(&value);
                out.push_str(&seg[at + 2..]);
            }
        }
    }
    out
}

/// 정규식 제약을 만족하는 후보 값.
fn regex_sample(p: &str) -> String {
    let re = Regex::new(&format!("^(?:{p})$")).expect("constraint regex compiles");
    ["7", "abc", "ab-c", "a.b", "x1", "A1", "123e4567-e89b-12d3-a456-426614174000"]
        .iter()
        .find(|c| re.is_match(c))
        .map(|c| c.to_string())
        .unwrap_or_else(|| "x1".to_string())
}

/// 사실 템플릿이 요청 경로와 맞는가(끝 슬래시 optional이면 양쪽 형태).
fn matches(f: &Fact, path: &str) -> bool {
    if matches_exact(f, path) {
        return true;
    }
    if f.trailing_slash.as_deref() == Some("optional") && path != "/" {
        let alt = match path.strip_suffix('/') {
            Some(p) => p.to_string(),
            None => format!("{path}/"),
        };
        return matches_exact(f, &alt);
    }
    false
}

fn matches_exact(f: &Fact, path: &str) -> bool {
    let t: Vec<&str> = f.channel[1..].split('/').collect();
    let p: Vec<&str> = path[1..].split('/').collect();
    for (i, seg) in t.iter().enumerate() {
        if *seg == "{**}" {
            let rest = &p[i.min(p.len())..];
            return !rest.is_empty() && rest.iter().any(|s| !s.is_empty());
        }
        let Some(value) = p.get(i) else { return false };
        match seg.find("{}") {
            None => {
                if seg != value {
                    return false;
                }
            }
            Some(at) => {
                let (pre, suf) = (&seg[..at], &seg[at + 2..]);
                if value.len() <= pre.len() + suf.len()
                    || !value.starts_with(pre)
                    || !value.ends_with(suf)
                {
                    return false;
                }
                let mid = &value[pre.len()..value.len() - suf.len()];
                if !constraint_ok(f, i, mid) {
                    return false;
                }
            }
        }
    }
    t.len() == p.len()
}

fn constraint_ok(f: &Fact, seg: usize, v: &str) -> bool {
    match f.constraints.iter().find(|c| c.0 == seg) {
        Some((_, k, _)) if k == "int" => Regex::new(r"^[+-]?[0-9]+$").unwrap().is_match(v),
        Some((_, k, _)) if k == "slug" => Regex::new(r"^[-A-Za-z0-9_]+$").unwrap().is_match(v),
        Some((_, k, _)) if k == "uuid" => Regex::new(
            r"^(?i:[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|[0-9a-f]{32})$",
        )
        .unwrap()
        .is_match(v),
        Some((_, _, Some(p))) => Regex::new(&format!("^(?:{p})$")).unwrap().is_match(v),
        _ => true,
    }
}

/// 세그먼트 순위 — 리터럴 > 부분 > 제약 있는 `{}` > `{}` > `{**}`(isthmus 구체성).
fn ranks(f: &Fact) -> Vec<u8> {
    f.channel[1..]
        .split('/')
        .enumerate()
        .map(|(i, s)| {
            if s == "{**}" {
                0
            } else if s == "{}" {
                if f.constraints.iter().any(|c| c.0 == i) {
                    2
                } else {
                    1
                }
            } else if s.contains("{}") {
                3
            } else {
                4
            }
        })
        .collect()
}

/// 사실만으로 요청을 받을 핸들러를 고른다.
pub fn predict(facts: &[Fact], dispatch: &str, method: &str, path: &str) -> Predicted {
    let method_ok = |f: &Fact| f.method == method || f.method == "ANY" || (method == "HEAD" && f.method == "GET");
    let mut cands: Vec<&Fact> = facts
        .iter()
        .filter(|f| f.path_anchor == "root" && method_ok(f) && matches(f, path))
        .collect();
    if cands.is_empty() {
        return Predicted::None;
    }
    // 명시 동사가 ANY·head-as-get보다 먼저다(같은 등록·같은 템플릿 안).
    let exact = |f: &&Fact| f.method == method;
    let pick = |f: &Fact| match &f.usr {
        Some(u) => Predicted::Handler(u.clone()),
        None => Predicted::Unnamed,
    };
    if dispatch == "registration-order" {
        cands.sort_by_key(|f| (f.order.clone().map(|o| o.1).unwrap_or(u64::MAX), !exact(f)));
        return pick(cands[0]);
    }
    let best = cands.iter().map(|f| ranks(f)).max().expect("non-empty");
    let top: Vec<&&Fact> = cands.iter().filter(|f| ranks(f) == best).collect();
    let templates: BTreeSet<&str> = top.iter().map(|f| f.channel.as_str()).collect();
    if templates.len() > 1 {
        return Predicted::Ambiguous;
    }
    let chosen = top.iter().find(|f| exact(f)).copied().unwrap_or(top[0]);
    pick(chosen)
}

/// 응답 해석 — 404·405는 미도달, 본문이 알려진 ID면 그 핸들러.
pub fn actual(status: u16, body: &str, known: &BTreeSet<String>) -> Actual {
    if status == 404 || status == 405 {
        Actual::NoMatch
    } else if known.contains(body) {
        Actual::Handler(body.to_string())
    } else {
        Actual::Other(body.to_string())
    }
}

/// 예측과 실제가 같은가.
fn agrees(p: &Predicted, a: &Actual) -> bool {
    match (p, a) {
        (Predicted::None, Actual::NoMatch) => true,
        (Predicted::Unnamed, Actual::Other(_)) => true,
        (Predicted::Handler(u), Actual::Handler(b)) => u == b,
        _ => false,
    }
}

/// 요청 하나의 기록.
#[derive(Serialize)]
pub struct ProbeResult {
    #[serde(flatten)]
    pub probe: Probe,
    pub predicted: Predicted,
    pub status: u16,
    pub actual: Actual,
    pub pass: bool,
}

/// 합계.
#[derive(Serialize, Default)]
pub struct Tally {
    pub passed: usize,
    pub total: usize,
}

/// 오라클 기록 — 커밋해 오프라인 테스트가 읽는다.
#[derive(Serialize)]
pub struct Recording {
    pub fixture: String,
    pub framework: String,
    pub dispatch: String,
    /// 모든 요청이 통과한 사실(정밀도의 분자).
    #[serde(rename = "verifiedFacts")]
    pub verified_facts: Vec<Fact>,
    /// 요청으로 확인할 수 없는 사실(base 앵커 — 붙는 곳이 없다)과 이유.
    pub unprobed: Vec<(Fact, String)>,
    /// 요청이 하나라도 실패한 사실.
    #[serde(rename = "failedFacts")]
    pub failed_facts: Vec<Fact>,
    pub precision: Tally,
    pub recall: Tally,
    pub negatives: Tally,
    #[serde(rename = "trailingSlash")]
    pub trailing: Tally,
    pub probes: Vec<ProbeResult>,
}

/// 실행 결과를 판정해 기록을 만든다.
pub fn evaluate(
    fixture: &str,
    framework: &str,
    dispatch: &str,
    facts: &[Fact],
    results: Vec<(Probe, u16, String)>,
) -> Recording {
    let mut known: BTreeSet<String> = facts.iter().filter_map(|f| f.usr.clone()).collect();
    for (p, _, _) in &results {
        if let Some(Some(h)) = &p.truth {
            known.insert(h.clone());
        }
    }
    let mut failed: BTreeSet<usize> = BTreeSet::new();
    let (mut precision, mut recall, mut negatives, mut trailing) =
        (Tally::default(), Tally::default(), Tally::default(), Tally::default());
    let mut probes = Vec::new();
    for (probe, status, body) in results {
        let predicted = predict(facts, dispatch, &probe.method, &probe.path);
        let act = actual(status, &body, &known);
        let pass = match &probe.truth {
            // 기대 요청: 실제가 기대 핸들러이고 사실 예측도 같아야 한다(재현율).
            Some(Some(h)) => act == Actual::Handler(h.clone()) && predicted == Predicted::Handler(h.clone()),
            // 음성 요청: 실제로 닿지 않고 사실도 아무것도 주장하지 않는다.
            Some(None) => act == Actual::NoMatch && predicted == Predicted::None,
            None => agrees(&predicted, &act),
        };
        let tally = match probe.kind {
            "fact" => &mut precision,
            "trailing" => &mut trailing,
            "expected" => &mut recall,
            _ => &mut negatives,
        };
        tally.total += 1;
        if pass {
            tally.passed += 1;
        } else if let Some(i) = probe.fact {
            failed.insert(i);
        }
        probes.push(ProbeResult {
            probe,
            predicted,
            status,
            actual: act,
            pass,
        });
    }
    let mut verified = Vec::new();
    let mut unprobed = Vec::new();
    let mut failed_facts = Vec::new();
    for (i, f) in facts.iter().enumerate() {
        if f.path_anchor != "root" {
            unprobed.push((f.clone(), "pathAnchor base: the router is not mounted on the served app".to_string()));
        } else if failed.contains(&i) {
            failed_facts.push(f.clone());
        } else {
            verified.push(f.clone());
        }
    }
    Recording {
        fixture: fixture.to_string(),
        framework: framework.to_string(),
        dispatch: dispatch.to_string(),
        verified_facts: verified,
        unprobed,
        failed_facts,
        precision,
        recall,
        negatives,
        trailing,
        probes,
    }
}

/// 명령행: `<routes 문서 경로> <기록 출력 경로>`.
pub fn io_paths() -> (String, String) {
    let args: Vec<String> = std::env::args().collect();
    assert!(args.len() == 3, "usage: oracle <routes.json> <recording.json>");
    (args[1].clone(), args[2].clone())
}

/// 기록을 쓰고 요약을 출력한다. 정밀도·재현율이 100%가 아니면 실패 코드.
pub fn write(rec: &Recording, out: &str) -> std::process::ExitCode {
    let text = serde_json::to_string_pretty(rec).expect("recording serializes") + "\n";
    std::fs::write(out, text).expect("recording written");
    println!(
        "{}: precision {}/{} recall {}/{} negatives {}/{} trailing {}/{} unprobed {}",
        rec.fixture,
        rec.precision.passed,
        rec.precision.total,
        rec.recall.passed,
        rec.recall.total,
        rec.negatives.passed,
        rec.negatives.total,
        rec.trailing.passed,
        rec.trailing.total,
        rec.unprobed.len()
    );
    for p in rec.probes.iter().filter(|p| !p.pass) {
        println!(
            "  FAIL {} {} {} predicted={:?} actual={:?} status={}",
            p.probe.kind, p.probe.method, p.probe.path, p.predicted, p.actual, p.status
        );
    }
    let ok = [&rec.precision, &rec.recall, &rec.negatives, &rec.trailing]
        .iter()
        .all(|t| t.passed == t.total);
    if ok {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}
