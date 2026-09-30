//! 생산 문서 자체 검사 — isthmus 파서가 거부할 `order`·`limitationScopes`를 내기
//! 전에 잡는다.
//!
//! 규칙의 정본은 isthmus `src/exchange/parse.ts`(`validateRouteOrder`·
//! `validateRouteDocumentFacts`·`validateRouteOrderGroups`)와 스코프 검증이며, 공유
//! 벡터 `dispatch.validate`·`scope.validate`로 같은 판정을 확인한다. JSON 값 위에서
//! 동작해 벡터(소수·문자열 index 같은 잘못된 입력)와 생산 문서를 한 구현으로 본다.

use super::template::template_problem;
use serde_json::Value;
use std::collections::BTreeMap;

/// `order.group` 최대 길이(UTF-16 코드 단위) — isthmus `MAX_ROUTE_ORDER_GROUP_LENGTH`.
const MAX_GROUP_LENGTH: usize = 256;

/// JS `Number.MAX_SAFE_INTEGER` — index는 안전 정수여야 한다.
const MAX_SAFE_INTEGER: u64 = (1 << 53) - 1;

/// isthmus가 받는 route 동사다(`ANY`는 스코프 methods에 올 수 없다).
const HTTP_METHODS: &[&str] = &[
    "GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "TRACE",
];

/// http 문서(`dispatch`·`service`·`facts`)의 `order`·catch-all 접두사 규칙 위반을
/// 찾는다. 문제가 없으면 None이다.
pub fn order_problem(document: &Value) -> Option<String> {
    let dispatch = document.get("dispatch");
    let facts = document.get("facts")?.as_array()?;
    for (i, fact) in facts.iter().enumerate() {
        if let Some(order) = fact.get("order") {
            if let Some(problem) = order_value_problem(order, dispatch) {
                return Some(format!("{problem} at index {i}"));
            }
        }
    }
    catch_all_prefix_problem(document, facts).or_else(|| group_problem(document, facts))
}

/// `order` 값 하나의 형식 위반이다.
fn order_value_problem(order: &Value, dispatch: Option<&Value>) -> Option<&'static str> {
    if dispatch.and_then(Value::as_str) != Some("registration-order") {
        return Some("order requires dispatch registration-order");
    }
    let Some(obj) = order.as_object() else {
        return Some("order must be an object");
    };
    if obj.keys().any(|k| k != "group" && k != "index") {
        return Some("order has keys other than group and index");
    }
    let group_ok = obj
        .get("group")
        .and_then(Value::as_str)
        .is_some_and(is_group_string);
    if !group_ok {
        return Some("order group must be a safe non-empty string");
    }
    let index_ok = obj
        .get("index")
        .and_then(Value::as_u64)
        .is_some_and(|n| n <= MAX_SAFE_INTEGER);
    if !index_ok {
        return Some("order index must be a non-negative safe integer");
    }
    None
}

/// group 문자열 규칙 — 비어 있지 않고, 제어 문자·앞뒤 공백이 없고, 256자 이하.
fn is_group_string(group: &str) -> bool {
    !group.trim().is_empty()
        && group.trim() == group
        && group.encode_utf16().count() <= MAX_GROUP_LENGTH
        && !group.chars().any(is_control)
}

/// isthmus `controlCharacterPattern`(C0·DEL·C1·U+2028·U+2029)과 같은 판정이다.
fn is_control(c: char) -> bool {
    matches!(c, '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}' | '\u{2028}' | '\u{2029}')
}

/// 사실의 유효 service(사실 값, 없으면 문서 값)다.
fn effective_service<'a>(document: &'a Value, fact: &'a Value) -> Option<&'a str> {
    fact.get("service")
        .or_else(|| document.get("service"))
        .and_then(Value::as_str)
}

/// catch-all 접두사 decl은 같은 method·usr·service·order의 원본 `{**}` decl이 있어야 한다.
fn catch_all_prefix_problem(document: &Value, facts: &[Value]) -> Option<String> {
    let key = |fact: &Value, channel: &str| {
        serde_json::json!([
            fact.get("method"),
            fact.pointer("/symbol/usr"),
            channel,
            effective_service(document, fact),
            fact.get("order"),
        ])
        .to_string()
    };
    let originals: std::collections::BTreeSet<String> = facts
        .iter()
        .filter(|f| {
            f.get("dynamic") != Some(&Value::Bool(true)) && f.get("catchAllPrefix").is_none()
        })
        .filter_map(|f| f.get("channel").and_then(Value::as_str).map(|c| key(f, c)))
        .collect();
    for (i, fact) in facts.iter().enumerate() {
        if fact.get("catchAllPrefix") != Some(&Value::Bool(true)) {
            continue;
        }
        let channel = fact.get("channel").and_then(Value::as_str).unwrap_or("");
        let original = if channel == "/" {
            "/{**}".to_string()
        } else {
            format!("{channel}/{{**}}")
        };
        if !originals.contains(&key(fact, &original)) {
            return Some(format!(
                "catch-all prefix declaration has no matching {{**}} declaration at index {i}"
            ));
        }
    }
    None
}

/// 한 (group, index)는 한 위치, 한 group은 한 service다.
fn group_problem(document: &Value, facts: &[Value]) -> Option<String> {
    let mut registrations: BTreeMap<String, String> = BTreeMap::new();
    let mut services: BTreeMap<String, Option<String>> = BTreeMap::new();
    for (i, fact) in facts.iter().enumerate() {
        let Some(order) = fact.get("order") else {
            continue;
        };
        let group = order.get("group").and_then(Value::as_str).unwrap_or("");
        let service = effective_service(document, fact).map(str::to_string);
        if let Some(seen) = services.get(group) {
            if *seen != service {
                return Some(format!(
                    "order group is shared by different services at index {i}"
                ));
            }
        }
        services.insert(group.to_string(), service);
        let reg = serde_json::json!([group, order.get("index")]).to_string();
        let loc = serde_json::json!([
            fact.pointer("/location/path"),
            fact.pointer("/location/line"),
            fact.pointer("/location/column"),
        ])
        .to_string();
        if let Some(seen) = registrations.get(&reg) {
            if *seen != loc {
                return Some(format!(
                    "order index is shared by registrations at different locations at index {i}"
                ));
            }
        }
        registrations.insert(reg, loc);
    }
    None
}

/// http `limitationScopes` 항목 하나의 위반이다. 문제가 없으면 None.
pub fn scope_problem(scope: &Value) -> Option<&'static str> {
    let Some(obj) = scope.as_object() else {
        return Some("scope must be an object");
    };
    const KEYS: &[&str] = &[
        "limitationIndex",
        "templates",
        "templatePrefixes",
        "templateSuffixes",
        "methods",
    ];
    if obj.keys().any(|k| !KEYS.contains(&k.as_str())) {
        return Some("unknown scope key");
    }
    if !obj.get("limitationIndex").is_some_and(Value::is_u64) {
        return Some("limitationIndex must be a non-negative integer");
    }
    let paths = ["templates", "templatePrefixes", "templateSuffixes"];
    if !paths.iter().any(|k| obj.contains_key(*k)) {
        return Some("scope needs a path field");
    }
    for field in paths {
        if let Some(value) = obj.get(field) {
            if let Some(problem) = path_field_problem(field, value) {
                return Some(problem);
            }
        }
    }
    obj.get("methods").and_then(methods_problem)
}

/// 경로 필드 하나 — 비어 있지 않은 정규 템플릿 배열, 접두사·접미사 제한.
fn path_field_problem(field: &str, value: &Value) -> Option<&'static str> {
    let Some(items) = value.as_array().filter(|a| !a.is_empty()) else {
        return Some("scope path field must be a non-empty array");
    };
    for item in items {
        let Some(t) = item.as_str() else {
            return Some("scope path element must be a string");
        };
        if template_problem(t).is_some() {
            return Some("scope path element is not a canonical template");
        }
        let catch_all = t.split('/').any(|s| s == "{**}");
        match field {
            "templatePrefixes" if catch_all || (t != "/" && t.ends_with('/')) => {
                return Some("template prefix must not end with / or contain {**}")
            }
            "templateSuffixes" if catch_all || t == "/" => {
                return Some("template suffix must not be / or contain {**}")
            }
            _ => {}
        }
    }
    None
}

/// `methods` — 중복 없는 HTTP 동사(`ANY` 제외)의 비어 있지 않은 배열.
fn methods_problem(value: &Value) -> Option<&'static str> {
    let Some(items) = value.as_array().filter(|a| !a.is_empty()) else {
        return Some("scope methods must be a non-empty array");
    };
    let mut seen = std::collections::BTreeSet::new();
    for item in items {
        let ok = item
            .as_str()
            .is_some_and(|m| HTTP_METHODS.contains(&m) && seen.insert(m));
        if !ok {
            return Some("scope methods must be distinct HTTP verbs");
        }
    }
    None
}
