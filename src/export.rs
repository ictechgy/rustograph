//! 문서 출력 — 결정적 JSON, Mermaid, 저장/읽기.
//!
//! serde_json은 필드 선언 순서로 직렬화하고 BTreeMap은 정렬된 키를 낸다 —
//! 같은 입력이 같은 바이트가 되는 계약은 struct 필드 순서와 정렬된 Vec에 있다.

use crate::graph::Document;
use std::path::Path;

/// 문서를 결정적 pretty JSON으로 직렬화한다.
pub fn to_json<T: serde::Serialize>(v: &T) -> String {
    let mut s = serde_json::to_string_pretty(v).expect("document must serialize");
    s.push('\n');
    s
}

/// Mermaid 그래프 — 사람이 보는 용도. 정점 ID는 그대로 라벨이 된다.
pub fn to_mermaid(doc: &Document) -> String {
    let mut out = String::from("graph TD\n");
    let ids: std::collections::BTreeSet<&str> = doc.vertex_ids();
    for e in &doc.edges {
        if !ids.contains(e.from.as_str()) || !ids.contains(e.to.as_str()) {
            continue;
        }
        out.push_str(&format!(
            "    {}[\"{}\"] -->|{}| {}[\"{}\"]\n",
            node_id(&e.from),
            e.from,
            edge_label(e.kind),
            node_id(&e.to),
            e.to
        ));
    }
    out
}

fn node_id(id: &str) -> String {
    // mermaid 노드 ID에 쓸 수 없는 문자를 밑줄로.
    id.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
}

fn edge_label(kind: crate::graph::EdgeKind) -> &'static str {
    use crate::graph::EdgeKind::*;
    match kind {
        Depends => "depends",
        Uses => "uses",
        Contains => "contains",
        Call => "call",
        References => "references",
        Implements => "implements",
        Signature => "signature",
    }
}

/// 문서를 파일로 저장한다.
pub fn save_file(doc: &Document, path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    std::fs::write(path, to_json(doc)).map_err(|e| format!("cannot write {}: {e}", path.display()))
}

/// 저장된 문서를 읽는다. 미래 버전은 거부한다 — 모르는 필드보다 버전 불일치가 위험하다.
pub fn load_file(path: &Path) -> Result<Document, String> {
    let src = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let doc: Document =
        serde_json::from_str(&src).map_err(|e| format!("bad {}: {e}", path.display()))?;
    if doc.version > crate::graph::DOCUMENT_VERSION {
        return Err(format!(
            "graph document version {} is newer than this tool understands ({})",
            doc.version,
            crate::graph::DOCUMENT_VERSION
        ));
    }
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{document, Edge, EdgeKind, Kind, Level, Vertex};

    fn doc() -> Document {
        document(
            Level::Module,
            ".".into(),
            None,
            vec!["c".into()],
            vec![
                Vertex {
                    id: "c".into(),
                    kind: Kind::Crate,
                    krate: "c".into(),
                    module: "c".into(),
                    position: None,
                    exported: true,
                    generated: false,
                    cfg: None,
                    unsafe_: false,
                },
                Vertex {
                    id: "c::m".into(),
                    kind: Kind::Module,
                    krate: "c".into(),
                    module: "c::m".into(),
                    position: Some("src/m.rs:1".into()),
                    exported: false,
                    generated: false,
                    cfg: None,
                    unsafe_: false,
                },
            ],
            vec![Edge::new("c".into(), "c::m".into(), EdgeKind::Contains)],
            vec![],
        )
    }

    #[test]
    fn json_is_deterministic() {
        let a = to_json(&doc());
        let b = to_json(&doc());
        assert_eq!(a, b);
        // 정점·간선 배열이 id/from/to/kind 순으로 정렬돼 있다 — 같은 입력이
        // 같은 바이트여야 diff·캐시가 성립한다.
        let j: serde_json::Value = serde_json::from_str(&a).unwrap();
        let ids: Vec<&str> = j["vertices"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["id"].as_str().unwrap())
            .collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!("rg-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("g.json");
        save_file(&doc(), &p).unwrap();
        let loaded = load_file(&p).unwrap();
        assert_eq!(to_json(&loaded), to_json(&doc()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_rejects_future_version() {
        let dir = std::env::temp_dir().join(format!("rg-export2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("g.json");
        std::fs::write(&p, r#"{"version":999,"tool":"rustograph","level":"module","root":".","roots":[],"vertices":[],"edges":[]}"#).unwrap();
        assert!(load_file(&p).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mermaid_has_nodes_and_edges() {
        let m = to_mermaid(&doc());
        assert!(m.contains("graph"));
        assert!(m.contains("c__m"));
        assert!(m.contains("contains"));
    }
}
