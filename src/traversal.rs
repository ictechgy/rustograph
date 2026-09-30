//! 여러 root에서 한 번에 그래프를 정방향(dependencies) 또는 역방향(dependents)으로 훑는다.
//!
//! isthmus `language-traversal` v1(`docs/LANGUAGE-TRAVERSAL.md`)의 도달 의미를 그대로 계산한다 —
//! 계열 생산자(pythograph `graph/traversal.py`, tsograph `src/graph/traversal.ts`)와 같은 알고리즘이다.
//!
//! - `reached`는 자기 자신이 아닌 root에서 간선 1개 이상으로(깊이 상한 안에서) 닿은 정점 전부다.
//!   root이기도 한 정점도 싣되 그 `roots`에는 그에 닿는 다른 root만 넣는다. 자기 자신에게서만 닿는
//!   root는 싣지 않는다.
//! - `depth`는 그 root들 중 가장 가까운 것까지의 거리, `via`는 가장 가까운 root(같은 거리면 작은
//!   인덱스)에서 depth-1 거리에 있는 선행 정점 중 id가 가장 작은 것이다(UTF-16 코드 단위 순서).
//! - 구현은 모든 root를 한 번에 출발시키는 단계 동기 너비 우선 패스다. 정점이 자기 자신이 아닌 더 작은
//!   인덱스 root를 65개 이상 가졌으면 더 큰 인덱스 root는 그 정점에서 전파를 멈춘다 — 그 정점을 지나는
//!   경로는 작은 root들도 같거나 짧은 거리로 지나므로 출력(작은 인덱스 64개·depth·via)을 바꿀 수 없다.
//! - `evidence`는 root별 하한이다: 정점에 닿는 모든 root 각각의 가장 강한 등급 중 가장 약한 것.
//!   추정 간선(`tentative` — 이름 팬아웃·`dyn`/제네릭 트레이트 impl 후보)은 `candidate`, 나머지는
//!   `direct`다. 비트 집합이 메모리 상한을 넘으면 약하게 적을 수는 있어도 부풀리지 않는 근사로 바꾼다.
//!
//! 이 모듈은 문서(graph)만 소비한다 — 수확·입출력을 모른다.

pub mod sha256;

use crate::graph::Document;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// 정점 하나가 싣는 root 인덱스의 상한이다(계약의 `rootsTruncated`).
pub const MAX_ROOTS_PER_NODE: usize = 64;
/// 계약이 허용하는 최대 깊이다.
pub const MAX_DEPTH: usize = 128;
/// 계약이 허용하는 최대 도달 정점 수다.
pub const MAX_REACHED: usize = 100_000;
/// 계약이 허용하는 최대 root 수다.
pub const MAX_ROOTS: usize = 10_000;
/// 전파를 멈추게 하는 더 작은 root 수 — 정점이 자기 인덱스를 빼도 64개가 남도록 하나 더 둔다.
const DOMINATING_ROOTS: usize = MAX_ROOTS_PER_NODE + 1;
/// 정확한 등급 비교(root당 비트 하나)가 쓸 수 있는 메모리 상한(바이트)이다.
pub const EVIDENCE_MEMORY_BYTES: usize = 64 * 1024 * 1024;

/// 순회 방향 — 계약 문자열과 1:1이다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// root가 기대는 쪽(호출·참조 대상).
    Dependencies,
    /// root에 기대는 쪽(호출자·참조자).
    Dependents,
}

impl Direction {
    /// 계약의 `direction` 문자열이다.
    pub fn as_str(self) -> &'static str {
        match self {
            Direction::Dependencies => "dependencies",
            Direction::Dependents => "dependents",
        }
    }
}

/// 근거 등급 — 간선 집합이 포개진다(`direct ⊂ candidate`). bound는 만들지 않는다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Evidence {
    /// 구문·타입이 해석한 간선만으로 닿는다.
    Direct,
    /// 가능성만 있는 추정 간선을 하나 이상 거친다.
    Candidate,
}

impl Evidence {
    /// 계약의 `evidence` 문자열이다.
    pub fn as_str(self) -> &'static str {
        match self {
            Evidence::Direct => "direct",
            Evidence::Candidate => "candidate",
        }
    }
}

/// 순회 요청. root는 모두 문서 정점이고 서로 다르다(호출자가 거른다).
pub struct Request<'a> {
    /// root id — 순서가 `roots` 인덱스의 뜻이다.
    pub roots: &'a [String],
    pub direction: Direction,
    /// 최대 깊이(1~128).
    pub max_depth: usize,
    /// 최대 도달 정점 수 — 넘으면 (depth, id) 순 앞쪽만 남기고 `max-reached`로 잘린다.
    pub max_reached: usize,
    /// 등급 비교 메모리 상한(테스트 주입용).
    pub evidence_memory_bytes: usize,
}

/// 도달 정점 하나.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reached {
    pub id: String,
    /// 최단 경로의 직전 정점(root id 또는 다른 도달 정점 id).
    pub via: String,
    /// 가장 가까운 root까지의 간선 수.
    pub depth: usize,
    /// 닿는 root 인덱스(오름차순, 최대 64개).
    pub roots: Vec<usize>,
    /// via와 이 정점 사이 간선 종류(원래 방향 기준, 정렬).
    pub relationships: Vec<String>,
    /// root별 하한 근거 등급.
    pub evidence: Evidence,
}

/// 순회 결과.
#[derive(Debug, Clone)]
pub struct Outcome {
    /// (depth, UTF-16 id) 순 도달 정점.
    pub reached: Vec<Reached>,
    /// 정렬한 잘림 이유(`depth`·`max-reached`).
    pub truncation_reasons: Vec<&'static str>,
    /// 정점당 root 64개 상한이 적용됐는지.
    pub roots_truncated: bool,
    /// 근거 등급을 보수적으로 근사했는지.
    pub evidence_approximated: bool,
}

/// UTF-16 코드 단위 순서 비교다(계약의 정렬 규칙, locale 무관).
/// Rust 문자열 비교는 UTF-8 바이트(=코드 포인트) 순이라 U+E000~FFFF와 보충 평면 문자의
/// 순서가 계약과 뒤집힌다.
pub fn utf16_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// 정점을 정수로 바꾼 인접 구조 — 정점 번호는 UTF-16 순서라 번호 비교가 곧 id 비교다.
struct Graph {
    ids: Vec<String>,
    index: HashMap<String, usize>,
    /// 순회 방향의 이웃(중복 없음). `firm`은 확정 간선이 하나라도 있는 쌍이다.
    full: Vec<Vec<usize>>,
    firm: Vec<Vec<usize>>,
    /// 순회 방향의 선행 정점(번호 오름차순 = UTF-16 순).
    predecessors: Vec<Vec<usize>>,
    /// (출발, 도착) → 원래 방향 간선 종류(정렬).
    kinds: HashMap<(usize, usize), BTreeSet<String>>,
    /// 추정 간선만 있는 쌍의 순회 방향 출발점 — 등급 비교 대상 root를 고르는 재료.
    weak_tails: Vec<usize>,
    /// 추정 간선만 있는 쌍(출발, 도착).
    weak_pairs: Vec<(usize, usize)>,
}

impl Graph {
    /// 문서의 의존 간선으로 방향에 맞춘 인접 구조를 만든다. 양 끝이 정점인 간선만 쓴다 —
    /// 저장 문서의 dangling 간선이 없는 정점을 지어내지 않게 한다.
    fn new(doc: &Document, direction: Direction) -> Graph {
        let mut ids: Vec<String> = doc.vertex_ids().into_iter().map(str::to_string).collect();
        ids.sort_by(|a, b| utf16_cmp(a, b));
        let index: HashMap<String, usize> = ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.clone(), i))
            .collect();
        let n = ids.len();
        // 쌍별 (확정 존재, 종류) — 같은 쌍의 여러 간선은 하나로 합친다.
        let mut pairs: BTreeMap<(usize, usize), (bool, BTreeSet<String>)> = BTreeMap::new();
        for e in doc.edges.iter().filter(|e| e.kind.is_dependency()) {
            let (Some(&from), Some(&to)) = (index.get(&e.from), index.get(&e.to)) else {
                continue;
            };
            let key = match direction {
                Direction::Dependencies => (from, to),
                Direction::Dependents => (to, from),
            };
            let entry = pairs.entry(key).or_insert((false, BTreeSet::new()));
            entry.0 |= !e.tentative;
            entry.1.insert(edge_kind_name(e.kind));
        }
        let mut g = Graph {
            ids,
            index,
            full: vec![Vec::new(); n],
            firm: vec![Vec::new(); n],
            predecessors: vec![Vec::new(); n],
            kinds: HashMap::new(),
            weak_tails: Vec::new(),
            weak_pairs: Vec::new(),
        };
        for ((a, b), (firm, kinds)) in pairs {
            g.full[a].push(b);
            g.predecessors[b].push(a);
            if firm {
                g.firm[a].push(b);
            } else {
                g.weak_tails.push(a);
                g.weak_pairs.push((a, b));
            }
            g.kinds.insert((a, b), kinds);
        }
        for p in &mut g.predecessors {
            p.sort_unstable();
        }
        g.weak_tails.sort_unstable();
        g.weak_tails.dedup();
        g
    }
}

/// 간선 종류의 계약 문자열(serde 이름과 같다).
fn edge_kind_name(kind: crate::graph::EdgeKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// 정점별 (root 인덱스 → 처음 닿은 단계)다.
type Levels = Vec<HashMap<usize, usize>>;

/// 순회한다.
pub fn traverse(doc: &Document, req: &Request) -> Outcome {
    let g = Graph::new(doc, req.direction);
    let roots: Vec<usize> = req
        .roots
        .iter()
        .filter_map(|r| g.index.get(r).copied())
        .collect();
    debug_assert_eq!(roots.len(), req.roots.len(), "roots must be vertices");
    let (levels, depth_cut, pruned) = propagate(&g, &roots, req.max_depth);
    let rows = reached_rows(&g, &levels, &roots);
    let over = rows.len() > req.max_reached;
    let kept: Vec<Row> = rows.into_iter().take(req.max_reached).collect();
    let mut reasons = Vec::new();
    if depth_cut {
        reasons.push("depth");
    }
    if over {
        reasons.push("max-reached");
    }
    let roots_truncated = pruned || kept.iter().any(|r| r.roots.len() > MAX_ROOTS_PER_NODE);
    let (evidence, approximated) = evidence_tiers(&g, &roots, req, &levels);
    let reached = kept
        .into_iter()
        .map(|row| Reached {
            id: g.ids[row.node].clone(),
            via: g.ids[row.via].clone(),
            depth: row.depth,
            relationships: g
                .kinds
                .get(&(row.via, row.node))
                .map(|k| k.iter().take(32).cloned().collect())
                .unwrap_or_default(),
            evidence: evidence(row.node),
            roots: row.roots.into_iter().take(MAX_ROOTS_PER_NODE).collect(),
        })
        .collect();
    Outcome {
        reached,
        truncation_reasons: reasons,
        roots_truncated,
        evidence_approximated: approximated,
    }
}

/// 모든 root에서 단계 동기로 전파한다. (단계 기록, 깊이 잘림, 전파 중단 여부)를 돌려준다.
fn propagate(g: &Graph, roots: &[usize], max_depth: usize) -> (Levels, bool, bool) {
    let mut levels: Levels = vec![HashMap::new(); g.ids.len()];
    let owners: HashMap<usize, usize> = roots.iter().enumerate().map(|(i, &n)| (n, i)).collect();
    let mut frontier: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (i, &n) in roots.iter().enumerate() {
        levels[n].insert(i, 0);
        frontier.insert(n, vec![i]);
    }
    let mut pruned = false;
    for level in 0..max_depth {
        if frontier.is_empty() {
            break;
        }
        let mut next: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (&node, held_roots) in &frontier {
            for &nb in &g.full[node] {
                pruned |= spread(
                    &mut levels,
                    owners.get(&nb).copied(),
                    nb,
                    held_roots,
                    level + 1,
                    &mut next,
                );
            }
        }
        frontier = next;
    }
    let cut = frontier.iter().any(|(&node, held_roots)| {
        g.full[node]
            .iter()
            .any(|&nb| held_roots.iter().any(|r| !levels[nb].contains_key(r)))
    });
    (levels, cut, pruned)
}

/// 한 간선으로 root 인덱스들을 이웃에 전파한다. 전파를 멈춘 쌍이 있었으면 true다.
fn spread(
    levels: &mut Levels,
    owner: Option<usize>,
    nb: usize,
    roots: &[usize],
    level: usize,
    next: &mut BTreeMap<usize, Vec<usize>>,
) -> bool {
    let mut pruned = false;
    for &r in roots {
        if levels[nb].contains_key(&r) {
            continue;
        }
        if dominated(&levels[nb], owner, r) {
            pruned = true;
            continue;
        }
        levels[nb].insert(r, level);
        next.entry(nb).or_default().push(r);
    }
    pruned
}

/// 정점이 자기 자신이 아닌 root 중 이 root보다 작은 인덱스를 이미 65개 이상 가졌는지 본다.
fn dominated(held: &HashMap<usize, usize>, owner: Option<usize>, root: usize) -> bool {
    if held.len() - usize::from(owner.is_some_and(|o| held.contains_key(&o))) < DOMINATING_ROOTS {
        return false;
    }
    held.keys()
        .filter(|&&i| Some(i) != owner && i < root)
        .count()
        >= DOMINATING_ROOTS
}

/// 출력 전 도달 행.
struct Row {
    node: usize,
    via: usize,
    depth: usize,
    roots: Vec<usize>,
}

/// 단계 기록에서 도달 행을 만든다. (depth, UTF-16 id) 순이다.
fn reached_rows(g: &Graph, levels: &Levels, roots: &[usize]) -> Vec<Row> {
    let owners: HashMap<usize, usize> = roots.iter().enumerate().map(|(i, &n)| (n, i)).collect();
    let mut rows = Vec::new();
    for (node, held) in levels.iter().enumerate() {
        let owner = owners.get(&node).copied();
        let mut rs: Vec<usize> = held.keys().copied().filter(|&i| Some(i) != owner).collect();
        if rs.is_empty() {
            continue;
        }
        rs.sort_unstable();
        let depth = rs.iter().map(|i| held[i]).min().unwrap_or(0);
        let nearest = *rs.iter().find(|i| held[i] == depth).unwrap_or(&rs[0]);
        // 가장 가까운 root에서 depth-1 거리의 선행 정점은 반드시 있다 — 이 정점이 그 단계에
        // 전파받았다는 것이 곧 그런 선행 정점의 존재다. 번호 순이 UTF-16 순이다.
        let Some(&via) = g.predecessors[node]
            .iter()
            .find(|&&p| levels[p].get(&nearest) == Some(&(depth - 1)))
        else {
            continue;
        };
        rows.push(Row {
            node,
            via,
            depth,
            roots: rs,
        });
    }
    rows.sort_by_key(|r| (r.depth, r.node));
    rows
}

/// 정점 번호 → 등급 함수와 근사 여부.
type EvidenceFn = Box<dyn Fn(usize) -> Evidence>;

/// 정점의 root별 하한 근거 등급을 구하는 함수를 만든다.
fn evidence_tiers(
    g: &Graph,
    roots: &[usize],
    req: &Request,
    levels: &Levels,
) -> (EvidenceFn, bool) {
    let compared = weak_touching_roots(g, roots, req.max_depth);
    if compared.is_empty() {
        return (Box::new(|_| Evidence::Direct), false);
    }
    let words = compared.len().div_ceil(64);
    let estimate = g.ids.len().saturating_mul(words * 8 + 32).saturating_mul(3);
    if estimate > req.evidence_memory_bytes {
        let marks = approximate_marks(g, levels);
        return (
            Box::new(move |n| {
                if marks[n] {
                    Evidence::Candidate
                } else {
                    Evidence::Direct
                }
            }),
            true,
        );
    }
    let full = reach_bits(&g.full, &compared, words, req.max_depth);
    let firm = reach_bits(&g.firm, &compared, words, req.max_depth);
    (
        Box::new(move |n| {
            if full[n] == firm[n] {
                Evidence::Direct
            } else {
                Evidence::Candidate
            }
        }),
        false,
    )
}

/// 깊이 상한 안에서 추정 간선의 출발점에 닿을 수 있는 root다(입력 순서).
/// 닿지 못하는 root는 모든 등급에서 같게 닿으므로 비교에서 뺀다.
fn weak_touching_roots(g: &Graph, roots: &[usize], max_depth: usize) -> Vec<usize> {
    let mut seen = vec![false; g.ids.len()];
    let mut frontier: Vec<usize> = g.weak_tails.clone();
    for &t in &frontier {
        seen[t] = true;
    }
    for _ in 0..max_depth.saturating_sub(1) {
        if frontier.is_empty() {
            break;
        }
        let mut next = Vec::new();
        for &n in &frontier {
            for &p in &g.predecessors[n] {
                if !seen[p] {
                    seen[p] = true;
                    next.push(p);
                }
            }
        }
        frontier = next;
    }
    roots.iter().copied().filter(|&r| seen[r]).collect()
}

/// root마다 비트 하나를 두고 단계 동기로 전파해, 깊이 상한 안에서 각 정점에 닿는 root 비트 집합을 구한다.
fn reach_bits(
    adj: &[Vec<usize>],
    roots: &[usize],
    words: usize,
    max_depth: usize,
) -> Vec<Vec<u64>> {
    let mut seen = vec![vec![0u64; words]; adj.len()];
    let mut frontier: BTreeMap<usize, Vec<u64>> = BTreeMap::new();
    for (pos, &r) in roots.iter().enumerate() {
        seen[r][pos / 64] |= 1 << (pos % 64);
        frontier.entry(r).or_insert_with(|| vec![0; words])[pos / 64] |= 1 << (pos % 64);
    }
    for _ in 0..max_depth {
        if frontier.is_empty() {
            break;
        }
        let mut next: BTreeMap<usize, Vec<u64>> = BTreeMap::new();
        for (&n, bits) in &frontier {
            for &nb in &adj[n] {
                let fresh: Vec<u64> = bits.iter().zip(&seen[nb]).map(|(b, s)| b & !s).collect();
                if fresh.iter().any(|w| *w != 0) {
                    for (s, f) in seen[nb].iter_mut().zip(&fresh) {
                        *s |= f;
                    }
                    let slot = next.entry(nb).or_insert_with(|| vec![0; words]);
                    for (s, f) in slot.iter_mut().zip(&fresh) {
                        *s |= f;
                    }
                }
            }
        }
        frontier = next;
    }
    seen
}

/// 메모리 상한을 넘을 때의 보수적 등급 표시다. root에서 닿은 출발점을 가진 추정 간선의 도착점에서
/// 깊이 제한 없이 닿는 정점을 candidate로 표시한다. 표시되지 않은 정점은 모든 root에서 추정 간선
/// 없이 닿으므로 정확히 direct다 — 약하게 적을 수는 있어도 부풀리지 않는다.
fn approximate_marks(g: &Graph, levels: &Levels) -> Vec<bool> {
    let mut marks = vec![false; g.ids.len()];
    let mut stack: Vec<usize> = g
        .weak_pairs
        .iter()
        .filter(|(tail, _)| !levels[*tail].is_empty())
        .map(|&(_, head)| head)
        .collect();
    while let Some(n) = stack.pop() {
        if marks[n] {
            continue;
        }
        marks[n] = true;
        stack.extend(g.full[n].iter().copied().filter(|&m| !marks[m]));
    }
    marks
}

#[cfg(test)]
mod tests;
