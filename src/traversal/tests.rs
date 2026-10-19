//! 다중 root 단일 패스를 root별 너비 우선 탐색(오라클)과 무작위 그래프로 대조한다.

use super::*;
use crate::graph::{self, Edge, EdgeKind, Kind, Level, Vertex};
use std::collections::{BTreeSet, VecDeque};

/// 의존성 없는 결정적 난수(xorshift64*).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn vertex(id: &str) -> Vertex {
    Vertex {
        id: id.to_string(),
        kind: Kind::Fn,
        krate: "c".into(),
        module: "c".into(),
        position: None,
        exported: false,
        generated: false,
        cfg: None,
        unsafe_: false,
    }
}

/// 정점 이름 — UTF-16 순서가 바이트 순서와 다른 문자(U+E000대·보충 평면)를 섞는다.
fn name(i: usize) -> String {
    match i % 4 {
        0 => format!("c::n{i}"),
        1 => format!("c::\u{e000}{i}"),
        2 => format!("c::\u{10400}{i}"),
        _ => format!("c::N{i}"),
    }
}

fn random_doc(rng: &mut Rng, n: usize, m: usize) -> Document {
    let vertices = (0..n).map(|i| vertex(&name(i))).collect();
    let mut edges = Vec::new();
    for _ in 0..m {
        let (a, b) = (rng.below(n), rng.below(n));
        let kind = [EdgeKind::Call, EdgeKind::References, EdgeKind::Signature][rng.below(3)];
        let e = if rng.below(4) == 0 {
            Edge::maybe(name(a), name(b), kind)
        } else {
            Edge::new(name(a), name(b), kind)
        };
        edges.push(e);
    }
    // 소유 간선은 의존이 아니다 — 순회가 따라가면 안 된다.
    edges.push(Edge::new(name(0), name(1), EdgeKind::Contains));
    graph::document(
        Level::Symbol,
        ".".into(),
        None,
        vec![],
        vertices,
        edges,
        vec![],
    )
}

/// 오라클 쪽 간선 종류 이름 — 구현과 따로 serde 이름을 읽는다.
fn kind_name(kind: EdgeKind) -> String {
    serde_json::to_value(kind)
        .unwrap()
        .as_str()
        .unwrap()
        .to_string()
}

/// 방향에 맞춘 (출발 → 도착) 인접 목록 — firm_only면 확정 간선만.
fn oracle_adj(doc: &Document, dir: Direction, firm_only: bool) -> BTreeMap<String, Vec<String>> {
    let mut adj: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for e in &doc.edges {
        if !e.kind.is_dependency() || (firm_only && e.tentative) {
            continue;
        }
        let (a, b) = match dir {
            Direction::Dependencies => (e.from.clone(), e.to.clone()),
            Direction::Dependents => (e.to.clone(), e.from.clone()),
        };
        adj.entry(a).or_default().push(b);
    }
    adj
}

fn bfs(adj: &BTreeMap<String, Vec<String>>, root: &str, limit: usize) -> BTreeMap<String, usize> {
    let mut dist = BTreeMap::from([(root.to_string(), 0)]);
    let mut q = VecDeque::from([root.to_string()]);
    while let Some(n) = q.pop_front() {
        let d = dist[&n];
        if d == limit {
            continue;
        }
        for m in adj.get(&n).into_iter().flatten() {
            if !dist.contains_key(m) {
                dist.insert(m.clone(), d + 1);
                q.push_back(m.clone());
            }
        }
    }
    dist
}

/// 한 경우를 오라클과 대조한다.
fn check_case(doc: &Document, roots: &[String], dir: Direction, max_depth: usize) {
    let out = traverse(
        doc,
        &Request {
            roots,
            direction: dir,
            max_depth,
            max_reached: MAX_REACHED,
            evidence_memory_bytes: EVIDENCE_MEMORY_BYTES,
        },
    );
    let full = oracle_adj(doc, dir, false);
    let firm = oracle_adj(doc, dir, true);
    let dists: Vec<_> = roots.iter().map(|r| bfs(&full, r, max_depth)).collect();
    let firm_dists: Vec<_> = roots.iter().map(|r| bfs(&firm, r, max_depth)).collect();
    // 기대 도달 집합: 자기 자신이 아닌 root에서 거리 1 이상으로 닿은 정점.
    let mut expected: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, d) in dists.iter().enumerate() {
        for v in d.keys() {
            if *v != roots[i] {
                expected.entry(v.clone()).or_default().push(i);
            }
        }
    }
    let got: BTreeSet<&str> = out.reached.iter().map(|r| r.id.as_str()).collect();
    let want: BTreeSet<&str> = expected.keys().map(String::as_str).collect();
    assert_eq!(got, want, "reached set differs");
    // (depth, UTF-16 id) 정렬.
    for w in out.reached.windows(2) {
        assert!(w[0]
            .depth
            .cmp(&w[1].depth)
            .then(utf16_cmp(&w[0].id, &w[1].id))
            .is_lt());
    }
    for row in &out.reached {
        let rs = &expected[&row.id];
        let depth = rs.iter().map(|&i| dists[i][&row.id]).min().unwrap();
        assert_eq!(row.depth, depth, "depth of {}", row.id);
        assert_eq!(row.roots, rs.iter().copied().take(64).collect::<Vec<_>>());
        let nearest = *rs.iter().find(|&&i| dists[i][&row.id] == depth).unwrap();
        let via = full
            .iter()
            .filter(|(p, ns)| ns.contains(&row.id) && dists[nearest].get(*p) == Some(&(depth - 1)))
            .map(|(p, _)| p.clone())
            .min_by(|a, b| utf16_cmp(a, b))
            .unwrap();
        assert_eq!(row.via, via, "via of {}", row.id);
        let strongest = |i: usize| {
            if firm_dists[i].contains_key(&row.id) {
                Evidence::Direct
            } else {
                Evidence::Candidate
            }
        };
        let evidence = rs.iter().map(|&i| strongest(i)).max().unwrap();
        assert_eq!(row.evidence, evidence, "evidence of {}", row.id);
        // 관계는 원래 방향 via→정점 의존 간선 종류 이름이다(이름 사전순, 중복 없음).
        let (from, to) = match dir {
            Direction::Dependencies => (&row.via, &row.id),
            Direction::Dependents => (&row.id, &row.via),
        };
        let kinds: BTreeSet<String> = doc
            .edges
            .iter()
            .filter(|e| e.kind.is_dependency() && &e.from == from && &e.to == to)
            .map(|e| kind_name(e.kind))
            .collect();
        assert!(!kinds.is_empty(), "via edge of {}", row.id);
        assert_eq!(
            row.relationships,
            kinds.into_iter().collect::<Vec<_>>(),
            "relationships of {}",
            row.id
        );
    }
    assert_eq!(
        out.roots_truncated,
        expected.values().any(|rs| rs.len() > 64),
        "rootsTruncated"
    );
    if roots.len() <= 64 {
        // 깊이 상한 밖(정확히 max_depth+1)에 정점이 있으면 depth로 잘린다.
        let cut = roots
            .iter()
            .enumerate()
            .any(|(i, r)| bfs(&full, r, max_depth + 1).len() > dists[i].len());
        assert_eq!(out.truncation_reasons.contains(&"depth"), cut, "depth cut");
    }
}

#[test]
fn single_pass_matches_per_root_oracle() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    for case in 0..300 {
        let n = 2 + rng.below(30);
        let m = rng.below(n * 3);
        let doc = random_doc(&mut rng, n, m);
        let k = 1 + rng.below(n.min(6));
        let mut picked: Vec<usize> = (0..n).collect();
        for i in (1..picked.len()).rev() {
            picked.swap(i, rng.below(i + 1));
        }
        let roots: Vec<String> = picked[..k].iter().map(|&i| name(i)).collect();
        let dir = if case % 2 == 0 {
            Direction::Dependencies
        } else {
            Direction::Dependents
        };
        let depth = 1 + rng.below(6);
        check_case(&doc, &roots, dir, depth);
        let reversed: Vec<String> = roots.iter().rev().cloned().collect();
        check_case(&doc, &reversed, dir, depth);
    }
}

/// 같은 root의 요청 순서 세 가지 — 주어진 순서, 역순, 섞은 순서. 섞기는 별도 난수라 호출자의
/// 무작위 사례 열을 바꾸지 않는다.
fn root_orders(roots: &[String], seed: u64) -> [Vec<String>; 3] {
    let mut shuffled = roots.to_vec();
    let mut rng = Rng(seed);
    for i in (1..shuffled.len()).rev() {
        shuffled.swap(i, rng.below(i + 1));
    }
    [
        roots.to_vec(),
        roots.iter().rev().cloned().collect(),
        shuffled,
    ]
}

#[test]
fn many_roots_prune_without_changing_output() {
    // 65개 넘는 root가 한 정점에 모이면 큰 인덱스 root는 전파를 멈춘다 — 출력은 오라클과 같아야 한다.
    // 가지치기는 처리 순서에 기대면 안 되므로 요청 순서(역순·섞음)를 바꿔도 같아야 한다.
    let mut rng = Rng(42);
    for case in 0..20u64 {
        let n = 90 + rng.below(20);
        let doc = random_doc(&mut rng, n, n * 4);
        let k = 70 + rng.below(15);
        let roots: Vec<String> = (0..k).map(name).collect();
        let depth = 1 + rng.below(4);
        for order in root_orders(&roots, case) {
            check_case(&doc, &order, Direction::Dependencies, depth);
        }
    }
    // 허브 하나에 root 80개가 모이는 결정적 경우 — rootsTruncated가 선다.
    let mut vertices: Vec<Vertex> = (0..80).map(|i| vertex(&name(i))).collect();
    vertices.push(vertex("c::hub"));
    vertices.push(vertex("c::leaf"));
    let mut edges: Vec<Edge> = (0..80)
        .map(|i| Edge::new(name(i), "c::hub".into(), EdgeKind::Call))
        .collect();
    edges.push(Edge::new("c::hub".into(), "c::leaf".into(), EdgeKind::Call));
    let doc = graph::document(
        Level::Symbol,
        ".".into(),
        None,
        vec![],
        vertices,
        edges,
        vec![],
    );
    let roots: Vec<String> = (0..80).map(name).collect();
    for order in root_orders(&roots, 80) {
        check_case(&doc, &order, Direction::Dependencies, 8);
    }
}

/// root `r`개 → root마다 중간 정점 하나 → `c::hub` → 자식 `c`개 팬 문서와 root id(오름차순).
/// 허브에는 둘째 단계에 닿는다 — 처음 경계(root마다 자기 정점)가 아니라 전파가 쌓은 경계에서 root가
/// 허브로 몰릴 때도 가지치기가 요청 순서에 기대지 않는지 본다.
fn fan_doc(r: usize, c: usize) -> (Document, Vec<String>) {
    let roots: Vec<String> = (0..r).map(|i| format!("c::r{i:03}")).collect();
    let middles: Vec<String> = (0..r).map(|i| format!("c::m{i:03}")).collect();
    let kids: Vec<String> = (0..c).map(|j| format!("c::k{j:02}")).collect();
    let mut vertices: Vec<Vertex> = roots
        .iter()
        .chain(&middles)
        .chain(&kids)
        .map(|id| vertex(id))
        .collect();
    vertices.push(vertex("c::hub"));
    let mut edges: Vec<Edge> = roots
        .iter()
        .zip(&middles)
        .map(|(x, m)| Edge::new(x.clone(), m.clone(), EdgeKind::Call))
        .collect();
    edges.extend(
        middles
            .iter()
            .map(|m| Edge::new(m.clone(), "c::hub".into(), EdgeKind::Call)),
    );
    edges.extend(
        kids.iter()
            .map(|k| Edge::new("c::hub".into(), k.clone(), EdgeKind::Call)),
    );
    let doc = graph::document(
        Level::Symbol,
        ".".into(),
        None,
        vec![],
        vertices,
        edges,
        vec![],
    );
    (doc, roots)
}

/// 정점 번호별 기록한 root 수와 전파 중단 여부 — 기록 수가 곧 순회 메모리와 이웃 방문 수다.
fn held_counts(doc: &Document, roots: &[String]) -> (Vec<usize>, bool) {
    let g = Graph::new(doc, Direction::Dependencies);
    let starts: Vec<usize> = roots.iter().map(|r| g.index[r]).collect();
    let (levels, _, pruned) = propagate(&g, &starts, MAX_DEPTH);
    (levels.iter().map(HashMap::len).collect(), pruned)
}

#[test]
fn pruning_does_not_depend_on_root_request_order() {
    // root 200개가 허브 하나로 모인다. 어떤 순서로 요청해도 허브와 그 아래 정점은 root를 65개만
    // 싣는다. 도착 순서대로 지배를 따지면 root 인덱스가 정점 id 순서와 반대일 때 가지치기가 전혀
    // 일어나지 않아, 정점마다 root 200개를 싣고 그 수만큼 이웃에 전파했다.
    let (doc, roots) = fan_doc(200, 10);
    let [ascending, reversed, shuffled] = root_orders(&roots, 5);
    let want = held_counts(&doc, &ascending);
    // root·중간 정점은 하나씩, 허브와 자식 10개는 65개씩.
    assert_eq!(want.0.iter().sum::<usize>(), 400 + 11 * DOMINATING_ROOTS);
    assert!(want.1, "the hub prunes");
    assert_eq!(held_counts(&doc, &reversed), want, "reversed request");
    assert_eq!(held_counts(&doc, &shuffled), want, "shuffled request");
}

#[test]
fn each_level_keeps_what_ascending_root_order_would_keep() {
    // 한 단계에 정점으로 오는 root(중복 포함)를 어떤 순서로 보내도, 단계가 끝난 기록은 root를 인덱스
    // 오름차순으로 처리한 결과 — 이전 기록에, 이번에 온 root 가운데 자기 아닌 기록과 합쳐 가장 작은
    // 65개에 드는 것을 더한 것 — 와 같다. 다음 경계에는 새로 남은 root만 있고, 멈춤 신호는 이번에 온
    // root 가운데 떨어진 것이 있을 때만 선다. 정점이 root 자신인 경우와 아닌 경우 모두 본다.
    let mut rng = Rng(23);
    for case in 0..60 {
        let total = 70 + rng.below(150);
        let owner = (case % 2 == 0).then(|| rng.below(total));
        let mut levels: Levels = vec![owner.map(|o| (o, 0)).into_iter().collect()];
        let mut smallest = Smallest::new();
        for level in 1..=5 {
            let arrivals: Vec<usize> = (0..rng.below(120)).map(|_| rng.below(total)).collect();
            let before: BTreeSet<usize> = levels[0].keys().copied().collect();
            let fresh: BTreeSet<usize> = arrivals
                .iter()
                .copied()
                .filter(|r| !before.contains(r))
                .collect();
            let mut pool: Vec<usize> = before
                .iter()
                .copied()
                .filter(|&i| Some(i) != owner)
                .chain(fresh.iter().copied())
                .collect();
            pool.sort_unstable();
            let smallest_65: BTreeSet<usize> = pool.into_iter().take(DOMINATING_ROOTS).collect();
            let kept: BTreeSet<usize> = fresh.intersection(&smallest_65).copied().collect();
            let mut next = BTreeMap::new();
            let mut stopped = false;
            for r in arrivals {
                stopped |= spread(&mut levels, &mut smallest, 0, r, level, &mut next);
            }
            let got: BTreeSet<usize> = levels[0].keys().copied().collect();
            assert_eq!(got, &before | &kept, "case {case} level {level}");
            let reached: BTreeSet<usize> = next.get(&0).into_iter().flatten().copied().collect();
            assert_eq!(reached, kept, "case {case} level {level} next");
            assert!(kept.iter().all(|r| levels[0][r] == level));
            assert_eq!(
                stopped,
                kept.len() < fresh.len(),
                "case {case} level {level}"
            );
        }
    }
}

#[test]
fn depth_cut_survives_pruning_in_every_root_order() {
    // 깊이 2에서 허브(root 200개, 65개만 전파)까지 닿고 자식은 상한 밖이다 — 전파를 멈춘 root가
    // 있어도 도달 정점 집합이 깊이에 잘렸으면 depth를 알린다.
    let (doc, roots) = fan_doc(200, 10);
    for order in root_orders(&roots, 13) {
        let out = traverse(
            &doc,
            &Request {
                roots: &order,
                direction: Direction::Dependencies,
                max_depth: 2,
                max_reached: MAX_REACHED,
                evidence_memory_bytes: EVIDENCE_MEMORY_BYTES,
            },
        );
        assert_eq!(out.truncation_reasons, vec!["depth"]);
        assert!(out.roots_truncated);
        assert!(out.reached.iter().all(|r| !r.id.starts_with("c::k")));
    }
}

#[test]
fn max_reached_cut_keeps_roots_truncated_in_every_root_order() {
    // 허브에 root 80개가 모이지만 max-reached 1이 허브 행을 자른다(깊이 1에서 id가 가장 앞인 c::a만
    // 남는다). 허브에서 전파를 멈췄으니 rootsTruncated는 참이다 — 오름차순 요청에서 늘 그랬고, 요청
    // 순서가 바뀌어도 같아야 한다.
    let (mut doc, roots) = fan_doc(80, 1);
    doc.vertices.push(vertex("c::a"));
    doc.edges
        .push(Edge::new(roots[0].clone(), "c::a".into(), EdgeKind::Call));
    doc.sort();
    for order in root_orders(&roots, 9) {
        let out = traverse(
            &doc,
            &Request {
                roots: &order,
                direction: Direction::Dependencies,
                max_depth: MAX_DEPTH,
                max_reached: 1,
                evidence_memory_bytes: EVIDENCE_MEMORY_BYTES,
            },
        );
        assert_eq!(out.reached.len(), 1);
        assert_eq!(out.reached[0].id, "c::a");
        assert_eq!(out.truncation_reasons, vec!["max-reached"]);
        assert!(out.roots_truncated, "request starting with {}", order[0]);
    }
}

/// 작은 손 그래프 — A(0)·R(1), A→W→V→R, R→V. 계약 문서의 root 항목 예와 같다.
#[test]
fn root_reached_from_other_root_keeps_witness() {
    let ids = ["c::A", "c::R", "c::V", "c::W"];
    let doc = graph::document(
        Level::Symbol,
        ".".into(),
        None,
        vec![],
        ids.iter().map(|i| vertex(i)).collect(),
        vec![
            Edge::new("c::A".into(), "c::W".into(), EdgeKind::Call),
            Edge::new("c::W".into(), "c::V".into(), EdgeKind::Call),
            Edge::new("c::V".into(), "c::R".into(), EdgeKind::Call),
            Edge::new("c::R".into(), "c::V".into(), EdgeKind::Call),
        ],
        vec![],
    );
    let roots = vec!["c::A".to_string(), "c::R".to_string()];
    let out = traverse(
        &doc,
        &Request {
            roots: &roots,
            direction: Direction::Dependencies,
            max_depth: MAX_DEPTH,
            max_reached: MAX_REACHED,
            evidence_memory_bytes: EVIDENCE_MEMORY_BYTES,
        },
    );
    let v = out.reached.iter().find(|r| r.id == "c::V").unwrap();
    assert_eq!(
        (v.via.as_str(), v.depth, v.roots.clone()),
        ("c::R", 1, vec![0, 1])
    );
    let r = out.reached.iter().find(|r| r.id == "c::R").unwrap();
    assert_eq!(
        (r.via.as_str(), r.depth, r.roots.clone()),
        ("c::V", 3, vec![0])
    );
    assert!(out.truncation_reasons.is_empty());
}

#[test]
fn candidate_evidence_is_a_per_root_lower_bound() {
    // A→(추정)X, B→X(확정) — X에 닿는 root 중 A는 추정 간선으로만 닿으니 X는 candidate.
    // B만 root면 direct다.
    let doc = graph::document(
        Level::Symbol,
        ".".into(),
        None,
        vec![],
        ["c::A", "c::B", "c::X"].iter().map(|i| vertex(i)).collect(),
        vec![
            Edge::maybe("c::A".into(), "c::X".into(), EdgeKind::Call),
            Edge::new("c::B".into(), "c::X".into(), EdgeKind::Call),
        ],
        vec![],
    );
    let run = |roots: &[String], bytes: usize| {
        traverse(
            &doc,
            &Request {
                roots,
                direction: Direction::Dependencies,
                max_depth: MAX_DEPTH,
                max_reached: MAX_REACHED,
                evidence_memory_bytes: bytes,
            },
        )
    };
    let both = vec!["c::A".to_string(), "c::B".to_string()];
    let out = run(&both, EVIDENCE_MEMORY_BYTES);
    assert_eq!(out.reached[0].evidence, Evidence::Candidate);
    assert!(!out.evidence_approximated);
    let only_b = vec!["c::B".to_string()];
    assert_eq!(
        run(&only_b, EVIDENCE_MEMORY_BYTES).reached[0].evidence,
        Evidence::Direct
    );
    // 메모리 상한을 넘으면 근사하되 부풀리지 않는다.
    let approx = run(&both, 0);
    assert!(approx.evidence_approximated);
    assert_eq!(approx.reached[0].evidence, Evidence::Candidate);
}

#[test]
fn approximation_never_inflates_evidence() {
    let mut rng = Rng(7);
    for _ in 0..100 {
        let n = 2 + rng.below(25);
        let m = rng.below(n * 3);
        let doc = random_doc(&mut rng, n, m);
        let roots: Vec<String> = (0..1 + rng.below(n.min(4))).map(name).collect();
        let req = |bytes| Request {
            roots: &roots,
            direction: Direction::Dependencies,
            max_depth: 4,
            max_reached: MAX_REACHED,
            evidence_memory_bytes: bytes,
        };
        let exact = traverse(&doc, &req(EVIDENCE_MEMORY_BYTES));
        let approx = traverse(&doc, &req(0));
        for (e, a) in exact.reached.iter().zip(&approx.reached) {
            assert_eq!(e.id, a.id);
            assert!(a.evidence >= e.evidence, "approximation inflated {}", e.id);
        }
    }
}

#[test]
fn max_reached_truncates_in_order() {
    let doc = graph::document(
        Level::Symbol,
        ".".into(),
        None,
        vec![],
        ["c::a", "c::b", "c::c"].iter().map(|i| vertex(i)).collect(),
        vec![
            Edge::new("c::a".into(), "c::b".into(), EdgeKind::Call),
            Edge::new("c::b".into(), "c::c".into(), EdgeKind::Call),
        ],
        vec![],
    );
    let roots = vec!["c::a".to_string()];
    let out = traverse(
        &doc,
        &Request {
            roots: &roots,
            direction: Direction::Dependencies,
            max_depth: MAX_DEPTH,
            max_reached: 1,
            evidence_memory_bytes: EVIDENCE_MEMORY_BYTES,
        },
    );
    assert_eq!(out.reached.len(), 1);
    assert_eq!(out.reached[0].id, "c::b");
    assert_eq!(out.truncation_reasons, vec!["max-reached"]);
}

#[test]
fn edge_kind_names_are_made_per_kind_not_per_edge() {
    // 간선이 수백만인 그래프에서 쌍마다 종류 이름 문자열 집합을 두면 순회 메모리가 문서의 몇 배가
    // 된다 — 이름은 종류마다 한 번만 만들고 쌍에는 종류 비트만 둔다.
    let mut rng = Rng(11);
    let doc = random_doc(&mut rng, 200, 2_000);
    let before = kind_names_made();
    let out = traverse(
        &doc,
        &Request {
            roots: &[name(0), name(1), name(2)],
            direction: Direction::Dependencies,
            max_depth: MAX_DEPTH,
            max_reached: MAX_REACHED,
            evidence_memory_bytes: EVIDENCE_MEMORY_BYTES,
        },
    );
    let made = kind_names_made() - before;
    // 의존 간선 종류는 call·references·signature 셋뿐이다.
    assert!(made <= 3, "{made} kind names for {} edges", doc.edges.len());
    assert!(out.reached.len() > 100, "the walk must cover the graph");
}

#[test]
fn relationships_name_every_dependency_kind_on_the_via_pair() {
    // a→b에 call·references·signature(추정)·uses가 겹치고 contains는 의존이 아니라 빠진다.
    // 역방향 순회도 원래 방향(a→b) 간선 종류를 싣는다. 순서는 이름 사전순이다.
    let edges = vec![
        Edge::maybe("c::a".into(), "c::b".into(), EdgeKind::Signature),
        Edge::new("c::a".into(), "c::b".into(), EdgeKind::Uses),
        Edge::new("c::a".into(), "c::b".into(), EdgeKind::Call),
        Edge::new("c::a".into(), "c::b".into(), EdgeKind::References),
        Edge::new("c::a".into(), "c::b".into(), EdgeKind::Contains),
    ];
    let doc = graph::document(
        Level::Symbol,
        ".".into(),
        None,
        vec![],
        vec![vertex("c::a"), vertex("c::b")],
        edges,
        vec![],
    );
    let walk = |root: &str, direction| {
        let roots = vec![root.to_string()];
        traverse(
            &doc,
            &Request {
                roots: &roots,
                direction,
                max_depth: MAX_DEPTH,
                max_reached: MAX_REACHED,
                evidence_memory_bytes: EVIDENCE_MEMORY_BYTES,
            },
        )
        .reached
    };
    let want = vec!["call", "references", "signature", "uses"];
    assert_eq!(walk("c::a", Direction::Dependencies)[0].relationships, want);
    assert_eq!(walk("c::b", Direction::Dependents)[0].relationships, want);
}

#[test]
fn direction_strings_are_contract_values() {
    assert_eq!(Direction::Dependencies.as_str(), "dependencies");
    assert_eq!(Direction::Dependents.as_str(), "dependents");
    assert_eq!(Evidence::Direct.as_str(), "direct");
    assert_eq!(Evidence::Candidate.as_str(), "candidate");
}
