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

/// 두 정점 사이의 경로 하나 — 각 홉이 어떤 간선을 건넜는지까지 싣는다.
#[derive(Debug, Serialize)]
pub struct Path {
    /// 경로를 이루는 정점 ID — 첫째가 from, 마지막이 to.
    pub vertices: Vec<String>,
    /// 홉마다 실제로 건넌 간선 종류 — vertices보다 하나 짧다.
    /// 같은 쌍에 여러 간선이 있으면 확정 간선·사전순 종류를 우선한다.
    pub edges: Vec<EdgeKind>,
    /// 추정(팬아웃) 간선이 하나라도 섞이면 이 경로는 "가능한" 경로다.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub tentative: bool,
}

/// paths 질의 결과 — 탐색이 예산에 잘리면 truncated가 참이다.
#[derive(Debug, Serialize)]
pub struct PathsReport {
    pub from: String,
    pub to: String,
    /// 두 정점이 모두 존재하고 경로가 하나 이상 발견됐는가.
    pub found: bool,
    /// 경로 상한이나 탐색 예산 때문에 잘렸다 — 보고된 목록이 전부가 아니다.
    pub truncated: bool,
    /// 탐색이 펼친 정점 수 — 예산 소비량 측정.
    pub expanded: usize,
    pub paths: Vec<Path>,
}

/// A→B 경로 탐색 — 의존 간선만 따라가는 bounded BFS.
/// 경로는 정점 재방문이 없는 단순 경로만이고, BFS 특성상 짧은 것부터
/// 나온다. 추정 간선도 따라가되 경로에 tentative로 표시한다 —
/// "도달 가능할 수 있다"는 사실이 보고 가치다.
pub fn paths(doc: &Document, from: &str, to: &str, max_paths: usize, budget: usize) -> PathsReport {
    let ids = doc.vertex_ids();
    // 인접 목록: (from,to) 쌍에 간선이 여러 개면 확정·사전순 하나만 남긴다 —
    // 같은 경로를 간선 종류만큼 중복 보고하지 않기 위함이다. 끝점이 없는
    // 간선은 건너뛴다 — 비형식 문서의 dangling 간선이 경로에 없는
    // 정점을 끼워 넣는 것을 막는다.
    let mut adj: BTreeMap<&str, Vec<(&str, EdgeKind, bool)>> = BTreeMap::new();
    for e in &doc.edges {
        if e.kind.is_dependency() && ids.contains(e.from.as_str()) && ids.contains(e.to.as_str()) {
            adj.entry(e.from.as_str())
                .or_default()
                .push((e.to.as_str(), e.kind, e.tentative));
        }
    }
    for outs in adj.values_mut() {
        outs.sort_by(|a, b| (a.0, a.2, a.1).cmp(&(b.0, b.2, b.1)));
        outs.dedup_by(|a, b| a.0 == b.0); // 정렬 후 첫 항목이 확정·최소 종류.
    }
    let mut report = PathsReport {
        from: from.to_string(),
        to: to.to_string(),
        found: false,
        truncated: false,
        expanded: 0,
        paths: Vec::new(),
    };
    if !ids.contains(from) || !ids.contains(to) {
        return report; // 끝점이 없으면 경로도 없다 — 호출자가 404를 처리한다.
    }
    if from == to {
        // 자기 경로도 max_paths=0이면 보고할 수 없다 — 0은 "없다"가
        // 아니라 "제한"이니 truncated로 표시한다.
        report.found = max_paths > 0;
        report.truncated = max_paths == 0;
        if max_paths > 0 {
            report.paths.push(Path {
                vertices: vec![from.to_string()],
                edges: Vec::new(),
                tentative: false,
            });
        }
        return report;
    }
    // 부분 경로 상태 — 경로 내부 재방문 금지로 단순 경로만 나온다.
    struct State {
        vertices: Vec<String>,
        edges: Vec<EdgeKind>,
        tentative: bool,
    }
    let mut queue: VecDeque<State> = VecDeque::from([State {
        vertices: vec![from.to_string()],
        edges: Vec::new(),
        tentative: false,
    }]);
    while let Some(s) = queue.pop_front() {
        let cur = s.vertices.last().expect("state is never empty").clone();
        // 목적지 확인이 예산 검사보다 먼저다 — 대기 중인 완성 경로는
        // 예산을 쓰지 않고 수확한다. 경로 수 상한을 넘는 하나가 더
        // 보이면 잘림이 확정이다 — 더 찾지 않고 멈춘다.
        if cur == to {
            if report.paths.len() >= max_paths {
                report.truncated = true;
                break;
            }
            report.paths.push(Path {
                vertices: s.vertices,
                edges: s.edges,
                tentative: s.tentative,
            });
            continue; // 목적지 도달 — 그 너머는 같은 경로의 연장일 뿐이다.
        }
        // 확장 예산이 찼으면 큐를 펼치지 않고 비우며 목적지만 거둔다 —
        // 비목적지 상태 뒤에 대기 중인 완성 경로가 버려지면 안 된다.
        if report.expanded >= budget {
            report.truncated = true;
            continue;
        }
        report.expanded += 1;
        for (nbr, kind, tent) in adj.get(cur.as_str()).into_iter().flatten() {
            if s.vertices.iter().any(|v| v.as_str() == *nbr) {
                continue;
            }
            let mut next = State {
                vertices: s.vertices.clone(),
                edges: s.edges.clone(),
                tentative: s.tentative || *tent,
            };
            next.vertices.push((*nbr).to_string());
            next.edges.push(*kind);
            queue.push_back(next);
        }
    }
    // 남은 큐가 있으면 처리하지 못한 경로가 있다 — 잘렸다고 표시한다.
    // max_paths만 차고 큐가 비었으면 자연 종료라 truncated가 아니다.
    if !queue.is_empty() {
        report.truncated = true;
    }
    report.found = !report.paths.is_empty();
    report
}

/// 검색 히트 하나 — 정점과 그 식별 정보.
#[derive(Debug, Serialize)]
pub struct SearchHit {
    pub id: String,
    pub kind: Kind,
    pub module: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<String>,
}

/// search 질의 결과 — 상한에 잘리면 truncated.
#[derive(Debug, Serialize)]
pub struct SearchReport {
    pub query: String,
    pub truncated: bool,
    pub matches: Vec<SearchHit>,
}

/// 정점 검색 — 정확 일치 → `::query` 꼬리 일치 → 부분 문자열 순으로 점수를
/// 매기고 같은 점수 안에서는 ID 정렬이다. 에이전트가 "이름만 아는" 정점을
/// 찾는 입구다 — 정확 ID 없이 query/impact를 부르기 전의 단계.
pub fn search(doc: &Document, query: &str, max: usize) -> SearchReport {
    let suffix = format!("::{query}");
    let q_lower = query.to_lowercase();
    let mut scored: Vec<(u8, &crate::graph::Vertex)> = doc
        .vertices
        .iter()
        .filter_map(|v| {
            if v.id == query {
                Some((0, v))
            } else if v.id.ends_with(&suffix) {
                Some((1, v))
            } else if v.id.to_lowercase().contains(&q_lower) {
                Some((2, v))
            } else {
                None
            }
        })
        .collect();
    scored.sort_by(|a, b| (a.0, &a.1.id).cmp(&(b.0, &b.1.id)));
    let truncated = scored.len() > max;
    scored.truncate(max);
    SearchReport {
        query: query.to_string(),
        truncated,
        matches: scored
            .into_iter()
            .map(|(_, v)| SearchHit {
                id: v.id.clone(),
                kind: v.kind,
                module: v.module.clone(),
                position: v.position.clone(),
            })
            .collect(),
    }
}

/// 정점 ID를 하나로 확정한다. 정확 일치가 없으면 검색 후보를 모아
/// Err로 돌린다 — 조용히 아무 후보나 고르면 호출자가 엉뚱한 정점을
/// 본다. 실패 메시지에 후보를 싣는 것이 애매 거부의 전부다.
pub fn resolve_id(doc: &Document, id: &str) -> Result<String, Vec<String>> {
    if doc.vertex_ids().contains(id) {
        return Ok(id.to_string());
    }
    Err(search(doc, id, 10)
        .matches
        .into_iter()
        .map(|h| h.id)
        .collect())
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

    #[test]
    fn paths_finds_short_paths_with_edge_kinds() {
        let mut d = doc();
        // 두 갈래 경로 — a->b->c와 a->x->c.
        d.edges
            .push(Edge::new("c::a".into(), "c::x".into(), EdgeKind::Call));
        d.edges
            .push(Edge::new("c::x".into(), "c::c".into(), EdgeKind::Call));
        let r = paths(&d, "c::a", "c::c", 10, 10_000);
        assert!(r.found && !r.truncated);
        let vs: Vec<Vec<String>> = r.paths.iter().map(|p| p.vertices.clone()).collect();
        assert!(vs.contains(&vec![
            "c::a".to_string(),
            "c::b".to_string(),
            "c::c".to_string()
        ]));
        assert!(vs.contains(&vec![
            "c::a".to_string(),
            "c::x".to_string(),
            "c::c".to_string()
        ]));
        assert_eq!(r.paths[0].edges, vec![EdgeKind::Call, EdgeKind::Call]);
        assert!(!r.paths.iter().any(|p| p.tentative));
    }

    #[test]
    fn paths_respects_budget_and_marks_tentative() {
        let d = doc();
        // 예산 1 — 출발점 하나만 펼치고 멈춘다.
        let r = paths(&d, "c::a", "c::c", 10, 1);
        assert!(!r.found && r.truncated && r.expanded == 1);
        // 추정 간선을 건너는 경로는 tentative로 표시된다 — "가능한" 경로다.
        let r2 = paths(&d, "c::e", "c::c", 10, 10_000);
        assert!(r2.found && r2.paths[0].tentative);
        // 없는 끝점은 found=false — 호출자가 후보를 제안한다.
        assert!(!paths(&d, "c::a", "nope", 10, 10).found);
        // 자기 자신은 길이 0 경로 하나.
        let r3 = paths(&d, "c::a", "c::a", 10, 10);
        assert!(r3.found && r3.paths[0].vertices == vec!["c::a".to_string()]);
        // max_paths가 차면 truncated — 보고된 목록이 전부가 아니다.
        let mut d2 = doc();
        d2.edges
            .push(Edge::new("c::a".into(), "c::x".into(), EdgeKind::Call));
        d2.edges
            .push(Edge::new("c::x".into(), "c::c".into(), EdgeKind::Call));
        let r4 = paths(&d2, "c::a", "c::c", 1, 10_000);
        assert_eq!(r4.paths.len(), 1);
        assert!(r4.truncated);
    }

    #[test]
    fn paths_direct_edge_survives_minimal_budget() {
        let d = doc();
        // 직접 간선 a→b는 예산 1로도 찾아야 한다 — 예산은 확장에만
        // 쓰이고, 큐에 있는 완성 경로의 수확은 예산을 쓰지 않는다.
        let r = paths(&d, "c::a", "c::b", 10, 1);
        assert!(r.found && !r.truncated);
        assert_eq!(
            r.paths[0].vertices,
            vec!["c::a".to_string(), "c::b".to_string()]
        );
    }

    #[test]
    fn paths_drains_queued_destination_after_budget() {
        // a->b, a->x에 예산 1 — a를 펼치면 큐는 [b, x]. 예산이 찼다고
        // 멈추면 비목적지 b 뒤에 대기 중인 완성 경로 x를 버린다.
        // 큐를 비우며 목적지만 거둬야 한다 — b의 미확장은 truncated로.
        let mut d = doc();
        d.edges
            .push(Edge::new("c::a".into(), "c::x".into(), EdgeKind::Call));
        let r = paths(&d, "c::a", "c::x", 10, 1);
        assert!(r.found);
        assert_eq!(
            r.paths[0].vertices,
            vec!["c::a".to_string(), "c::x".to_string()]
        );
        assert!(r.truncated); // b를 펼치지 못했다 — 그 너머 경로는 모른다.
    }

    #[test]
    fn paths_zero_max_is_truncated_not_silent() {
        let d = doc();
        // max=0은 "경로 없음"이 아니라 "보고할 수 없음"이다.
        let r = paths(&d, "c::a", "c::a", 0, 10);
        assert!(!r.found && r.truncated && r.paths.is_empty());
        let r2 = paths(&d, "c::a", "c::b", 0, 10);
        assert!(!r2.found && r2.truncated);
    }

    #[test]
    fn paths_ignores_dangling_intermediate_edges() {
        // 비형식 문서 — 없는 정점을 가리키는 간선이 끼어 있으면 그
        // 간선을 따라가 만든 경로는 유령 정점을 품는다. 건너뛴다.
        let mut d = doc();
        d.edges
            .push(Edge::new("c::a".into(), "c::ghost".into(), EdgeKind::Call));
        d.edges
            .push(Edge::new("c::ghost".into(), "c::c".into(), EdgeKind::Call));
        let r = paths(&d, "c::a", "c::c", 10, 10_000);
        assert!(r.found);
        // 모든 경로의 정점은 문서에 존재하는 정점뿐이다.
        let ids = d.vertex_ids();
        for p in &r.paths {
            for v in &p.vertices {
                assert!(ids.contains(v.as_str()), "ghost vertex {v} in path");
            }
        }
    }

    #[test]
    fn paths_dedups_multi_edge_pairs() {
        let mut d = doc();
        // 같은 쌍에 두 종류의 간선 — 정점 경로는 중복되면 안 된다.
        d.edges.push(Edge::new(
            "c::a".into(),
            "c::b".into(),
            EdgeKind::References,
        ));
        d.edges
            .push(Edge::maybe("c::a".into(), "c::b".into(), EdgeKind::Call));
        let r = paths(&d, "c::a", "c::c", 10, 10_000);
        assert_eq!(r.paths.len(), 1);
        // 같은 쌍의 홉은 하나 — 확정·최소 종류가 선택된다.
        assert_eq!(r.paths[0].edges.len(), 2);
        assert!(!r.paths[0].tentative);
    }

    #[test]
    fn search_scores_exact_suffix_then_substring() {
        let d = doc();
        // 정확 일치가 항상 먼저다.
        let r = search(&d, "c::b", 10);
        assert_eq!(r.matches[0].id, "c::b");
        // `::b` 꼬리 일치 — 부분 문자열 "b"보다 높은 점수.
        let r2 = search(&d, "b", 10);
        assert_eq!(r2.matches[0].id, "c::b");
        // 부분 문자열은 전부에 걸리고 max로 잘린다.
        let r3 = search(&d, "c::", 3);
        assert_eq!(r3.matches.len(), 3);
        assert!(r3.truncated);
        // 정렬은 결정적 — ID 오름차순.
        assert_eq!(r3.matches[0].id, "c::a");
    }

    #[test]
    fn resolve_id_accepts_exact_and_refuses_with_candidates() {
        let d = doc();
        assert_eq!(resolve_id(&d, "c::a"), Ok("c::a".to_string()));
        // 부분 문자열은 조용히 고르지 않고 후보를 돌린다.
        let err = resolve_id(&d, "c::").unwrap_err();
        assert!(err.contains(&"c::a".to_string()));
        // 후보조차 없으면 빈 목록 — 호출자가 "not found"만 보고한다.
        assert_eq!(resolve_id(&d, "zzz").unwrap_err(), Vec::<String>::new());
    }
}
