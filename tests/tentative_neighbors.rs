//! query 결과가 이름 팬아웃을 확정 호출로 읽히게 만들지 않는지 확인한다.

use rustograph::{analysis, graph::Document};
use serde_json::json;

#[test]
fn query_and_impact_preserve_candidate_evidence_per_kind_and_path() {
    let vertices =
        ["a", "b", "c", "d"].map(|id| json!({"id":id,"kind":"fn","crate":"app","module":"app"}));
    let d: Document = serde_json::from_value(json!({
        "version": 1, "tool": "rustograph", "level": "symbol", "root": ".", "roots": [],
        "vertices": vertices,
        "edges": [
            {"from":"a","to":"b","kind":"call","tentative":true},
            {"from":"a","to":"b","kind":"references"},
            {"from":"b","to":"c","kind":"call","tentative":true},
            {"from":"c","to":"d","kind":"call"}
        ]
    }))
    .unwrap();
    let q = serde_json::to_value(analysis::query(&d, "a", 3, 20)).unwrap();
    let row = |id: &str| {
        q["neighbors"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == id)
            .unwrap()
    };
    assert_eq!(row("b")["evidence"], "direct");
    assert_eq!(
        row("b")["edge_evidence"],
        json!([
            {"relationship":"call","evidence":"candidate"},
            {"relationship":"references","evidence":"direct"}
        ])
    );
    assert_eq!(row("c")["evidence"], "candidate");
    assert_eq!(
        row("d")["evidence"],
        "candidate",
        "certainty cannot be recovered after a candidate hop"
    );
    let impact = serde_json::to_value(analysis::impact(&d, "d", 3, 20)).unwrap();
    assert!(impact["impacted"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["id"] == "a" && r["evidence"] == "candidate"));
    let diagram = rustograph::export::to_mermaid(&d);
    assert_eq!(diagram.matches("|call (candidate)|").count(), 2);
    assert!(
        diagram.contains("|call|"),
        "confirmed edges retain their original label"
    );
}

#[test]
fn legacy_snapshot_stays_readable_but_duplicate_identity_is_explicit() {
    let d: Document = serde_json::from_value(json!({
        "version":1,"tool":"rustograph","level":"symbol","root":".","roots":[],
        "vertices":[
            {"id":"caller","kind":"fn","crate":"app","module":"app"},
            {"id":"old::from","kind":"method","crate":"app","module":"app","position":"one.rs:2"},
            {"id":"old::from","kind":"method","crate":"app","module":"app","position":"two.rs:3"}
        ],
        "edges":[{"from":"caller","to":"old::from","kind":"call"}]
    }))
    .unwrap();
    let q = serde_json::to_value(analysis::query(&d, "old::from", 1, 10)).unwrap();
    assert_eq!(q["found"], true);
    assert_eq!(q["neighbors"][0]["evidence"], "candidate");
    assert!(q["limitations"][0]
        .as_str()
        .unwrap()
        .contains("1 vertex IDs are shared by 1 additional rows"));
    let impact = serde_json::to_value(analysis::impact(&d, "old::from", 1, 10)).unwrap();
    assert_eq!(impact["witnesses"][0]["evidence"], "candidate");
}
