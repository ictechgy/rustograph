//! 다중 root 단일 패스를 root별 너비 우선 탐색(오라클)과 무작위 그래프로 대조한다.

use super::*;
use crate::graph::{self, Edge, EdgeKind, Kind, Level, Vertex};
use std::collections::VecDeque;

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
        // 관계는 원래 방향 via→정점 간선 종류다.
        assert!(!row.relationships.is_empty());
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
        check_case(&doc, &roots, dir, 1 + rng.below(6));
    }
}

#[test]
fn many_roots_prune_without_changing_output() {
    // 65개 넘는 root가 한 정점에 모이면 큰 인덱스 root는 전파를 멈춘다 — 출력은 오라클과 같아야 한다.
    let mut rng = Rng(42);
    for _ in 0..20 {
        let n = 90 + rng.below(20);
        let doc = random_doc(&mut rng, n, n * 4);
        let k = 70 + rng.below(15);
        let roots: Vec<String> = (0..k).map(name).collect();
        check_case(&doc, &roots, Direction::Dependencies, 1 + rng.below(4));
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
    check_case(&doc, &roots, Direction::Dependencies, 8);
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
fn direction_strings_are_contract_values() {
    assert_eq!(Direction::Dependencies.as_str(), "dependencies");
    assert_eq!(Direction::Dependents.as_str(), "dependents");
    assert_eq!(Evidence::Direct.as_str(), "direct");
    assert_eq!(Evidence::Candidate.as_str(), "candidate");
}
