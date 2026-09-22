//! SARIF 2.1.0 출력 — CI 코드 스캐닝(GitHub 등)이 소비하는 형식.
//!
//! 규칙 위반을 SARIF result로 옮긴다. 위치는 정점의 file:line을 쓰고,
//! 없으면 결과는 메시지만 싣는다 — 없는 위치를 지어내지 않는다.

use crate::graph::Document;
use crate::rules::Violation;
use serde_json::{json, Map, Value};

/// 위반 목록을 SARIF 로그로 직렬화한다.
pub fn rules_sarif(doc: &Document, violations: &[Violation]) -> String {
    let mut results = Vec::new();
    for v in violations {
        let mut result = Map::new();
        result.insert(
            "ruleId".to_string(),
            json!(format!("rustograph/{}", v.rule)),
        );
        result.insert(
            "level".to_string(),
            json!(if v.rule == "deny" { "error" } else { "warning" }),
        );
        result.insert(
            "message".to_string(),
            json!(format!(
                "{} component '{}' may not depend on '{}' ({:?} edge: {} -> {})",
                v.rule, v.component, v.forbidden, v.kind, v.from, v.to
            )),
        );
        if let Some(loc) = location(doc, &v.from) {
            result.insert("locations".to_string(), json!([loc]));
        }
        results.push(Value::Object(result));
    }
    let log = json!({
        "version": "2.1.0",
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "rustograph",
                    "version": crate::cli::VERSION,
                    "informationUri": "https://github.com/ictechgy/rustograph",
                    "rules": [
                        {"id": "rustograph/allow", "name": "AllowlistViolation"},
                        {"id": "rustograph/deny", "name": "DenyViolation"},
                        {"id": "rustograph/signature", "name": "SignatureLeak"},
                    ]
                }
            },
            "results": results
        }]
    });
    let mut s = serde_json::to_string_pretty(&log).expect("sarif must serialize");
    s.push('\n');
    s
}

/// 정점의 `file:line` 위치를 SARIF location으로 변환한다.
fn location(doc: &Document, vertex_id: &str) -> Option<Value> {
    let pos = doc
        .vertices
        .iter()
        .find(|v| v.id == vertex_id)
        .and_then(|v| v.position.clone())?;
    let (file, line) = pos
        .rsplit_once(':')
        .map(|(f, l)| (f.to_string(), l.parse::<u32>().unwrap_or(1)))?;
    Some(json!({
        "physicalLocation": {
            "artifactLocation": {"uri": file},
            "region": {"startLine": line.max(1)}
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::EdgeKind;
    use crate::graph::{document, Kind, Level, Vertex};
    use crate::rules::Violation;

    #[test]
    fn sarif_shape_is_valid() {
        let doc = document(
            Level::Symbol,
            ".".into(),
            None,
            vec![],
            vec![Vertex {
                id: "c::a".into(),
                kind: Kind::Fn,
                krate: "c".into(),
                module: "c".into(),
                position: Some("src/lib.rs:10".into()),
                exported: true,
                generated: false,
            }],
            vec![],
            vec![],
        );
        let v = Violation {
            rule: "deny".into(),
            from: "c::a".into(),
            to: "c::b".into(),
            component: "core".into(),
            forbidden: "ui".into(),
            kind: EdgeKind::Call,
        };
        let s = rules_sarif(&doc, &[v]);
        let j: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(j["version"], "2.1.0");
        assert_eq!(j["runs"][0]["results"][0]["ruleId"], "rustograph/deny");
        // 위치는 정점의 file:line에서 온다.
        let loc = &j["runs"][0]["results"][0]["locations"][0]["physicalLocation"];
        assert_eq!(loc["artifactLocation"]["uri"], "src/lib.rs");
        assert_eq!(loc["region"]["startLine"], 10);
    }
}
