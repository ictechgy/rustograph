//! 그래프 위의 질의 — 수확 없이 Document만 소비한다.
//!
//! 여기서도 판정은 보수적이다: unreachable은 "보존 루트에서 도달 불가"라는
//! 그래프 사실이지 삭제 권고가 아니다.

use crate::graph::{Document, EdgeKind, Kind};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// 이웃 질의 한 건 — 정점과 그쪽으로의 간선 종류 전부.
#[derive(Debug, Serialize)]
pub struct Neighbor {
    pub id: String,
    pub edges: Vec<EdgeKind>,
    pub direction: String,
    pub depth: usize,
}

/// query의 결과 — 깊이·절단 여부를 항상 싣는다.
#[derive(Debug, Serialize)]
pub struct QueryResult {
    pub id: String,
    pub found: bool,
    pub depth: usize,
    pub truncated: bool,
    pub neighbors: Vec<Neighbor>,
}

/// 정점의 양방향 이웃을 depth까지 BFS로 찾는다.
pub fn query(doc: &Document, id: &str, depth: usize, max: usize) -> QueryResult {
    let found = doc.vertex_ids().contains(id);
    let out = doc.outgoing();
    let inc = doc.incoming();
    let mut neighbors = Vec::new();
    let mut seen: BTreeSet<&str> = BTreeSet::from([id]);
    let mut queue: VecDeque<(&str, usize)> = VecDeque::from([(id, 0)]);
    let mut truncated = false;
    while let Some((cur, d)) = queue.pop_front() {
        if d >= depth {
            continue;
        }
        // 이웃 정점 단위로 묶고 간선 종류는 전부 싣는다 — 하나만 고르면
        // 나머지 관계가 사라지고 정렬 타이에 따라 실행마다 달라진다.
        let mut step: Vec<(&str, &str)> = Vec::new(); // (neighbor, direction)
        for (_, to) in out.get(cur).into_iter().flatten() {
            step.push((to, "out"));
        }
        for (_, from) in inc.get(cur).into_iter().flatten() {
            step.push((from, "in"));
        }
        step.sort();
        step.dedup();
        for (nbr, dir) in step {
            neighbors.push(Neighbor {
                id: nbr.to_string(),
                edges: doc.edge_kinds_between(cur, nbr),
                direction: dir.to_string(),
                depth: d + 1,
            });
            if seen.insert(nbr) {
                if neighbors.len() >= max {
                    truncated = true;
                    break;
                }
                queue.push_back((nbr, d + 1));
            }
        }
        if truncated {
            break;
        }
    }
    QueryResult {
        id: id.to_string(),
        found,
        depth,
        truncated,
        neighbors,
    }
}

/// 역방향 전이 — 이 정점을 바꾸면 뭐가 깨지는가(incoming BFS).
#[derive(Debug, Serialize)]
pub struct ImpactResult {
    pub id: String,
    pub found: bool,
    pub depth: usize,
    pub truncated: bool,
    /// 깊이별 영향받는 정점.
    pub impacted: Vec<Neighbor>,
}

/// impact 질의 — query의 incoming 방향만 골라 깊이를 단다.
pub fn impact(doc: &Document, id: &str, depth: usize, max: usize) -> ImpactResult {
    let q = query(doc, id, depth, max);
    let impacted = q
        .neighbors
        .into_iter()
        .filter(|n| n.direction == "in")
        .collect();
    ImpactResult {
        id: q.id,
        found: q.found,
        depth: q.depth,
        truncated: q.truncated,
        impacted,
    }
}

/// Tarjan SCC — 2개 이상 정점의 강연결 컴포넌트, 또는 자기루프 단독 정점.
#[derive(Debug, Serialize)]
pub struct Cycle {
    pub members: Vec<String>,
    /// 사이클을 이루는 증거 간선.
    pub evidence: Vec<CycleEdge>,
}

/// cycles 보고서 — 순환과 함께 탐지에서 제외한 추정 간선 수를 싣는다.
#[derive(Debug, Serialize)]
pub struct CycleReport {
    pub cycles: Vec<Cycle>,
    /// 추정(팬아웃) 간선은 "가능한" 호출이라 순환 증거가 못 된다 — 제외하고 셈을 남긴다.
    #[serde(skip_serializing_if = "crate::graph::is_zero")]
    pub excluded_tentative: usize,
}

#[derive(Debug, Serialize)]
pub struct CycleEdge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
}

/// 의존 간선 중 확정분만의 outgoing 인접 목록 — 순환·규칙 검사용.
/// tentative는 "이 중 하나일 수 있다"라는 추정이라 증거가 못 된다.
fn firm_outgoing(doc: &Document) -> BTreeMap<&str, Vec<(&EdgeKind, &str)>> {
    let mut map: BTreeMap<&str, Vec<(&EdgeKind, &str)>> = BTreeMap::new();
    for e in &doc.edges {
        if e.kind.is_dependency() && !e.tentative {
            map.entry(e.from.as_str())
                .or_default()
                .push((&e.kind, e.to.as_str()));
        }
    }
    map
}

/// 순환을 찾는다 — 확정 의존 간선만 따라간다(추정 간선은 제외하고 센다).
pub fn cycles(doc: &Document) -> CycleReport {
    let excluded = doc
        .edges
        .iter()
        .filter(|e| e.kind.is_dependency() && e.tentative)
        .count();
    let out = firm_outgoing(doc);
    let ids: Vec<&str> = doc.vertex_ids().into_iter().collect();
    // Tarjan.
    let mut index = 0usize;
    let mut indices: BTreeMap<&str, usize> = BTreeMap::new();
    let mut low: BTreeMap<&str, usize> = BTreeMap::new();
    let mut on_stack: BTreeSet<&str> = BTreeSet::new();
    let mut stack: Vec<&str> = Vec::new();
    let mut sccs: Vec<Vec<String>> = Vec::new();

    // Tarjan 상태 기계는 인자가 많다 — 구조체로 나누면 재귀가 더 읽기 어렵다.
    #[allow(clippy::too_many_arguments)]
    fn strongconnect<'a>(
        v: &'a str,
        out: &BTreeMap<&'a str, Vec<(&'a EdgeKind, &'a str)>>,
        index: &mut usize,
        indices: &mut BTreeMap<&'a str, usize>,
        low: &mut BTreeMap<&'a str, usize>,
        stack: &mut Vec<&'a str>,
        on_stack: &mut BTreeSet<&'a str>,
        sccs: &mut Vec<Vec<String>>,
    ) {
        indices.insert(v, *index);
        low.insert(v, *index);
        *index += 1;
        stack.push(v);
        on_stack.insert(v);
        if let Some(succs) = out.get(v) {
            for (_, w) in succs {
                if !indices.contains_key(w) {
                    strongconnect(w, out, index, indices, low, stack, on_stack, sccs);
                    low.insert(v, low[v].min(low[w]));
                } else if on_stack.contains(w) {
                    low.insert(v, low[v].min(indices[w]));
                }
            }
        }
        if low[v] == indices[v] {
            let mut scc = Vec::new();
            while let Some(w) = stack.pop() {
                on_stack.remove(w);
                scc.push(w.to_string());
                if w == v {
                    break;
                }
            }
            sccs.push(scc);
        }
    }
    for id in &ids {
        if !indices.contains_key(id) {
            strongconnect(
                id,
                &out,
                &mut index,
                &mut indices,
                &mut low,
                &mut stack,
                &mut on_stack,
                &mut sccs,
            );
        }
    }
    let mut result = Vec::new();
    for scc in sccs {
        let is_cycle = scc.len() > 1
            || doc.edges.iter().any(|e| {
                e.kind.is_dependency() && !e.tentative && e.from == scc[0] && e.to == scc[0]
            });
        if !is_cycle {
            continue;
        }
        let member_set: BTreeSet<&str> = scc.iter().map(|s| s.as_str()).collect();
        let mut evidence: Vec<CycleEdge> = doc
            .edges
            .iter()
            .filter(|e| {
                e.kind.is_dependency()
                    && !e.tentative
                    && member_set.contains(e.from.as_str())
                    && member_set.contains(e.to.as_str())
            })
            .map(|e| CycleEdge {
                from: e.from.clone(),
                to: e.to.clone(),
                kind: e.kind,
            })
            .collect();
        evidence.sort_by(|a, b| (&a.from, &a.to).cmp(&(&b.from, &b.to)));
        let mut members = scc;
        members.sort();
        result.push(Cycle { members, evidence });
    }
    result.sort_by(|a, b| a.members.cmp(&b.members));
    CycleReport {
        cycles: result,
        excluded_tentative: excluded,
    }
}

/// dead 질의의 finding 하나.
#[derive(Debug, Serialize)]
pub struct Finding {
    pub id: String,
    pub kind: Kind,
    /// 그래프 사실 — 삭제 판정이 아니다.
    pub state: String,
    /// 왜 이 상태인가.
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<String>,
}

/// dead 보고서 — 사용한 루트를 항상 싣는다(도달성은 루트에 따라 달라진다).
#[derive(Debug, Serialize)]
pub struct DeadReport {
    pub roots: Vec<String>,
    pub findings: Vec<Finding>,
    /// 이 분석의 실측 한계 — 문서의 것과 별개로 판정 관련 한계.
    pub limitations: Vec<String>,
}

/// 루트에서 도달 가능한 정점 집합 — 의존 간선만.
pub fn reachable(doc: &Document) -> BTreeSet<&str> {
    let out = doc.outgoing();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut queue: VecDeque<&str> = VecDeque::new();
    for r in &doc.roots {
        if seen.insert(r.as_str()) {
            queue.push_back(r.as_str());
        }
    }
    while let Some(cur) = queue.pop_front() {
        for (_, to) in out.get(cur).into_iter().flatten() {
            if seen.insert(to) {
                queue.push_back(to);
            }
        }
    }
    seen
}

/// 루트에서 정점까지의 경로 하나를 찾는다(explain용).
pub fn explain_path(doc: &Document, id: &str) -> Option<Vec<String>> {
    let out = doc.outgoing();
    let mut parent: BTreeMap<&str, &str> = BTreeMap::new();
    let mut queue: VecDeque<&str> = VecDeque::new();
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for r in &doc.roots {
        if seen.insert(r.as_str()) {
            queue.push_back(r.as_str());
        }
    }
    while let Some(cur) = queue.pop_front() {
        if cur == id {
            let mut path = vec![cur.to_string()];
            let mut c = cur;
            while let Some(p) = parent.get(c) {
                path.push(p.to_string());
                c = p;
            }
            path.reverse();
            return Some(path);
        }
        for (_, to) in out.get(cur).into_iter().flatten() {
            if seen.insert(to) {
                parent.insert(to, cur);
                queue.push_back(to);
            }
        }
    }
    None
}

/// 도달 불가 심볼 보고 — state+reason만, 삭제 판정 없음.
/// 루트도 크레이트/모듈 정점도 리포트 대상이 아니다(그것들은 골격이다).
pub fn dead(doc: &Document) -> DeadReport {
    let reach = reachable(doc);
    let mut findings = Vec::new();
    for v in &doc.vertices {
        if matches!(v.kind, Kind::Crate | Kind::Module) {
            continue;
        }
        if reach.contains(v.id.as_str()) {
            continue;
        }
        findings.push(Finding {
            id: v.id.clone(),
            kind: v.kind,
            state: "unreachable".to_string(),
            reason: "not reachable from retention roots".to_string(),
            position: v.position.clone(),
        });
    }
    findings.sort_by(|a, b| a.id.cmp(&b.id));
    let mut limitations = Vec::new();
    let method_findings = findings.iter().filter(|f| f.kind == Kind::Method).count();
    if method_findings > 0 {
        limitations.push(format!(
            "{method_findings} unreachable methods may be invoked via trait objects, generics, or external crates — dispatch not visible to syntactic analysis"
        ));
    }
    DeadReport {
        roots: doc.roots.clone(),
        findings,
        limitations,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{document, Edge, Level, Vertex};

    fn v(id: &str, kind: Kind) -> Vertex {
        Vertex {
            id: id.to_string(),
            kind,
            krate: "c".to_string(),
            module: "c".to_string(),
            position: None,
            exported: false,
            generated: false,
            cfg: None,
            unsafe_: false,
        }
    }

    /// a->b->c 사슬 + 루트 없는 d, 추정 간선 e->a.
    fn doc() -> Document {
        document(
            Level::Symbol,
            ".".into(),
            None,
            vec!["c::a".into()],
            vec![
                v("c::a", Kind::Fn),
                v("c::b", Kind::Fn),
                v("c::c", Kind::Fn),
                v("c::d", Kind::Fn),
                v("c::e", Kind::Fn),
                v("c::x", Kind::Fn),
                v("c::y", Kind::Fn),
            ],
            vec![
                Edge::new("c::a".into(), "c::b".into(), EdgeKind::Call),
                Edge::new("c::b".into(), "c::c".into(), EdgeKind::Call),
                // 추정 간선만으로 이루어진 x->y->x 순환 — 확정이 아니라
                // 순환으로 보고되면 안 된다.
                Edge::maybe("c::x".into(), "c::y".into(), EdgeKind::Call),
                Edge::maybe("c::y".into(), "c::x".into(), EdgeKind::Call),
                // 추정 간선도 도달성에는 따라간다 — e는 a를 경유해 산다.
                Edge::maybe("c::e".into(), "c::a".into(), EdgeKind::Call),
            ],
            vec![],
        )
    }

    #[test]
    fn reachable_follows_tentative_but_roots_gate() {
        let d = doc();
        let r = reachable(&d);
        assert!(r.contains("c::a") && r.contains("c::b") && r.contains("c::c"));
        // e는 루트가 아니라 도달 불가 — 추정 간선의 출발점이 루트가 아니면 끝.
        assert!(!r.contains("c::e") && !r.contains("c::d"));
    }

    #[test]
    fn dead_reports_unreachable_with_reason() {
        let d = doc();
        let rep = dead(&d);
        let ids: Vec<&str> = rep.findings.iter().map(|f| f.id.as_str()).collect();
        assert!(ids.contains(&"c::d"));
        assert_eq!(rep.roots, vec!["c::a"]);
        assert_eq!(rep.findings[0].state, "unreachable");
        assert_eq!(rep.findings[0].reason, "not reachable from retention roots");
    }

    #[test]
    fn cycles_excludes_tentative_edges() {
        let d = doc();
        let rep = cycles(&d);
        // x<->y는 추정 간선뿐 — 순환으로 보고하지 않는다.
        assert!(rep.cycles.is_empty());
        assert_eq!(rep.excluded_tentative, 3);
    }

    #[test]
    fn cycles_finds_firm_cycle() {
        let mut d = doc();
        d.edges
            .push(Edge::new("c::a".into(), "c::d".into(), EdgeKind::Call));
        d.edges
            .push(Edge::new("c::d".into(), "c::a".into(), EdgeKind::Call));
        let rep = cycles(&d);
        assert_eq!(rep.cycles.len(), 1);
        assert!(rep.cycles[0].members.contains(&"c::a".to_string()));
        assert!(!rep.cycles[0].evidence.is_empty());
    }

    #[test]
    fn query_lists_all_edge_kinds_and_depth() {
        let d = doc();
        let q = query(&d, "c::a", 1, 10);
        assert!(q.found);
        let out: Vec<&Neighbor> = q
            .neighbors
            .iter()
            .filter(|n| n.direction == "out")
            .collect();
        assert!(out.iter().any(|n| n.id == "c::b"));
        let inn: Vec<&Neighbor> = q.neighbors.iter().filter(|n| n.direction == "in").collect();
        assert!(inn.iter().any(|n| n.id == "c::e"));
    }

    #[test]
    fn impact_walks_incoming() {
        let d = doc();
        let r = impact(&d, "c::c", 3, 10);
        let ids: Vec<&str> = r.impacted.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains(&"c::b"));
        assert!(ids.contains(&"c::a")); // 깊이 2
    }

    #[test]
    fn explain_finds_root_path() {
        let d = doc();
        let path = explain_path(&d, "c::c").unwrap();
        assert_eq!(path, vec!["c::a", "c::b", "c::c"]);
        assert!(explain_path(&d, "c::d").is_none());
    }
}
