//! 그래프 위의 질의 — 수확 없이 Document만 소비한다.
//!
//! 여기서도 판정은 보수적이다: unreachable은 "보존 루트에서 도달 불가"라는
//! 그래프 사실이지 삭제 권고가 아니다.

use crate::graph::{Document, Edge, EdgeKind, Kind};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::OnceLock;

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
/// 같은 이웃·방향은 최단 거리에서 한 번만 보고하고 시작 정점은 제외한다.
pub fn query(doc: &Document, id: &str, depth: usize, max: usize) -> QueryResult {
    QueryIndex::new(doc).query(id, depth, max)
}

/// 방향별 간선 종류를 색인한다 — 역방향 종류나 contains를 증거에 섞지 않는다.
type NeighborIndex<'a> = BTreeMap<&'a str, BTreeMap<(&'a str, &'static str), BTreeSet<EdgeKind>>>;

/// 경로 탐색 인접 목록 — (from,to) 쌍마다 간선 하나(확정·최소 종류).
type PathAdjacency<'a> = BTreeMap<&'a str, Vec<(&'a str, EdgeKind, bool)>>;

/// 도달성 인접 목록 — 의존 간선의 outgoing([`Document::outgoing`]).
type ReachMap<'a> = BTreeMap<&'a str, Vec<(&'a EdgeKind, &'a str)>>;

#[cfg(test)]
thread_local! {
    /// 색인을 만들며 살핀 정점·간선 수 — 같은 문서에 질의를 되풀이할 때 색인을 다시 만들지
    /// 않는지 단위 테스트가 센다(테스트 스레드마다 따로).
    static INDEXED: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// 테스트용 — 이 스레드에서 지금까지 색인 구축이 살핀 정점·간선 수.
#[cfg(test)]
pub(crate) fn indexed() -> usize {
    INDEXED.with(std::cell::Cell::get)
}

/// 테스트용 — 색인 구축이 정점·간선 `n`개를 살폈다고 센다.
#[cfg(test)]
fn note_indexed(n: usize) {
    INDEXED.with(|c| c.set(c.get() + n));
}

#[cfg(test)]
thread_local! {
    /// 경로 탐색이 만든 경로 정점 문자열 수 — 부분 경로마다 경로 전체를 복제하지 않고 보고할 경로만
    /// 만드는지 단위 테스트가 센다(테스트 스레드마다 따로).
    static PATH_VERTEX_COPIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// 테스트용 — 이 스레드에서 지금까지 경로 탐색이 만든 경로 정점 문자열 수.
#[cfg(test)]
fn path_vertex_copies() -> usize {
    PATH_VERTEX_COPIES.with(std::cell::Cell::get)
}

/// 테스트용 — 경로 정점 문자열 `n`개를 만들었다고 센다.
#[cfg(test)]
fn note_path_copies(n: usize) {
    PATH_VERTEX_COPIES.with(|c| c.set(c.get() + n));
}

/// 한 문서 위의 질의 색인 — 정점 ID 집합, 양방향·역방향 이웃, 경로 인접, 도달 인접을 처음
/// 쓰는 질의가 한 번 만들고 같은 문서의 다음 질의가 재사용한다. MCP 서버는 불변 스냅샷
/// 하나에 query·impact를 세션 내내 묻는다 — 호출마다 전 간선을 다시 색인하면 답이 이웃 몇
/// 개여도 호출당 O(E log V)였다. [`query`]·[`impact`]·[`paths`]·[`resolve_id`]·[`dead`]는
/// 이 색인을 한 번 쓰고 버리는 래퍼라 CLI와 MCP가 같은 경로로 답한다.
pub struct QueryIndex<'a> {
    doc: &'a Document,
    ids: OnceLock<BTreeSet<&'a str>>,
    neighbors: OnceLock<NeighborIndex<'a>>,
    dependers: OnceLock<NeighborIndex<'a>>,
    adjacency: OnceLock<PathAdjacency<'a>>,
    reach: OnceLock<ReachMap<'a>>,
}

impl<'a> QueryIndex<'a> {
    /// 아직 아무것도 색인하지 않은 색인 — 각 색인은 그것을 쓰는 첫 질의가 만든다.
    pub fn new(doc: &'a Document) -> Self {
        QueryIndex {
            doc,
            ids: OnceLock::new(),
            neighbors: OnceLock::new(),
            dependers: OnceLock::new(),
            adjacency: OnceLock::new(),
            reach: OnceLock::new(),
        }
    }

    /// 이 ID의 정점이 문서에 있는가.
    pub fn has_vertex(&self, id: &str) -> bool {
        self.vertex_set().contains(id)
    }

    /// [`query`]와 같은 양방향 이웃.
    pub fn query(&self, id: &str, depth: usize, max: usize) -> QueryResult {
        walk_neighbors(self, id, depth, max, false)
    }

    /// [`impact`]와 같은 역방향 전이 — 처음부터 incoming만 따라간다.
    pub fn impact(&self, id: &str, depth: usize, max: usize) -> ImpactResult {
        let q = walk_neighbors(self, id, depth, max, true);
        ImpactResult {
            id: q.id,
            found: q.found,
            depth: q.depth,
            truncated: q.truncated,
            impacted: q.neighbors,
        }
    }

    /// [`paths`]와 같은 경로 탐색.
    pub fn paths(&self, from: &str, to: &str, max_paths: usize, budget: usize) -> PathsReport {
        paths_bounded(self, from, to, max_paths, budget, &mut 0)
    }

    /// [`resolve_id`]와 같은 ID 확정 — 정확 일치가 없으면 검색 후보를 돌린다.
    pub fn resolve_id(&self, id: &str) -> Result<String, Vec<String>> {
        if self.has_vertex(id) {
            return Ok(id.to_string());
        }
        Err(search(self.doc, id, 10)
            .matches
            .into_iter()
            .map(|h| h.id)
            .collect())
    }

    /// `roots`를 보존 루트로 삼은 [`dead`] — 루트만 바꾼 문서 사본을 만들지 않는다.
    pub fn dead(&self, roots: &[String]) -> Result<DeadReport, String> {
        // 레벨이 모자라면 도달 인접을 만들기 전에 거절한다.
        self.doc.require_level(crate::graph::Level::Symbol)?;
        Ok(dead_with(self.doc, self.reach_map(), roots))
    }

    /// 정점 ID 집합 — 같은 ID의 cfg 변형은 하나다.
    fn vertex_set(&self) -> &BTreeSet<&'a str> {
        self.ids.get_or_init(|| {
            #[cfg(test)]
            note_indexed(self.doc.vertices.len());
            self.doc.vertex_ids()
        })
    }

    /// 방향별 이웃 색인 — impact는 역방향만, query는 양방향을 쓴다.
    fn neighbor_map(&self, incoming_only: bool) -> &NeighborIndex<'a> {
        let slot = if incoming_only {
            &self.dependers
        } else {
            &self.neighbors
        };
        slot.get_or_init(|| neighbor_index(self.doc, self.vertex_set(), incoming_only))
    }

    /// 경로 탐색 인접 목록.
    fn path_adjacency(&self) -> &PathAdjacency<'a> {
        self.adjacency
            .get_or_init(|| adjacency_of(self.doc, self.vertex_set()))
    }

    /// 보존 루트 BFS가 따라가는 도달성 인접 목록.
    fn reach_map(&self) -> &ReachMap<'a> {
        self.reach.get_or_init(|| {
            #[cfg(test)]
            note_indexed(self.doc.edges.len());
            self.doc.outgoing()
        })
    }
}

fn neighbor_index<'a>(
    doc: &'a Document,
    ids: &BTreeSet<&str>,
    incoming_only: bool,
) -> NeighborIndex<'a> {
    let mut index: NeighborIndex<'a> = BTreeMap::new();
    for edge in &doc.edges {
        #[cfg(test)]
        note_indexed(1);
        if !edge.kind.is_dependency()
            || !ids.contains(edge.from.as_str())
            || !ids.contains(edge.to.as_str())
        {
            continue;
        }
        index
            .entry(&edge.to)
            .or_default()
            .entry((&edge.from, "in"))
            .or_default()
            .insert(edge.kind);
        if !incoming_only {
            index
                .entry(&edge.from)
                .or_default()
                .entry((&edge.to, "out"))
                .or_default()
                .insert(edge.kind);
        }
    }
    index
}

/// BFS의 한 깊이를 먼저 합쳐 다이아몬드 경로의 모든 최단 증거를 보존한다.
/// 상한은 실제 보고 항목 수에 적용하고, 생략할 항목이 있어야 truncated다.
fn walk_neighbors(
    index: &QueryIndex<'_>,
    id: &str,
    depth: usize,
    max: usize,
    incoming_only: bool,
) -> QueryResult {
    let mut result = QueryResult {
        id: id.to_string(),
        found: index.has_vertex(id),
        depth,
        truncated: false,
        neighbors: Vec::new(),
    };
    if !result.found || depth == 0 {
        return result;
    }
    let edges_of = index.neighbor_map(incoming_only);
    let mut seen = BTreeSet::from([id]);
    let mut reported = BTreeSet::new();
    let mut frontier = BTreeSet::from([id]);
    for distance in 1..=depth {
        if frontier.is_empty() {
            break;
        }
        let mut step: BTreeMap<(&str, &str), BTreeSet<EdgeKind>> = BTreeMap::new();
        for current in frontier {
            for (&key, kinds) in edges_of.get(current).into_iter().flatten() {
                if key.0 != id && !reported.contains(&key) {
                    step.entry(key).or_default().extend(kinds);
                }
            }
        }
        frontier = BTreeSet::new();
        for ((neighbor, direction), kinds) in step {
            if result.neighbors.len() == max {
                result.truncated = true;
                return result;
            }
            reported.insert((neighbor, direction));
            result.neighbors.push(Neighbor {
                id: neighbor.to_string(),
                edges: kinds.into_iter().collect(),
                direction: direction.to_string(),
                depth: distance,
            });
            if seen.insert(neighbor) {
                frontier.insert(neighbor);
            }
        }
    }
    result
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

/// impact 질의 — 처음부터 incoming만 따라가야 공유 의존의 다른 사용자가 섞이지 않는다.
pub fn impact(doc: &Document, id: &str, depth: usize, max: usize) -> ImpactResult {
    QueryIndex::new(doc).impact(id, depth, max)
}

/// 두 정점 사이의 경로 하나 — 각 홉이 어떤 간선을 건넜는지까지 싣는다.
#[derive(Debug, Serialize)]
pub struct Path {
    /// 경로를 이루는 정점 ID — 첫째가 from, 마지막이 to.
    pub vertices: Vec<String>,
    /// 홉마다 실제로 건넌 간선 종류 — vertices보다 하나 짧다.
    /// 같은 쌍에 여러 간선이 있으면 확정·최소 종류(EdgeKind의
    /// 선언 순서 Ord)를 우선한다.
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
    QueryIndex::new(doc).paths(from, to, max_paths, budget)
}

/// 경로 탐색 인접 목록: (from,to) 쌍에 간선이 여러 개면 확정·최소 종류
/// (EdgeKind의 선언 순서 Ord) 하나만 남긴다 — 같은 경로를 간선
/// 종류만큼 중복 보고하지 않기 위함이다. 끝점이 없는 간선은 건너뛴다
/// — 비형식 문서의 dangling 간선이 경로에 없는 정점을 끼워 넣는
/// 것을 막는다.
fn adjacency_of<'a>(doc: &'a Document, ids: &BTreeSet<&str>) -> PathAdjacency<'a> {
    let mut adj: PathAdjacency<'a> = BTreeMap::new();
    for e in &doc.edges {
        #[cfg(test)]
        note_indexed(1);
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
    adj
}

fn paths_bounded(
    index: &QueryIndex<'_>,
    from: &str,
    to: &str,
    max_paths: usize,
    budget: usize,
    peak_pending: &mut usize,
) -> PathsReport {
    let mut report = PathsReport {
        from: from.to_string(),
        to: to.to_string(),
        found: false,
        truncated: false,
        expanded: 0,
        paths: Vec::new(),
    };
    if !index.has_vertex(from) || !index.has_vertex(to) {
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
    let adj = index.path_adjacency();
    // 부분 경로 상태는 아레나의 칸 번호로 큐에 넣는다 — 경로 내부 재방문 금지로 단순 경로만 나온다.
    let mut steps = vec![PathStep {
        vertex: from,
        via: None,
        tentative: false,
    }];
    let mut queue: VecDeque<usize> = VecDeque::from([0]);
    let mut pending_states = 1usize;
    let mut pending_paths = 0usize;
    *peak_pending = 1;
    while let Some(at) = queue.pop_front() {
        let cur = steps[at].vertex;
        if cur == to {
            pending_paths -= 1;
        } else {
            pending_states -= 1;
        }
        // 목적지 확인이 예산 검사보다 먼저다 — 대기 중인 완성 경로는
        // 예산을 쓰지 않고 수확한다. 경로 수 상한을 넘는 하나가 더
        // 보이면 잘림이 확정이다 — 더 찾지 않고 멈춘다.
        if cur == to {
            if report.paths.len() >= max_paths {
                report.truncated = true;
                break;
            }
            report.paths.push(path_ending_at(&steps, at));
            continue; // 목적지 도달 — 그 너머는 같은 경로의 연장일 뿐이다.
        }
        // 확장 예산이 찼으면 큐를 펼치지 않고 비우며 목적지만 거둔다 —
        // 비목적지 상태 뒤에 대기 중인 완성 경로가 버려지면 안 된다.
        if report.expanded >= budget {
            report.truncated = true;
            continue;
        }
        report.expanded += 1;
        for (nbr, kind, tent) in adj.get(cur).into_iter().flatten() {
            if path_visits(&steps, at, nbr) {
                continue;
            }
            // 확장할 수 없는 부분 경로는 칸을 만들기 전에 버린다. 완성 경로는
            // 별도 슬롯을 사용해 예산이 찼어도 이미 찾은 증거를 보존한다.
            if *nbr != to && report.expanded.saturating_add(pending_states) >= budget {
                report.truncated = true;
                continue;
            }
            if *nbr == to && report.paths.len().saturating_add(pending_paths) > max_paths {
                report.truncated = true;
                continue;
            }
            let tentative = steps[at].tentative || *tent;
            steps.push(PathStep {
                vertex: nbr,
                via: Some((at, *kind)),
                tentative,
            });
            if *nbr == to {
                pending_paths += 1;
            } else {
                pending_states += 1;
            }
            queue.push_back(steps.len() - 1);
            *peak_pending = (*peak_pending).max(queue.len());
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

/// 경로 탐색 상태 한 칸 — 부분 경로는 앞 칸을 거슬러 올라가 읽는다. 상태마다 경로 전체를 복제하면
/// 메모리가 예산 × 경로 길이가 된다(30층×폭 6 DAG, 예산 100만에 514MB). 한 칸은 수십 바이트이고
/// 칸 수는 예산과 경로 상한으로 묶인다(비목적지 칸은 예산 이하, 목적지 칸은 상한 + 1 이하).
struct PathStep<'s> {
    /// 이 상태가 끝나는 정점.
    vertex: &'s str,
    /// 앞 상태의 칸 번호와 그 정점에서 건넌 간선 종류 — 출발 상태는 None이다.
    via: Option<(usize, EdgeKind)>,
    /// 경로에 추정 간선이 하나라도 섞였는가.
    tentative: bool,
}

/// 칸 `at`에서 끝나는 부분 경로가 `vertex`를 지나는가 — 단순 경로만 내기 위한 재방문 검사다.
/// 경로 길이만큼 거슬러 오른다(예전 복제본 선형 탐색과 같은 비용).
fn path_visits(steps: &[PathStep<'_>], at: usize, vertex: &str) -> bool {
    let mut cursor = Some(at);
    while let Some(i) = cursor {
        if steps[i].vertex == vertex {
            return true;
        }
        cursor = steps[i].via.map(|(parent, _)| parent);
    }
    false
}

/// 칸 `at`에서 끝나는 경로를 보고 형태로 만든다 — 정점 문자열은 보고하는 경로에만 만든다.
fn path_ending_at(steps: &[PathStep<'_>], at: usize) -> Path {
    let (mut vertices, mut edges) = (Vec::new(), Vec::new());
    let mut cursor = Some(at);
    while let Some(i) = cursor {
        vertices.push(steps[i].vertex.to_string());
        cursor = match steps[i].via {
            Some((parent, kind)) => {
                edges.push(kind);
                Some(parent)
            }
            None => None,
        };
    }
    vertices.reverse();
    edges.reverse();
    #[cfg(test)]
    note_path_copies(vertices.len());
    Path {
        vertices,
        edges,
        tentative: steps[at].tentative,
    }
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
    QueryIndex::new(doc).resolve_id(id)
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

/// 확정 의존 간선만 남긴 번호 그래프 — 순환 탐지가 ID 문자열 대신 번호로 돈다.
/// tentative는 "이 중 하나일 수 있다"라는 추정이라 증거가 못 된다.
/// 번호 `0..roots`는 정렬된 정점 ID(Tarjan 시작점)이고, 그 뒤는 간선에만 나오는
/// dangling 끝점이다. 저장 문서(`--graph`)에서 정점이 가리키는 dangling 끝점도
/// 방문해 SCC 멤버로 내 왔다 — 그 출력을 지키려면 끝점에도 번호가 있어야 한다.
struct FirmGraph<'a> {
    /// 번호 → ID.
    names: Vec<&'a str>,
    /// Tarjan 시작점 수 — 앞쪽 번호가 정점이다.
    roots: usize,
    /// 번호 → 후속 번호. doc.edges 순서라 방문 순서가 입력만으로 정해진다.
    successors: Vec<Vec<usize>>,
    /// 확정 의존 간선을 doc.edges 순서로 — (from 번호, to 번호, 간선). 증거 분배용.
    edges: Vec<(usize, usize, &'a Edge)>,
    /// 번호 → 확정 의존 자기 루프가 있는가. 정점 하나짜리 SCC의 순환 여부다.
    self_loop: Vec<bool>,
}

impl<'a> FirmGraph<'a> {
    fn new(doc: &'a Document) -> FirmGraph<'a> {
        let mut names: Vec<&str> = doc.vertex_ids().into_iter().collect();
        let roots = names.len();
        let mut numbers: HashMap<&str, usize> =
            names.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        let mut edges = Vec::new();
        for e in doc
            .edges
            .iter()
            .filter(|e| e.kind.is_dependency() && !e.tentative)
        {
            let from = number_of(&mut numbers, &mut names, &e.from);
            let to = number_of(&mut numbers, &mut names, &e.to);
            edges.push((from, to, e));
        }
        let mut successors = vec![Vec::new(); names.len()];
        let mut self_loop = vec![false; names.len()];
        for &(from, to, _) in &edges {
            successors[from].push(to);
            self_loop[from] |= from == to;
        }
        FirmGraph {
            names,
            roots,
            successors,
            edges,
            self_loop,
        }
    }
}

/// ID의 번호 — 처음 보는 ID(dangling 끝점)면 다음 번호를 준다.
fn number_of<'a>(
    numbers: &mut HashMap<&'a str, usize>,
    names: &mut Vec<&'a str>,
    id: &'a str,
) -> usize {
    *numbers.entry(id).or_insert_with(|| {
        names.push(id);
        names.len() - 1
    })
}

/// 반복형 Tarjan의 상태 — 정점 번호로 색인한다.
/// 재귀판은 DFS 경로 길이만큼 호출 스택을 써서, 수만 정점 고리나 긴 호출 사슬에서
/// 스레드 스택을 넘겨 프로세스가 abort했다(종료 코드 134, MCP 서버 소멸). 그래서
/// 호출 스택 대신 (정점, 다음 후속 위치) 프레임을 힙에 쌓는다. 시작점 순서(정렬된
/// 정점 ID)와 후속 순서(doc.edges)가 재귀판과 같아 SCC와 그 안의 순서도 같다.
struct Tarjan<'g> {
    successors: &'g [Vec<usize>],
    /// 방문 순번. `UNSEEN`이면 아직 안 봤다.
    order: Vec<usize>,
    /// DFS 부분 트리에서 Tarjan 스택 위 정점으로 닿는 가장 작은 순번.
    low: Vec<usize>,
    on_stack: Vec<bool>,
    stack: Vec<usize>,
    /// 다음에 매길 순번.
    next: usize,
    /// 완성된 SCC — 발견 순서, 멤버는 스택에서 꺼낸 순서.
    found: Vec<Vec<usize>>,
}

impl Tarjan<'_> {
    const UNSEEN: usize = usize::MAX;

    /// 시작점마다 아직 안 본 것에서 DFS를 연다.
    fn strongly_connected(firm: &FirmGraph) -> Vec<Vec<usize>> {
        let n = firm.successors.len();
        let mut state = Tarjan {
            successors: &firm.successors,
            order: vec![Self::UNSEEN; n],
            low: vec![0; n],
            on_stack: vec![false; n],
            stack: Vec::new(),
            next: 0,
            found: Vec::new(),
        };
        for start in 0..firm.roots {
            if state.order[start] == Self::UNSEEN {
                state.visit(start);
            }
        }
        state.found
    }

    /// start에서 닿는 끝점을 전부 방문한다 — 재귀 호출 하나가 프레임 하나다.
    fn visit(&mut self, start: usize) {
        self.enter(start);
        let mut frames = vec![(start, 0usize)];
        while let Some(frame) = frames.last_mut() {
            let v = frame.0;
            let Some(&w) = self.successors[v].get(frame.1) else {
                frames.pop();
                self.finish(v, frames.last().map(|parent| parent.0));
                continue;
            };
            frame.1 += 1;
            if self.order[w] == Self::UNSEEN {
                self.enter(w);
                frames.push((w, 0));
            } else if self.on_stack[w] {
                self.low[v] = self.low[v].min(self.order[w]);
            }
        }
    }

    /// 순번을 매기고 Tarjan 스택에 올린다.
    fn enter(&mut self, v: usize) {
        self.order[v] = self.next;
        self.low[v] = self.next;
        self.next += 1;
        self.stack.push(v);
        self.on_stack[v] = true;
    }

    /// v의 후속을 다 봤다 — 뿌리면 SCC를 꺼내고, 재귀판이 호출에서 돌아와 하던
    /// 대로 부모의 low에 v의 low를 반영한다.
    fn finish(&mut self, v: usize, parent: Option<usize>) {
        if self.low[v] == self.order[v] {
            let mut members = Vec::new();
            while let Some(w) = self.stack.pop() {
                self.on_stack[w] = false;
                members.push(w);
                if w == v {
                    break;
                }
            }
            self.found.push(members);
        }
        if let Some(p) = parent {
            self.low[p] = self.low[p].min(self.low[v]);
        }
    }
}

#[cfg(test)]
thread_local! {
    /// 순환 판정·증거 수집이 살핀 간선 수 — SCC마다 전체 간선을 다시 훑지 않는지
    /// 단위 테스트가 센다(테스트 스레드마다 따로).
    static CLASSIFIED_EDGES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// 테스트용 — 이 스레드에서 지금까지 순환 판정·증거 수집이 살핀 간선 수.
#[cfg(test)]
pub(crate) fn classified_edges() -> usize {
    CLASSIFIED_EDGES.with(std::cell::Cell::get)
}

/// 순환을 찾는다 — 확정 의존 간선만 따라간다(추정 간선은 제외하고 센다).
pub fn cycles(doc: &Document) -> CycleReport {
    let excluded = doc
        .edges
        .iter()
        .filter(|e| e.kind.is_dependency() && e.tentative)
        .count();
    let firm = FirmGraph::new(doc);
    let cyclic: Vec<Vec<usize>> = Tarjan::strongly_connected(&firm)
        .into_iter()
        // SCC는 비지 않는다 — 뿌리 자신이 들어 있다.
        .filter(|scc| scc.len() > 1 || firm.self_loop[scc[0]])
        .collect();
    let evidence = cycle_evidence(&firm, &cyclic);
    let mut result: Vec<Cycle> = cyclic
        .into_iter()
        .zip(evidence)
        .map(|(scc, mut evidence)| {
            // 안정 정렬 — 같은 (from,to)의 여러 종류는 doc.edges 순서를 지킨다.
            evidence.sort_by(|a, b| (&a.from, &a.to).cmp(&(&b.from, &b.to)));
            let mut members: Vec<String> =
                scc.into_iter().map(|m| firm.names[m].to_string()).collect();
            members.sort();
            Cycle { members, evidence }
        })
        .collect();
    result.sort_by(|a, b| a.members.cmp(&b.members));
    CycleReport {
        cycles: result,
        excluded_tentative: excluded,
    }
}

/// 순환 SCC마다 증거 간선(양 끝이 같은 SCC인 확정 의존 간선)을 doc.edges 순서로
/// 모은다. 확정 간선을 한 번만 돈다 — SCC마다 전체 간선을 다시 훑으면 비순환
/// 그래프(SCC 수 = 정점 수)에서 O(V·E)라 합성 심볼 그래프 8만 정점에서 1분을 넘겼다.
fn cycle_evidence(firm: &FirmGraph, cyclic: &[Vec<usize>]) -> Vec<Vec<CycleEdge>> {
    let mut owner: Vec<Option<usize>> = vec![None; firm.names.len()];
    for (k, scc) in cyclic.iter().enumerate() {
        for &m in scc {
            owner[m] = Some(k);
        }
    }
    let mut evidence: Vec<Vec<CycleEdge>> = cyclic.iter().map(|_| Vec::new()).collect();
    for &(from, to, e) in &firm.edges {
        #[cfg(test)]
        CLASSIFIED_EDGES.with(|c| c.set(c.get() + 1));
        if let Some(k) = owner[from].filter(|&k| owner[to] == Some(k)) {
            evidence[k].push(CycleEdge {
                from: e.from.clone(),
                to: e.to.clone(),
                kind: e.kind,
            });
        }
    }
    evidence
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
    /// 이 분석의 실측 한계 — 문서의 것과 별개로 판정 관련 한계. 문서 limitation 가운데서는 판정의
    /// 입력(보존 루트·들어오는 간선)을 지운 `--focus` 경계 문장만 옮겨 싣는다.
    pub limitations: Vec<String>,
}

/// 루트에서 도달 가능한 정점 집합 — 의존 간선만.
pub fn reachable(doc: &Document) -> BTreeSet<&str> {
    reach_from(&doc.outgoing(), &doc.roots)
}

/// `roots`에서 도달 인접 목록을 따라 닿는 정점 집합 — [`reachable`]과 색인을 쓰는 dead가 공유한다.
fn reach_from<'a>(out: &ReachMap<'a>, roots: &'a [String]) -> BTreeSet<&'a str> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut queue: VecDeque<&str> = VecDeque::new();
    for r in roots {
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
pub fn dead(doc: &Document) -> Result<DeadReport, String> {
    QueryIndex::new(doc).dead(&doc.roots)
}

/// `roots`를 보존 루트로 삼아 도달 인접 목록 `out` 위에서 [`dead`]를 판정한다 — 레벨
/// 확인은 호출자([`QueryIndex::dead`])가 먼저 한다.
fn dead_with(doc: &Document, out: &ReachMap<'_>, roots: &[String]) -> DeadReport {
    let reach = reach_from(out, roots);
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
    // `--focus`가 지운 보존 루트·들어오는 간선은 이 판정의 입력이라, 그 문서 limitation은 판정
    // 한계로도 싣는다 — JSON 보고서만 읽는 소비자가 거짓 unreachable을 완전한 답으로 받지 않게.
    limitations.extend(doc.focus_limitations().cloned());
    DeadReport {
        roots: roots.to_vec(),
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

    /// 예전 경로 탐색 — 부분 경로 상태마다 경로 전체를 복제하던 구현 그대로다. 아레나 탐색이 같은 BFS
    /// 순서·잘림·예산 소비·대기 상태 최대치를 내는지 대조하는 기준이다.
    fn paths_by_copying(
        index: &QueryIndex<'_>,
        from: &str,
        to: &str,
        max_paths: usize,
        budget: usize,
        peak_pending: &mut usize,
    ) -> PathsReport {
        let mut report = PathsReport {
            from: from.to_string(),
            to: to.to_string(),
            found: false,
            truncated: false,
            expanded: 0,
            paths: Vec::new(),
        };
        if !index.has_vertex(from) || !index.has_vertex(to) {
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
        let adj = index.path_adjacency();
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
        let mut pending_states = 1usize;
        let mut pending_paths = 0usize;
        *peak_pending = 1;
        while let Some(s) = queue.pop_front() {
            let cur = s.vertices.last().expect("state is never empty").clone();
            if cur == to {
                pending_paths -= 1;
            } else {
                pending_states -= 1;
            }
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
                // 확장할 수 없는 부분 경로는 복제 전에 버린다. 완성 경로는
                // 별도 슬롯을 사용해 예산이 찼어도 이미 찾은 증거를 보존한다.
                if *nbr != to && report.expanded.saturating_add(pending_states) >= budget {
                    report.truncated = true;
                    continue;
                }
                if *nbr == to && report.paths.len().saturating_add(pending_paths) > max_paths {
                    report.truncated = true;
                    continue;
                }
                let mut next = State {
                    vertices: s.vertices.clone(),
                    edges: s.edges.clone(),
                    tentative: s.tentative || *tent,
                };
                next.vertices.push((*nbr).to_string());
                next.edges.push(*kind);
                if *nbr == to {
                    pending_paths += 1;
                } else {
                    pending_states += 1;
                }
                queue.push_back(next);
                *peak_pending = (*peak_pending).max(queue.len());
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

    #[test]
    fn path_budget_bounds_queued_copies_and_keeps_completed_paths() {
        let mut graph = doc();
        graph.vertices = vec![v("start", Kind::Fn), v("zz_target", Kind::Fn)];
        graph.edges.clear();
        for i in 0..1000 {
            let id = format!("branch_{i:04}");
            graph.vertices.push(v(&id, Kind::Fn));
            graph
                .edges
                .push(Edge::new("start".into(), id, EdgeKind::Call));
        }
        graph.edges.push(Edge::new(
            "start".into(),
            "zz_target".into(),
            EdgeKind::Call,
        ));
        let mut peak = 0;
        let index = QueryIndex::new(&graph);
        let report = paths_bounded(&index, "start", "zz_target", 1, 2, &mut peak);
        assert!(report.found && report.truncated);
        assert_eq!(report.paths[0].vertices, ["start", "zz_target"]);
        assert_eq!(report.expanded, 2);
        assert!(peak <= 3, "unbounded pending states: {peak}");
    }

    /// 층마다 앞 층 전부와 이어진 DAG(`layers`층 × 폭 `width`, 정점 `c::lLL_W`)와 어디서도 닿지 않는
    /// `c::goal` — 출발점에서 단순 경로 상태가 층마다 폭 배로 불어난다.
    fn layered_doc(layers: usize, width: usize) -> Document {
        let id = |l: usize, w: usize| format!("c::l{l:02}_{w}");
        let mut d = doc();
        d.vertices = (0..layers * width)
            .map(|i| v(&id(i / width, i % width), Kind::Fn))
            .collect();
        d.vertices.push(v("c::goal", Kind::Fn));
        d.edges = (width..layers * width)
            .flat_map(|i| (0..width).map(move |a| (i, a)))
            .map(|(i, a)| {
                let layer = i / width;
                Edge::new(id(layer - 1, a), id(layer, i % width), EdgeKind::Call)
            })
            .collect();
        d
    }

    /// 경로 탐색은 부분 경로마다 경로 전체를 복제하지 않는다 — 상태는 (정점, 앞 상태, 간선 종류, 추정)
    /// 한 칸이고 경로 정점 문자열은 보고하는 경로에만 만든다. 예전 구현은 상태마다 경로 전체를 복제해
    /// 메모리가 예산 × 경로 길이였다(30층×폭 6 DAG, 예산 100만에 514MB).
    #[test]
    fn paths_materialize_only_the_reported_paths() {
        let deep = layered_doc(12, 4);
        let shallow = layered_doc(6, 3);
        for (d, to, max_paths, reported_paths) in [
            (&deep, "c::goal", 10, 0),
            (&shallow, "c::l05_2", 5, 5),
            (&shallow, "c::l05_2", 1_024, 81),
        ] {
            let before = path_vertex_copies();
            let r = paths(d, "c::l00_0", to, max_paths, 20_000);
            let copies = path_vertex_copies() - before;
            assert_eq!(r.paths.len(), reported_paths, "{to}");
            let reported: usize = r.paths.iter().map(|p| p.vertices.len()).sum();
            assert_eq!(
                copies, reported,
                "{to}: {copies} path vertices made for {reported} reported"
            );
        }
    }

    /// 아레나 탐색은 예전 복제 탐색과 같은 답을 낸다 — 무작위 작은 그래프(추정·비의존·dangling·자기
    /// 루프·중복 간선)와 작은 예산·경로 상한에서 보고 전체(경로 순서·간선 종류·tentative·found·
    /// truncated·expanded)와 대기 상태 최대치가 같다.
    #[test]
    fn paths_match_the_copying_search_on_random_graphs() {
        let kinds = [
            EdgeKind::Call,
            EdgeKind::References,
            EdgeKind::Contains,
            EdgeKind::Implements,
        ];
        let mut rng = Lcg(0xa7e4a);
        let (mut found, mut truncated, mut tentative) = (0, 0, 0);
        for _ in 0..4_000 {
            let count = 2 + rng.below(6);
            let vertex_names: Vec<String> = (0..count)
                .map(|i| format!("c::v{i}"))
                .chain(["x::gone".to_string()])
                .collect();
            let mut d = doc();
            d.vertices = vertex_names[..count]
                .iter()
                .map(|id| v(id, Kind::Fn))
                .collect();
            d.edges = (0..rng.below(24))
                .map(|_| {
                    let from = vertex_names[rng.below(vertex_names.len())].clone();
                    let to = vertex_names[rng.below(vertex_names.len())].clone();
                    let mut e = Edge::new(from, to, kinds[rng.below(kinds.len())]);
                    e.tentative = rng.below(4) == 0;
                    e
                })
                .collect();
            let from = &vertex_names[rng.below(count)];
            let to = &vertex_names[rng.below(vertex_names.len())];
            let (max_paths, budget) = (rng.below(6), rng.below(14));
            let index = QueryIndex::new(&d);
            let (mut peak, mut want_peak) = (0, 0);
            let got = paths_bounded(&index, from, to, max_paths, budget, &mut peak);
            let want = paths_by_copying(&index, from, to, max_paths, budget, &mut want_peak);
            assert_eq!(
                serde_json::to_value(&got).unwrap(),
                serde_json::to_value(&want).unwrap(),
                "{from} -> {to}, max {max_paths}, budget {budget}: {:?}",
                d.edges
            );
            assert_eq!(peak, want_peak, "{from} -> {to}: {:?}", d.edges);
            found += usize::from(got.found);
            truncated += usize::from(got.truncated);
            tentative += usize::from(got.paths.iter().any(|p| p.tentative));
        }
        assert!(
            found > 500 && truncated > 500 && tentative > 100,
            "found {found}, truncated {truncated}, tentative {tentative}"
        );
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
        let rep = dead(&d).unwrap();
        let ids: Vec<&str> = rep.findings.iter().map(|f| f.id.as_str()).collect();
        assert!(ids.contains(&"c::d"));
        assert_eq!(rep.roots, vec!["c::a"]);
        assert_eq!(rep.findings[0].state, "unreachable");
        assert_eq!(rep.findings[0].reason, "not reachable from retention roots");
    }

    /// `--focus`가 보존 루트·들어오는 간선을 지운 문서에서는 dead 판정 자체가 부분이다 — 보고서가 그
    /// limitation을 싣는다(호출자가 루트를 바꿔 묻는 MCP 경로도 같다). 문서의 다른 limitation은
    /// 지금처럼 보고서에 옮기지 않는다.
    #[test]
    fn dead_carries_the_focus_boundary_limitation_only() {
        let mut d = doc();
        d.limitations.push("harvest note".into());
        assert!(dead(&d).unwrap().limitations.is_empty());
        let focused = d.focus(&["c::b".to_string()]);
        let note: Vec<String> = focused.focus_limitations().cloned().collect();
        assert_eq!(note.len(), 1);
        assert!(note[0].starts_with("--focus dropped 1 retention roots and 2 dependency edges "));
        let rep = dead(&focused).unwrap();
        assert!(rep.roots.is_empty());
        assert_eq!(rep.findings[0].id, "c::b");
        assert_eq!(rep.limitations, note);
        let rooted = QueryIndex::new(&focused)
            .dead(&["c::b".to_string()])
            .unwrap();
        assert!(rooted.findings.is_empty());
        assert_eq!(rooted.limitations, note);
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

    /// 재귀 Tarjan은 DFS 경로 길이만큼 호출 스택을 써서 수만 정점 고리·사슬에서
    /// 스레드 스택을 넘기고 프로세스를 abort시켰다(종료 코드 134, MCP 서버 소멸).
    /// 재귀판이 수천 단계도 못 버티는 작은 스택에서 고리 하나·사슬 순환 없음을 낸다.
    #[test]
    fn cycles_survive_deep_paths_on_a_small_stack() {
        let n = 5_000;
        let ids: Vec<String> = (0..n).map(|i| format!("c::f{i:05}")).collect();
        let mut ring = doc();
        ring.vertices = ids.iter().map(|id| v(id, Kind::Fn)).collect();
        ring.edges = (0..n)
            .map(|i| Edge::new(ids[i].clone(), ids[(i + 1) % n].clone(), EdgeKind::Call))
            .collect();
        let mut chain = ring.clone();
        // 마지막 간선(끝 → 처음)을 빼면 같은 깊이의 비순환 사슬이다.
        chain.edges.pop();
        let (ring_rep, chain_rep) = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(move || (cycles(&ring), cycles(&chain)))
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(ring_rep.cycles.len(), 1);
        assert_eq!(ring_rep.cycles[0].members.len(), n);
        assert_eq!(ring_rep.cycles[0].evidence.len(), n);
        assert!(chain_rep.cycles.is_empty());
    }

    /// 순환 판정이 SCC마다 전체 간선을 다시 훑으면 비순환 그래프(SCC 수 = 정점 수)에서
    /// O(V·E)다 — 합성 심볼 그래프 8만 정점에서 1분을 넘겼다. 증거 분배가 확정 간선을 정확히
    /// 한 번씩 살피는지 센다 — 상한이 아니라 정확한 횟수라서, SCC마다 다시 훑어도(넘침) 분배를
    /// 빼고 옛 SCC별 전체 탐색으로 돌아가도(0) 실패한다.
    #[test]
    fn cycle_classification_examines_each_edge_once() {
        let n = 1_000;
        let ids: Vec<String> = (0..n).map(|i| format!("c::f{i:04}")).collect();
        let mut d = doc();
        d.vertices = ids.iter().map(|id| v(id, Kind::Fn)).collect();
        d.edges = (0..n)
            .flat_map(|i| (i + 1..n.min(i + 4)).map(move |j| (i, j)))
            .map(|(i, j)| Edge::new(ids[i].clone(), ids[j].clone(), EdgeKind::Call))
            .collect();
        // 여러 정점 순환 하나와 자기 루프 하나 — 판정이 실제로 일을 하게 한다.
        d.edges
            .push(Edge::new(ids[10].clone(), ids[0].clone(), EdgeKind::Call));
        d.edges.push(Edge::new(
            ids[500].clone(),
            ids[500].clone(),
            EdgeKind::Uses,
        ));
        let before = CLASSIFIED_EDGES.with(std::cell::Cell::get);
        let rep = cycles(&d);
        let visits = CLASSIFIED_EDGES.with(std::cell::Cell::get) - before;
        assert_eq!(rep.cycles.len(), 2);
        assert_eq!(rep.cycles[0].members.len(), 11);
        assert_eq!(rep.cycles[1].members, ["c::f0500"]);
        // 이 문서의 간선은 모두 확정 의존 간선이다.
        assert_eq!(
            visits,
            d.edges.len(),
            "{visits} edge visits for {} edges",
            d.edges.len()
        );
    }

    /// 결정적 의사 난수(LCG) — 무작위 비교 테스트가 실행마다 같은 그래프를 본다.
    struct Lcg(u64);

    impl Lcg {
        fn below(&mut self, n: usize) -> usize {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 33) as usize % n
        }
    }

    /// (멤버, 증거 (from, to, kind)) 목록과 제외한 추정 간선 수 — 비교용 평탄 형태.
    type FlatCycles = (Vec<(Vec<String>, Vec<(String, String, EdgeKind)>)>, usize);

    fn flatten(rep: &CycleReport) -> FlatCycles {
        let found = rep
            .cycles
            .iter()
            .map(|c| {
                let evidence = c
                    .evidence
                    .iter()
                    .map(|e| (e.from.clone(), e.to.clone(), e.kind))
                    .collect();
                (c.members.clone(), evidence)
            })
            .collect();
        (found, rep.excluded_tentative)
    }

    /// 정의 그대로의 순환 판정 — Tarjan과 독립인 오라클. 확정 의존 간선의 도달 집합이
    /// 서로를 품는 끝점끼리 SCC다. 후보는 정점에서 닿는 끝점뿐이고(dangling 끝점 포함 —
    /// 정점에서 안 닿는 dangling 순환은 보고하지 않는다), 증거는 doc.edges 순서를
    /// (from,to)로 안정 정렬한다. 재작성 전 구현의 출력 계약이 이것이다.
    fn cycles_by_reachability(d: &Document) -> FlatCycles {
        let firm: Vec<&Edge> = d
            .edges
            .iter()
            .filter(|e| e.kind.is_dependency() && !e.tentative)
            .collect();
        let reach = |start: &str| -> BTreeSet<String> {
            let mut seen = BTreeSet::from([start.to_string()]);
            let mut queue = vec![start.to_string()];
            while let Some(cur) = queue.pop() {
                for e in firm.iter().filter(|e| e.from == cur) {
                    if seen.insert(e.to.clone()) {
                        queue.push(e.to.clone());
                    }
                }
            }
            seen
        };
        let mut candidates = BTreeSet::new();
        for id in d.vertex_ids() {
            candidates.extend(reach(id));
        }
        let mut placed: BTreeSet<String> = BTreeSet::new();
        let mut found = Vec::new();
        for u in &candidates {
            if placed.contains(u) {
                continue;
            }
            let members: Vec<String> = reach(u)
                .into_iter()
                .filter(|w| reach(w).contains(u))
                .collect();
            placed.extend(members.iter().cloned());
            let looped = firm.iter().any(|e| e.from == *u && e.to == *u);
            if members.len() < 2 && !looped {
                continue;
            }
            let mut evidence: Vec<(String, String, EdgeKind)> = firm
                .iter()
                .filter(|e| members.contains(&e.from) && members.contains(&e.to))
                .map(|e| (e.from.clone(), e.to.clone(), e.kind))
                .collect();
            evidence.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
            found.push((members, evidence));
        }
        found.sort();
        let excluded = d
            .edges
            .iter()
            .filter(|e| e.kind.is_dependency() && e.tentative)
            .count();
        (found, excluded)
    }

    /// 정렬되지 않은 저장 문서(`--graph`)처럼 간선 순서·중복·자기 루프·추정 간선만의
    /// 순환·contains·정점이 아닌 dangling 끝점·같은 ID의 cfg 변형을 섞은 무작위
    /// 그래프에서 cycles가 오라클과 같은 보고서를 낸다.
    #[test]
    fn cycles_match_reachability_oracle_on_random_graphs() {
        let kinds = [
            EdgeKind::Call,
            EdgeKind::References,
            EdgeKind::Uses,
            EdgeKind::Contains,
            EdgeKind::Implements,
            EdgeKind::Depends,
            EdgeKind::Signature,
        ];
        let mut rng = Lcg(0x5eed);
        // 비교가 공허하지 않은지 — 여러 정점 순환, 자기 루프 단독, dangling 멤버를 실제로 봤다.
        let (mut multi, mut looped, mut dangling) = (0, 0, 0);
        for _ in 0..3_000 {
            let vertex_count = 1 + rng.below(6);
            let names: Vec<String> = (0..vertex_count)
                .map(|i| format!("c::v{i}"))
                .chain(["x::g0".to_string(), "x::g1".to_string()])
                .collect();
            let mut d = doc();
            d.vertices = names[..vertex_count]
                .iter()
                .rev()
                .map(|id| v(id, Kind::Fn))
                .collect();
            if rng.below(4) == 0 {
                d.vertices.push(v(&names[0], Kind::Fn));
            }
            d.edges = (0..rng.below(14))
                .map(|_| {
                    let from = names[rng.below(names.len())].clone();
                    let to = names[rng.below(names.len())].clone();
                    let mut e = Edge::new(from, to, kinds[rng.below(kinds.len())]);
                    e.tentative = rng.below(4) == 0;
                    e
                })
                .collect();
            let rep = cycles(&d);
            assert_eq!(
                flatten(&rep),
                cycles_by_reachability(&d),
                "edges: {:?}",
                d.edges
            );
            for c in &rep.cycles {
                multi += usize::from(c.members.len() > 1);
                looped += usize::from(c.members.len() == 1);
                dangling += usize::from(c.members.iter().any(|m| m.starts_with("x::")));
            }
        }
        assert!(multi > 100 && looped > 100 && dangling > 100);
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
    fn impact_does_not_cross_shared_dependencies() {
        let mut d = doc();
        // a -> b <- d: a의 변경은 같은 b를 쓰는 d에 영향을 주지 않는다.
        d.edges
            .push(Edge::new("c::d".into(), "c::b".into(), EdgeKind::Call));
        let r = impact(&d, "c::a", usize::MAX, 100);
        assert_eq!(
            r.impacted.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(),
            vec!["c::e"]
        );
        assert_eq!(r.impacted[0].depth, 1);
        assert!(!r.truncated);
    }

    #[test]
    fn impact_budget_is_not_spent_on_dependencies() {
        let r = impact(&doc(), "c::a", usize::MAX, 1);
        assert_eq!(r.impacted.len(), 1);
        assert_eq!(r.impacted[0].id, "c::e");
        assert!(!r.truncated);
    }

    #[test]
    fn neighbors_obey_zero_and_exact_limits() {
        let d = doc();
        for max in 0..=4 {
            let q = query(&d, "c::a", 1, max);
            assert_eq!(q.neighbors.len(), max.min(2));
            assert_eq!(q.truncated, max < 2);
            let r = impact(&d, "c::c", usize::MAX, max);
            assert_eq!(r.impacted.len(), max.min(3));
            assert_eq!(r.truncated, max < 3);
        }
        assert!(!query(&d, "c::d", 10, 0).truncated);
        assert!(!query(&d, "c::a", 0, 0).truncated);
    }

    #[test]
    fn neighbors_do_not_repeat_cycle_members_or_the_root() {
        let d = doc();
        let q = query(&d, "c::x", usize::MAX, 2);
        assert_eq!(q.neighbors.len(), 2);
        assert!(q.neighbors.iter().all(|n| n.id == "c::y" && n.depth == 1));
        assert!(!q.truncated);
        let r = impact(&d, "c::x", usize::MAX, 1);
        assert_eq!(r.impacted.len(), 1);
        assert_eq!(r.impacted[0].id, "c::y");
        assert!(!r.truncated);
    }

    #[test]
    fn neighbors_keep_edge_kinds_in_their_actual_direction() {
        let mut d = doc();
        d.edges.push(Edge::new(
            "c::b".into(),
            "c::a".into(),
            EdgeKind::References,
        ));
        d.edges
            .push(Edge::new("c::a".into(), "c::b".into(), EdgeKind::Contains));
        let q = query(&d, "c::a", 1, 10);
        let outgoing = q
            .neighbors
            .iter()
            .find(|n| n.id == "c::b" && n.direction == "out")
            .unwrap();
        assert_eq!(outgoing.edges, vec![EdgeKind::Call]);
        let incoming = q
            .neighbors
            .iter()
            .find(|n| n.id == "c::b" && n.direction == "in")
            .unwrap();
        assert_eq!(incoming.edges, vec![EdgeKind::References]);
    }

    #[test]
    fn neighbors_merge_shortest_layer_evidence_and_ignore_dangling_edges() {
        let mut d = doc();
        d.edges.extend([
            Edge::new("c::d".into(), "c::c".into(), EdgeKind::Call),
            Edge::new("c::a".into(), "c::d".into(), EdgeKind::References),
            Edge::new("c::ghost".into(), "c::c".into(), EdgeKind::Call),
        ]);
        let r = impact(&d, "c::c", 2, 10);
        assert_eq!(
            r.impacted.iter().map(|n| n.id.as_str()).collect::<Vec<_>>(),
            vec!["c::b", "c::d", "c::a"]
        );
        assert_eq!(
            r.impacted[2].edges,
            vec![EdgeKind::Call, EdgeKind::References]
        );
        assert_eq!(r.impacted[2].depth, 2);
        assert!(!impact(&d, "c::ghost", 10, 10).found);
        assert!(query(&d, "c::ghost", 10, 10).neighbors.is_empty());
        let expected = serde_json::to_string(&r).unwrap();
        d.edges.reverse();
        d.vertices.reverse();
        assert_eq!(
            serde_json::to_string(&impact(&d, "c::c", 2, 10)).unwrap(),
            expected
        );
    }

    #[test]
    fn impact_matches_reverse_reachability_for_every_three_vertex_graph() {
        // 3개 정점의 가능한 유향 그래프 512개를 독립 거리 행렬과 대조한다.
        // 자기 루프·순환·재수렴·고립 정점이 방향/상한 수정에서 빠지지 않게 한다.
        // 그래프마다 색인 하나를 모든 (root, depth, max) 질의가 함께 쓴다 — MCP 세션처럼
        // 재사용해도 앞 질의의 상태가 다음 질의에 새지 않아야 한다.
        let names = ["c::a", "c::b", "c::c"];
        for mask in 0..(1_u16 << 9) {
            let mut d = doc();
            d.vertices.retain(|v| names.contains(&v.id.as_str()));
            d.edges.clear();
            let mut distances = [[4_usize; 3]; 3];
            for from in 0..3 {
                distances[from][from] = 0;
                for to in 0..3 {
                    if mask & (1 << (from * 3 + to)) != 0 {
                        d.edges.push(Edge::new(
                            names[from].into(),
                            names[to].into(),
                            EdgeKind::Call,
                        ));
                        if from != to {
                            distances[from][to] = 1;
                        }
                    }
                }
            }
            for via in 0..3 {
                for from in 0..3 {
                    for to in 0..3 {
                        distances[from][to] =
                            distances[from][to].min(distances[from][via] + distances[via][to]);
                    }
                }
            }
            let shared = QueryIndex::new(&d);
            for (root, name) in names.iter().enumerate() {
                for depth in 0..=3 {
                    let mut expected: Vec<_> = (0..3)
                        .filter(|&i| i != root && distances[i][root] <= depth)
                        .map(|i| (distances[i][root], names[i]))
                        .collect();
                    expected.sort();
                    for max in 0..=3 {
                        for r in [
                            impact(&d, name, depth, max),
                            shared.impact(name, depth, max),
                        ] {
                            assert_eq!(
                                r.truncated,
                                expected.len() > max,
                                "mask {mask}, root {name}, depth {depth}, max {max}"
                            );
                            assert_eq!(
                                r.impacted
                                    .iter()
                                    .map(|n| (n.depth, n.id.as_str()))
                                    .collect::<Vec<_>>(),
                                expected.iter().copied().take(max).collect::<Vec<_>>()
                            );
                        }
                        // 양방향 이웃도 공유 색인과 새 색인의 답이 같다.
                        assert_eq!(
                            serde_json::to_string(&shared.query(name, depth, max)).unwrap(),
                            serde_json::to_string(&query(&d, name, depth, max)).unwrap(),
                            "mask {mask}, root {name}, depth {depth}, max {max}"
                        );
                    }
                }
            }
        }
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
