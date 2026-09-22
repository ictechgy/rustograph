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

/// 규칙 보고서.
#[derive(Debug, Serialize)]
pub struct RulesReport {
    pub violations: Vec<Violation>,
    /// 어느 컴포넌트에도 속하지 않은 모듈/크레이트.
    pub unmapped: Vec<String>,
    /// 추정(팬아웃) 간선은 "가능한" 의존이라 위반 증거가 못 된다 — 건너뛴 수를 남긴다.
    #[serde(skip_serializing_if = "crate::graph::is_zero")]
    pub skipped_tentative: usize,
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
}
