//! semantic 엔진 통합 테스트 — `cargo test --features semantic`에서만 돈다.
//! 계약: 그래프 스키마·정점 ID는 syn 수확과 같고, 정확도만 올라간다.
//! 같은 이름 메서드는 타입으로 구별하고, 매크로는 확장 트리를 걷는다.

#![cfg(feature = "semantic")]

use rustograph::graph::{Document, Edge, EdgeKind};
use rustograph::source::{self, Options};
use std::path::PathBuf;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixture")
}

fn sem_doc() -> Document {
    source::load(
        &fixture(),
        &Options {
            symbol_level: true,
            semantic: true,
            ..Default::default()
        },
    )
    .expect("semantic fixture load failed")
}

fn syn_doc() -> Document {
    source::load(
        &fixture(),
        &Options {
            symbol_level: true,
            ..Default::default()
        },
    )
    .expect("syntactic fixture load failed")
}

fn call<'a>(d: &'a Document, from: &str, to: &str) -> Option<&'a Edge> {
    d.edges
        .iter()
        .find(|e| e.from == from && e.to == to && e.kind == EdgeKind::Call)
}

fn edge_key(e: &Edge) -> (&str, &str, EdgeKind, bool) {
    (e.from.as_str(), e.to.as_str(), e.kind, e.tentative)
}

#[test]
fn method_call_on_concrete_type_is_firm() {
    let d = sem_doc();
    // u: Used — greet은 Greet impl 메서드로 정확히 해석되어 추정이 아니다.
    let e = call(
        &d,
        "fixture_app::main",
        "fixture_core::Used::<Greet>::greet",
    )
    .expect("semantic call edge");
    assert!(!e.tentative, "concrete receiver must resolve firmly");
    // syn 팬아웃이 만들던 무관한 같은-이름 메서드는 없어야 한다.
    assert!(call(&d, "fixture_app::main", "fixture_core::Other::greet").is_none());
}

#[test]
fn inherent_and_self_calls_are_firm() {
    let d = sem_doc();
    let e = call(&d, "fixture_app::main", "fixture_core::Used::quadrupled")
        .expect("inherent method call edge");
    assert!(!e.tentative);
    let e = call(
        &d,
        "fixture_core::Used::quadrupled",
        "fixture_core::Used::doubled",
    )
    .expect("self method call edge");
    assert!(!e.tentative);
}

#[test]
fn trait_dispatch_expands_to_impl_matrix() {
    let d = sem_doc();
    // dyn 수신자 — 실제 impl을 모르니 후보는 추정 간선이다.
    let decl = call(
        &d,
        "fixture_core::dyn_dispatch",
        "fixture_core::Greet::greet",
    )
    .expect("trait decl candidate");
    assert!(decl.tentative);
    let imp = call(
        &d,
        "fixture_core::dyn_dispatch",
        "fixture_core::Used::<Greet>::greet",
    )
    .expect("impl candidate");
    assert!(imp.tentative);
    // Greet을 구현하지 않은 Other::greet은 후보가 아니다.
    assert!(call(
        &d,
        "fixture_core::dyn_dispatch",
        "fixture_core::Other::greet"
    )
    .is_none());
    // 제네릭 수신자도 같은 행렬이다.
    assert!(call(
        &d,
        "fixture_core::generic_dispatch",
        "fixture_core::Used::<Greet>::greet"
    )
    .is_some_and(|e| e.tentative));
}

#[test]
fn macro_expansion_yields_inner_call() {
    let d = sem_doc();
    // emit_helper!() 확장 안의 helper 호출 — syn은 빈 토큰만 보고 놓친다.
    let e =
        call(&d, "fixture_app::main", "fixture_core::util::helper").expect("expanded call edge");
    assert!(!e.tentative);
    // 매크로 정의 자체로의 간선도 있다.
    assert!(call(&d, "fixture_app::main", "fixture_core::emit_helper").is_some());
    // syn 모드에서는 이 간선이 없다 — 개선의 증거.
    let s = syn_doc();
    assert!(call(&s, "fixture_app::main", "fixture_core::util::helper").is_none());
}

#[test]
fn graph_schema_and_ids_unchanged() {
    let s = syn_doc();
    let d = sem_doc();
    // 정점 집합은 엔진과 무관하게 동일 — 정점은 syn이 만든다.
    let mut sv: Vec<_> = s.vertices.iter().map(|v| &v.id).collect();
    let mut dv: Vec<_> = d.vertices.iter().map(|v| &v.id).collect();
    sv.sort();
    dv.sort();
    assert_eq!(sv, dv);
    // 구조 간선(contains/uses/implements/signature)도 동일해야 한다.
    // Edge는 PartialEq가 없으니 키 튜플로 비교한다.
    for kind in [
        EdgeKind::Contains,
        EdgeKind::Uses,
        EdgeKind::Implements,
        EdgeKind::Signature,
    ] {
        let se: Vec<_> = s
            .edges
            .iter()
            .filter(|e| e.kind == kind)
            .map(edge_key)
            .collect();
        let de: Vec<_> = d
            .edges
            .iter()
            .filter(|e| e.kind == kind)
            .map(edge_key)
            .collect();
        assert_eq!(se, de, "structural {kind:?} edges differ");
    }
    // 한계는 여전히 실측이다 — semantic 통계 문장이 있어야 한다.
    assert!(d
        .limitations
        .iter()
        .any(|l| l.contains("semantic analysis:")));
}

#[test]
fn syn_mode_still_fans_out() {
    // 기본 모드 계약은 그대로 — 같은-이름 팬아웃이 추정 간선으로 남는다.
    let s = syn_doc();
    let e = call(
        &s,
        "fixture_app::main",
        "fixture_core::Used::<Greet>::greet",
    )
    .expect("syntactic fan-out edge");
    assert!(e.tentative);
    assert!(
        call(&s, "fixture_app::main", "fixture_core::Other::greet").is_some_and(|e| e.tentative)
    );
}
