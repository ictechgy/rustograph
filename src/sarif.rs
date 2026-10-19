//! SARIF 2.1.0 출력 — CI 코드 스캐닝(GitHub 등)이 소비하는 형식.
//!
//! 규칙 위반을 SARIF result로 옮긴다. 위치는 정점의 file:line을 쓰고,
//! 없으면 결과는 메시지만 싣는다 — 없는 위치를 지어내지 않는다.

use crate::graph::{Document, Vertex};
use crate::rules::Violation;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// 위반 목록을 SARIF 로그로 직렬화한다. `version`은 `tool.driver.version`에 싣는 도구
/// 버전이다 — 호출자(진입점)가 넘긴다. 출력 계층이 `crate::cli::VERSION`을 직접 읽으면
/// cli ↔ sarif 모듈 순환인데, `json!` 매크로 토큰 안의 경로는 수확되지 않아 자기 분석
/// (`cycles --strict`)도 이 순환을 보지 못한다(cli::reach와 같은 방식).
pub fn rules_sarif(doc: &Document, violations: &[Violation], version: &str) -> String {
    // 위치 조회는 규칙 검사와 같은 색인(같은 ID의 첫 cfg 변형)을 한 번 만들어 쓴다 —
    // 위반마다 정점 배열을 훑으면 O(위반·V)다.
    let by_id = crate::rules::vertices_by_id(doc);
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
        if let Some(loc) = location(&by_id, &v.from) {
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
                    "version": version,
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
fn location(by_id: &BTreeMap<&str, &Vertex>, vertex_id: &str) -> Option<Value> {
    let pos = by_id.get(vertex_id).and_then(|v| v.position.as_deref())?;
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
                cfg: None,
                unsafe_: false,
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
        let s = rules_sarif(&doc, &[v], "9.9.9-test");
        let j: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(j["version"], "2.1.0");
        // 도구 버전은 호출자가 넘긴 값 그대로다 — sarif가 cli를 읽지 않는다.
        assert_eq!(j["runs"][0]["tool"]["driver"]["version"], "9.9.9-test");
        assert_eq!(j["runs"][0]["results"][0]["ruleId"], "rustograph/deny");
        // 위치는 정점의 file:line에서 온다.
        let loc = &j["runs"][0]["results"][0]["locations"][0]["physicalLocation"];
        assert_eq!(loc["artifactLocation"]["uri"], "src/lib.rs");
        assert_eq!(loc["region"]["startLine"], 10);
    }

    /// `from`이 정점 id인 deny 위반 하나.
    fn violation(from: &str) -> Violation {
        Violation {
            rule: "deny".into(),
            from: from.into(),
            to: "c::b".into(),
            component: "core".into(),
            forbidden: "ui".into(),
            kind: EdgeKind::Call,
        }
    }

    /// 위치가 `position`인 fn 정점.
    fn located(id: &str, position: &str, cfg: Option<&str>) -> Vertex {
        Vertex {
            id: id.into(),
            kind: Kind::Fn,
            krate: "c".into(),
            module: "c".into(),
            position: Some(position.into()),
            exported: true,
            generated: false,
            cfg: cfg.map(str::to_string),
            unsafe_: false,
        }
    }

    /// 결과 하나의 (uri, startLine).
    fn place(result: &Value) -> (Value, Value) {
        let loc = &result["locations"][0]["physicalLocation"];
        (
            loc["artifactLocation"]["uri"].clone(),
            loc["region"]["startLine"].clone(),
        )
    }

    /// 같은 ID의 cfg 변형은 문서 순서상 첫 변형의 위치를 쓴다 — 규칙 검사가 첫 변형의
    /// module로 판정한 위반이 다른 변형의 파일을 가리키면 안 된다.
    #[test]
    fn location_uses_first_cfg_variant() {
        let doc = document(
            Level::Symbol,
            ".".into(),
            None,
            vec![],
            vec![
                located("c::a", "src/unix.rs:10", Some("unix")),
                located("c::a", "src/windows.rs:20", Some("windows")),
            ],
            vec![],
            vec![],
        );
        let j: Value = serde_json::from_str(&rules_sarif(&doc, &[violation("c::a")], "t")).unwrap();
        assert_eq!(
            place(&j["runs"][0]["results"][0]),
            (json!("src/unix.rs"), json!(10))
        );
    }

    /// 위반마다 정점 배열을 처음부터 훑어 위치를 찾으면 O(위반·V)다 — 기준선을 만들기 전
    /// 위반이 17만 건인 합성 문서의 `--format sarif`가 22초였다. 위치 조회가 정점을 한 번씩만
    /// 살피는지 센다.
    #[test]
    fn locations_examine_each_vertex_once() {
        let n = 200;
        let vertices = (0..n)
            .map(|i| {
                located(
                    &format!("c::f{i:03}"),
                    &format!("src/lib.rs:{}", i + 1),
                    None,
                )
            })
            .collect();
        let doc = document(
            Level::Symbol,
            ".".into(),
            None,
            vec![],
            vertices,
            vec![],
            vec![],
        );
        let violations: Vec<Violation> = (0..n)
            .rev()
            .map(|i| violation(&format!("c::f{i:03}")))
            .collect();
        let before = crate::rules::examined_vertices();
        let s = rules_sarif(&doc, &violations, "t");
        let examined = crate::rules::examined_vertices() - before;
        let j: Value = serde_json::from_str(&s).unwrap();
        let results = j["runs"][0]["results"].as_array().unwrap();
        assert_eq!(results.len(), n);
        assert_eq!(place(&results[0]), (json!("src/lib.rs"), json!(n)));
        assert!(examined <= n, "{examined} vertex visits for {n} vertices");
    }
}
