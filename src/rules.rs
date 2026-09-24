//! 아키텍처 규칙 검사 — deps 허용 목록 + deny 금지 + signature 누출.
//!
//! 규칙 위반은 그래프 사실에 대한 규칙 불일치다. 매핑되지 않은 정점은
//! "규칙이 모르는 영역"으로 따로 보고한다 — 규칙 무관이 아니다.

use crate::config::{self, Config};
use crate::graph::{Document, EdgeKind, Kind};
use serde::Serialize;
use std::collections::BTreeMap;

/// 위반 하나 — 어떤 규칙이 어떤 간선을 깼는가.
#[derive(Debug, Serialize)]
pub struct Violation {
    /// "allow" | "deny" | "signature".
    pub rule: String,
    pub from: String,
    pub to: String,
    pub component: String,
    pub forbidden: String,
    pub kind: EdgeKind,
}

impl Violation {
    /// 기준선 키 — `rule|from|to|kind`. 위반의 정체성은 사람 문구가 아니라
    /// 이 넷이다 — 문구가 바뀌어도 같은 위반은 같은 키다.
    pub fn key(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            self.rule,
            self.from,
            self.to,
            format!("{:?}", self.kind).to_lowercase()
        )
    }
}

/// 기준선 — 기존 위반을 "알고 있는" 키 목록. 레거시 코드베이스가 규칙을
/// 도입할 때 기존 위반을 얼려서 새 위반만 실패하게 만드는 장치다.
/// 파일 형식은 한 줄에 키 하나, `#` 주석과 빈 줄 허용.
#[derive(Debug, Default)]
pub struct Baseline {
    keys: std::collections::BTreeSet<String>,
}

impl Baseline {
    /// 기준선 파일을 파싱한다. 모르는 줄도 키로 받는다 — 위반과 안 맞으면
    /// stale로 계수되니 형식 오류가 조용히 실패를 숨기지는 않는다.
    pub fn parse(src: &str) -> Baseline {
        Baseline {
            keys: src
                .lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty() && !l.starts_with('#'))
                .map(|l| l.to_string())
                .collect(),
        }
    }

    /// 위반 목록을 기준선 파일로 렌더링한다 — 결정적 정렬이라 diff가 읽힌다.
    pub fn render(violations: &[Violation]) -> String {
        let mut keys: Vec<String> = violations.iter().map(|v| v.key()).collect();
        keys.sort();
        keys.dedup();
        let mut out =
            String::from("# rustograph rules baseline — existing violations frozen at adoption.\n");
        for k in keys {
            out.push_str(&k);
            out.push('\n');
        }
        out
    }

    /// 이 위반이 기준선에 있는가.
    pub fn contains(&self, v: &Violation) -> bool {
        self.keys.contains(&v.key())
    }

    /// 기준선 항목 수 — stale 계수용.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// 기준선이 비었는지 — len의 짝.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

/// 보고서에 기준선을 적용한다 — 기준선 안 위반을 violations에서 빼고
/// 수를 baselined에, 더는 위반이 아닌 기준선 항목 수를 stale_baseline에
/// 센다. stale은 "개선됐으니 기준선을 갱신해도 된다"는 신호다.
pub fn apply_baseline(rep: &mut RulesReport, base: &Baseline) {
    let before = rep.violations.len();
    rep.violations.retain(|v| !base.contains(v));
    rep.baselined = before - rep.violations.len();
    // 기준선 키 하나는 위반 최대 하나에만 대응한다(위반은 키로 중복 제거됨) —
    // 안 쓰인 기준선 항목은 더는 위반이 아니라는 뜻이다.
    rep.stale_baseline = base.len().saturating_sub(rep.baselined);
}

/// 규칙 보고서.
#[derive(Debug, Serialize)]
pub struct RulesReport {
    pub violations: Vec<Violation>,
    /// 어느 컴포넌트에도 속하지 않은 모듈/크레이트.
    pub unmapped: Vec<String>,
    /// 추정(팬아웃) 간선은 "가능한" 의존이라 위반 증거가 못 된다 — 건너뛴 수를 남긴다.
    #[serde(skip_serializing_if = "crate::graph::is_zero")]
    pub skipped_tentative: usize,
    /// 기준선으로 억눌린 기존 위반 수 — 새 위반만 violations에 남는다.
    #[serde(skip_serializing_if = "crate::graph::is_zero")]
    pub baselined: usize,
    /// 기준선에 있지만 더는 위반이 아닌 항목 수 — 개선 신호.
    #[serde(skip_serializing_if = "crate::graph::is_zero")]
    pub stale_baseline: usize,
}

/// 정점의 컴포넌트 — module 경로(또는 crate)를 패턴에 맞춘다. 긴 패턴 우선.
fn component_of(cfg: &Config, path: &str) -> Option<String> {
    let mut best: Option<(&String, &String)> = None;
    for (component, patterns) in &cfg.components {
        for p in patterns {
            if config::matches(p, path) && best.is_none_or(|(_, bp)| p.len() > bp.len()) {
                best = Some((component, p));
            }
        }
    }
    best.map(|(c, _)| c.clone())
}

/// 규칙 대상 경로 — 정점의 소속 단위(모듈 경로 또는 크레이트 이름).
fn scope_of(doc: &Document, vertex_id: &str) -> String {
    // 정점의 module 필드가 규칙 매칭 단위다.
    doc.vertices
        .iter()
        .find(|v| v.id == vertex_id)
        .map(|v| v.module.clone())
        .unwrap_or_else(|| vertex_id.to_string())
}

/// 규칙 검사 — 의존 간선을 컴포넌트 규칙에 대조한다.
pub fn check(doc: &Document, cfg: &Config) -> RulesReport {
    let mut violations = Vec::new();
    let mut unmapped: Vec<String> = Vec::new();
    let mut mapped_cache: BTreeMap<String, Option<String>> = BTreeMap::new();
    let mut comp = |path: &str| -> Option<String> {
        mapped_cache
            .entry(path.to_string())
            .or_insert_with(|| component_of(cfg, path))
            .clone()
    };

    // 매핑되지 않은 모듈·크레이트 정점을 보고한다 — 규칙의 사각지대.
    for v in &doc.vertices {
        if matches!(v.kind, Kind::Module | Kind::Crate)
            && comp(&v.id).is_none()
            && comp(&v.module).is_none()
        {
            unmapped.push(v.id.clone());
        }
    }
    unmapped.sort();
    unmapped.dedup();

    let mut skipped_tentative = 0usize;
    for e in &doc.edges {
        if !e.kind.is_dependency() {
            continue;
        }
        if e.tentative {
            skipped_tentative += 1;
            continue;
        }
        let (fs, ts) = (scope_of(doc, &e.from), scope_of(doc, &e.to));
        if fs == ts {
            continue; // 같은 컴포넌트 내부 의존은 규칙 대상이 아니다.
        }
        let (Some(fc), Some(tc)) = (comp(&fs), comp(&ts)) else {
            continue; // 매핑 밖은 unmapped로 이미 보고된다.
        };
        if fc == tc {
            continue;
        }
        // deny가 deps를 이긴다.
        if cfg
            .deny
            .get(&fc)
            .is_some_and(|list| list.iter().any(|p| p == &tc || config::matches(p, &ts)))
        {
            violations.push(Violation {
                rule: "deny".to_string(),
                from: e.from.clone(),
                to: e.to.clone(),
                component: fc.clone(),
                forbidden: tc.clone(),
                kind: e.kind,
            });
            continue;
        }
        // signature 규칙: exported 심볼의 signature 간선만 대상. 비exported
        // 심볼이나 signature 규칙이 없는 컴포넌트의 간선은 아래 deps 검사로 간다.
        if e.kind == EdgeKind::Signature {
            let from_exported = doc
                .vertices
                .iter()
                .find(|v| v.id == e.from)
                .is_some_and(|v| v.exported);
            if from_exported {
                if let Some(allowed) = cfg.signature.get(&fc) {
                    let ok = allowed.iter().any(|p| p == &tc || config::matches(p, &ts));
                    if !ok {
                        violations.push(Violation {
                            rule: "signature".to_string(),
                            from: e.from.clone(),
                            to: e.to.clone(),
                            component: fc.clone(),
                            forbidden: tc.clone(),
                            kind: e.kind,
                        });
                    }
                    continue; // signature 규칙이 잡은 간선은 deps 검사를 건너뛴다.
                }
            }
        }
        // allowlist: deps 항목이 없는 컴포넌트는 자기 외 의존 불가.
        let allowed = cfg.deps.get(&fc).cloned().unwrap_or_default();
        let ok = allowed.iter().any(|p| p == &tc || config::matches(p, &ts));
        if !ok {
            violations.push(Violation {
                rule: "allow".to_string(),
                from: e.from.clone(),
                to: e.to.clone(),
                component: fc.clone(),
                forbidden: tc.clone(),
                kind: e.kind,
            });
        }
    }
    violations.sort_by(|a, b| (&a.rule, &a.from, &a.to).cmp(&(&b.rule, &b.from, &b.to)));
    violations
        .dedup_by(|a, b| a.rule == b.rule && a.from == b.from && a.to == b.to && a.kind == b.kind);
    RulesReport {
        violations,
        unmapped,
        skipped_tentative,
        baselined: 0,
        stale_baseline: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::graph::{document, Edge, Level, Vertex};
    use std::collections::BTreeMap;

    fn v(id: &str, kind: Kind, exported: bool) -> Vertex {
        Vertex {
            id: id.to_string(),
            kind,
            krate: "c".to_string(),
            module: id
                .rsplit_once("::")
                .map(|(m, _)| m.to_string())
                .unwrap_or_else(|| id.to_string()),
            position: None,
            exported,
            generated: false,
            cfg: None,
            unsafe_: false,
        }
    }

    fn cfg() -> Config {
        Config {
            components: BTreeMap::from([
                (
                    "core".to_string(),
                    vec!["c::core".to_string(), "c::core::*".to_string()],
                ),
                (
                    "ui".to_string(),
                    vec!["c::ui".to_string(), "c::ui::*".to_string()],
                ),
                (
                    "other".to_string(),
                    vec!["c::other".to_string(), "c::other::*".to_string()],
                ),
            ]),
            deps: BTreeMap::from([
                ("ui".to_string(), vec!["core".to_string()]),
                ("core".to_string(), vec![]),
            ]),
            deny: BTreeMap::from([("core".to_string(), vec!["ui".to_string()])]),
            signature: BTreeMap::from([(
                "ui".to_string(),
                vec!["ui".to_string(), "core".to_string()],
            )]),
            baseline: None,
        }
    }

    fn doc(edges: Vec<Edge>) -> Document {
        document(
            Level::Symbol,
            ".".into(),
            None,
            vec![],
            vec![
                v("c::core::a", Kind::Fn, true),
                v("c::ui::b", Kind::Fn, true),
                v("c::ui::c", Kind::Fn, false),
                v("c::other::x", Kind::Module, false),
                v("z::nowhere::m", Kind::Module, false),
            ],
            edges,
            vec![],
        )
    }

    #[test]
    fn deny_beats_allow_and_allowlist_catches_rest() {
        // core->ui: deny에 걸린다. ui->core: 허용. ui->(다른 ui): 같은 컴포넌트라 면제.
        let d = doc(vec![
            Edge::new("c::core::a".into(), "c::ui::b".into(), EdgeKind::Call),
            Edge::new("c::ui::b".into(), "c::core::a".into(), EdgeKind::Call),
            Edge::new("c::ui::b".into(), "c::ui::c".into(), EdgeKind::Call),
        ]);
        let rep = check(&d, &cfg());
        assert_eq!(rep.violations.len(), 1);
        assert_eq!(rep.violations[0].rule, "deny");
        assert_eq!(rep.violations[0].component, "core");
    }

    #[test]
    fn signature_rule_only_checks_exported() {
        // 비exported 심볼의 signature 간선은 signature 규칙 대상이 아니다
        // (deps 검사는 받는다 — ui::c→core::a는 허용 목록에 있어 위반 아님).
        let d = doc(vec![
            Edge::new("c::ui::c".into(), "c::core::a".into(), EdgeKind::Signature),
            Edge::new("c::ui::b".into(), "c::other::x".into(), EdgeKind::Signature),
        ]);
        let rep = check(&d, &cfg());
        assert_eq!(rep.violations.len(), 1);
        assert_eq!(rep.violations[0].rule, "signature");
        assert_eq!(rep.violations[0].from, "c::ui::b");
    }

    #[test]
    fn tentative_edges_are_skipped_and_counted() {
        let d = doc(vec![Edge::maybe(
            "c::core::a".into(),
            "c::ui::b".into(),
            EdgeKind::Call,
        )]);
        let rep = check(&d, &cfg());
        assert!(rep.violations.is_empty());
        assert_eq!(rep.skipped_tentative, 1);
    }

    #[test]
    fn unmapped_modules_reported() {
        let d = doc(vec![]);
        let rep = check(&d, &cfg());
        assert!(rep.unmapped.contains(&"z::nowhere::m".to_string()));
        assert!(!rep.unmapped.contains(&"c::other::x".to_string()));
    }

    /// 위반 두 건을 내는 문서 — core→ui는 deny, other→ui는 allow 위반.
    fn violating_doc() -> Document {
        doc(vec![
            Edge::new("c::core::a".into(), "c::ui::b".into(), EdgeKind::Call),
            Edge::new("c::other::x".into(), "c::ui::b".into(), EdgeKind::Uses),
        ])
    }

    #[test]
    fn baseline_suppresses_known_and_marks_stale() {
        let rep = check(&violating_doc(), &cfg());
        assert_eq!(rep.violations.len(), 2);
        // 얼리기: render → parse → apply — 같은 위반은 사라지고 센다.
        let frozen = Baseline::parse(&Baseline::render(&rep.violations));
        let mut rep2 = check(&violating_doc(), &cfg());
        apply_baseline(&mut rep2, &frozen);
        assert!(rep2.violations.is_empty());
        assert_eq!(rep2.baselined, 2);
        assert_eq!(rep2.stale_baseline, 0);
        // 사라진 위반의 기준선 항목은 stale — 갱신 신호다.
        let mut stale = Baseline::render(&rep.violations);
        stale.push_str("deny|gone::a|gone::b|call\n# comment\n\n");
        let mut rep3 = check(&violating_doc(), &cfg());
        apply_baseline(&mut rep3, &Baseline::parse(&stale));
        assert_eq!(rep3.baselined, 2);
        assert_eq!(rep3.stale_baseline, 1);
        // 기준선에 없는 새 위반은 막지 못한다 — 새 것만 남는다.
        let mut rep4 = check(&violating_doc(), &cfg());
        rep4.violations.push(Violation {
            rule: "allow".to_string(),
            from: "c::ui::b".to_string(),
            to: "z::new".to_string(),
            component: "ui".to_string(),
            forbidden: "other".to_string(),
            kind: EdgeKind::Call,
        });
        apply_baseline(&mut rep4, &frozen);
        assert_eq!(rep4.violations.len(), 1);
        assert_eq!(rep4.violations[0].to, "z::new");
        assert_eq!(rep4.baselined, 2);
    }
}
