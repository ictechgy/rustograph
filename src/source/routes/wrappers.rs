//! isthmus `http-wrappers` v1 선언 — 파싱과 동사·인자 바인딩(`wrapper.*` 규칙).
//!
//! 정본은 isthmus docs/HTTP-WRAPPERS.md다. 모르는 필드·잘못된 값은 선언 오류다 —
//! 낡은 선언을 조용히 무시하면 호출 0건이 "호출 없음"으로 읽힌다. 이 모듈은 syn을
//! 모른다. 추출기가 호출 인자를 [`CallArg`]로 바꿔 넘긴다.
//!
//! Rust 이름 규칙(`"language": "rust"` 항목만 적용):
//!
//! - `function`: `owner::name`이 rustograph 정점 ID다. 자유 함수는 모듈 경로
//!   (`app::net` + `send`), 메서드·연관 함수는 타입 ID(`app::api::ApiClient` +
//!   `request`), 트레이트 impl 메서드는 `app::api::ApiClient::<Trait>` + 이름.
//! - `constructor`: `owner`는 타입 ID다. `name`이 타입 이름과 같으면 구조체 리터럴
//!   (`Endpoint { method, path }` — `label`은 필드 이름)이나 튜플 구조체 생성
//!   (`Endpoint(m, p)` — `index`)이고, 다르면 연관 함수(`Endpoint::new`)다.
//! - 인자: Rust 함수에는 이름 붙은 인자가 없으므로 함수·메서드는 `index`(메서드는
//!   수신자를 빼고 0부터)로 묶는다. `label`은 구조체 리터럴 필드에만 맞는다.

use serde_json::Value;
use std::collections::BTreeMap;

/// 계약 동사.
pub const VERBS: &[&str] = &[
    "GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "TRACE",
];

/// 동사 인자·경로 인자 위치 지정.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArgSpec {
    pub index: Option<usize>,
    pub label: Option<String>,
}

/// 래퍼 선언의 종류.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WrapperKind {
    Constructor,
    Function,
}

/// 래퍼 선언 하나.
#[derive(Clone, Debug)]
pub struct Wrapper {
    /// 선언 파일의 `wrappers[n]` 위치 — 한계 문구에 쓴다.
    pub position: usize,
    pub language: String,
    pub kind: WrapperKind,
    pub owner: String,
    pub name: String,
    pub method_arg: Option<ArgSpec>,
    pub path_arg: ArgSpec,
    pub default_method: Option<String>,
    pub method_enum: BTreeMap<String, String>,
    /// `root`·`base`.
    pub path_anchor: String,
    pub service: Option<String>,
}

impl Wrapper {
    /// 동사 바인딩 명세.
    pub fn method_spec(&self) -> MethodSpec {
        MethodSpec {
            method_arg: self.method_arg.clone(),
            default_method: self.default_method.clone(),
            method_enum: self.method_enum.clone(),
        }
    }
}

/// 동사를 정하는 선언 부분 — 벡터(`wrapper.method`)가 이 형태만 준다.
#[derive(Clone, Debug, Default)]
pub struct MethodSpec {
    pub method_arg: Option<ArgSpec>,
    pub default_method: Option<String>,
    pub method_enum: BTreeMap<String, String>,
}

/// 호출 인자 값의 모양.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ArgValue {
    /// 문자열 리터럴(상수 치환 포함).
    Literal(String),
    /// enum case·연관 상수 경로의 마지막 이름(`Method::GET` → `GET`).
    EnumCase(String),
    /// 그 밖의 식.
    Other,
}

/// 호출 인자 하나 — 구조체 리터럴 필드면 `label`이 있다.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallArg {
    pub label: Option<String>,
    pub value: ArgValue,
}

/// 인자 찾기(`wrapper.method` 1): `label`이 같은 인자를 먼저, 없으면 `index` 위치의
/// 인자를 쓰되 그 인자가 다른 레이블을 달고 있으면 쓰지 않는다.
pub fn find_arg<'a>(spec: &ArgSpec, args: &'a [CallArg]) -> Option<&'a CallArg> {
    if let Some(label) = &spec.label {
        if let Some(a) = args.iter().find(|a| a.label.as_ref() == Some(label)) {
            return Some(a);
        }
    }
    let a = args.get(spec.index?)?;
    match &a.label {
        Some(l) if Some(l) != spec.label.as_ref() => None,
        _ => Some(a),
    }
}

/// 동사를 정한다 — None이면 `methodDynamic`이다(`wrapper.method` 2·3).
///
/// 인자가 없으면 `defaultMethod`, 인자가 있으면 정확한 대문자 동사 리터럴이나
/// `methodEnum`에 매핑된 enum case·리터럴만 동사다. 리터럴이 아닌 식은 기본값을
/// 쓰지 않는다.
pub fn bind_method(spec: &MethodSpec, args: &[CallArg]) -> Option<String> {
    let found = spec.method_arg.as_ref().and_then(|m| find_arg(m, args));
    let Some(arg) = found else {
        return spec.default_method.clone();
    };
    match &arg.value {
        ArgValue::Literal(s) if VERBS.contains(&s.as_str()) => Some(s.clone()),
        ArgValue::Literal(s) | ArgValue::EnumCase(s) => spec.method_enum.get(s).cloned(),
        ArgValue::Other => None,
    }
}

/// 선언 파일을 읽는다. 오류 문구는 원인과 고칠 자리를 담는다.
pub fn parse(text: &str) -> Result<Vec<Wrapper>, String> {
    let root: Value = serde_json::from_str(text)
        .map_err(|e| format!("the http-wrappers file is not valid JSON ({e})"))?;
    let obj = root
        .as_object()
        .ok_or("the http-wrappers file must be a JSON object")?;
    for key in obj.keys() {
        if !matches!(key.as_str(), "format" | "version" | "wrappers") {
            return Err(format!(
                "unknown field {key:?} in the http-wrappers file (allowed: format, version, wrappers)"
            ));
        }
    }
    if obj.get("format").and_then(Value::as_str) != Some("http-wrappers") {
        return Err("the http-wrappers file must declare \"format\": \"http-wrappers\"".into());
    }
    if obj.get("version").and_then(Value::as_u64) != Some(1) {
        return Err("only http-wrappers \"version\": 1 is supported".into());
    }
    let list = obj
        .get("wrappers")
        .and_then(Value::as_array)
        .ok_or("the http-wrappers file needs a \"wrappers\" array")?;
    list.iter()
        .enumerate()
        .map(|(i, w)| parse_wrapper(i, w).map_err(|e| format!("wrappers[{i}]: {e}")))
        .collect()
}

/// 선언 항목 하나를 읽는다.
fn parse_wrapper(position: usize, w: &Value) -> Result<Wrapper, String> {
    const FIELDS: &[&str] = &[
        "language",
        "kind",
        "owner",
        "name",
        "methodArg",
        "pathArg",
        "defaultMethod",
        "methodEnum",
        "pathAnchor",
        "service",
    ];
    let obj = w.as_object().ok_or("each wrapper must be a JSON object")?;
    if let Some(bad) = obj.keys().find(|k| !FIELDS.contains(&k.as_str())) {
        return Err(format!("unknown field {bad:?}"));
    }
    let text = |key: &str| -> Result<String, String> {
        obj.get(key)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .ok_or_else(|| format!("{key:?} must be a non-empty string"))
    };
    let kind = match text("kind")?.as_str() {
        "constructor" => WrapperKind::Constructor,
        "function" => WrapperKind::Function,
        other => {
            return Err(format!(
                "\"kind\" must be constructor or function, not {other:?}"
            ))
        }
    };
    let method_arg = obj.get("methodArg").map(arg_spec).transpose()?;
    let path_arg = arg_spec(obj.get("pathArg").ok_or("\"pathArg\" is required")?)?;
    let default_method = obj.get("defaultMethod").map(verb).transpose()?;
    if method_arg.is_none() && default_method.is_none() {
        return Err("declare \"methodArg\" or \"defaultMethod\" (a wrapper needs a verb)".into());
    }
    let method_enum = match obj.get("methodEnum") {
        None => BTreeMap::new(),
        Some(Value::Object(m)) => m
            .iter()
            .map(|(k, v)| Ok((k.clone(), verb(v)?)))
            .collect::<Result<_, String>>()?,
        Some(_) => return Err("\"methodEnum\" must be an object of case → verb".into()),
    };
    let path_anchor = text("pathAnchor")?;
    if path_anchor != "root" && path_anchor != "base" {
        return Err(format!(
            "\"pathAnchor\" must be root or base, not {path_anchor:?}"
        ));
    }
    let service = match obj.get("service") {
        None => None,
        Some(_) => Some(text("service")?),
    };
    Ok(Wrapper {
        position,
        language: text("language")?,
        kind,
        owner: text("owner")?,
        name: text("name")?,
        method_arg,
        path_arg,
        default_method,
        method_enum,
        path_anchor,
        service,
    })
}

/// `{ index }`·`{ label }`·둘 다.
fn arg_spec(v: &Value) -> Result<ArgSpec, String> {
    let obj = v
        .as_object()
        .ok_or("an argument spec must be an object with index and/or label")?;
    if let Some(bad) = obj.keys().find(|k| *k != "index" && *k != "label") {
        return Err(format!("unknown argument spec field {bad:?}"));
    }
    let index = match obj.get("index") {
        None => None,
        Some(i) => Some(
            i.as_u64()
                .ok_or("\"index\" must be a non-negative integer")? as usize,
        ),
    };
    let label = match obj.get("label") {
        None => None,
        Some(l) => Some(
            l.as_str()
                .filter(|s| !s.is_empty())
                .ok_or("\"label\" must be a non-empty string")?
                .to_string(),
        ),
    };
    if index.is_none() && label.is_none() {
        return Err("an argument spec needs index or label".into());
    }
    Ok(ArgSpec { index, label })
}

/// 계약 동사 문자열.
fn verb(v: &Value) -> Result<String, String> {
    match v.as_str() {
        Some(s) if VERBS.contains(&s) => Ok(s.to_string()),
        _ => Err(format!("{v} is not a contract verb ({})", VERBS.join(", "))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn declaration_errors_name_the_cause() {
        let bad = [
            (r#"[]"#, "JSON object"),
            (r#"{"format":"x","version":1,"wrappers":[]}"#, "format"),
            (
                r#"{"format":"http-wrappers","version":2,"wrappers":[]}"#,
                "version",
            ),
            (r#"{"format":"http-wrappers","version":1}"#, "wrappers"),
            (
                r#"{"format":"http-wrappers","version":1,"wrappers":[],"x":1}"#,
                "unknown field",
            ),
            (
                r#"{"format":"http-wrappers","version":1,"wrappers":[{"language":"rust","kind":"function","owner":"a","name":"b","pathArg":{"index":0},"pathAnchor":"root"}]}"#,
                "methodArg",
            ),
            (
                r#"{"format":"http-wrappers","version":1,"wrappers":[{"language":"rust","kind":"function","owner":"a","name":"b","pathArg":{},"defaultMethod":"GET","pathAnchor":"root"}]}"#,
                "index or label",
            ),
            (
                r#"{"format":"http-wrappers","version":1,"wrappers":[{"language":"rust","kind":"method","owner":"a","name":"b","pathArg":{"index":0},"defaultMethod":"GET","pathAnchor":"root"}]}"#,
                "kind",
            ),
            (
                r#"{"format":"http-wrappers","version":1,"wrappers":[{"language":"rust","kind":"function","owner":"a","name":"b","pathArg":{"index":0},"defaultMethod":"get","pathAnchor":"root"}]}"#,
                "contract verb",
            ),
            (
                r#"{"format":"http-wrappers","version":1,"wrappers":[{"language":"rust","kind":"function","owner":"a","name":"b","pathArg":{"index":0},"defaultMethod":"GET","pathAnchor":"top"}]}"#,
                "pathAnchor",
            ),
            (
                r#"{"format":"http-wrappers","version":1,"wrappers":[{"language":"rust","kind":"function","owner":"a","name":"b","pathArg":{"index":0},"defaultMethod":"GET","pathAnchor":"root","extra":1}]}"#,
                "unknown field",
            ),
        ];
        for (text, needle) in bad {
            let err = parse(text).expect_err(text);
            assert!(err.contains(needle), "{err} should mention {needle}");
        }
    }

    #[test]
    fn full_declaration_parses() {
        let text = r#"{"format":"http-wrappers","version":1,"wrappers":[{"language":"rust","kind":"constructor","owner":"app::Endpoint","name":"Endpoint","methodArg":{"index":0,"label":"method"},"pathArg":{"label":"path"},"methodEnum":{"Get":"GET"},"pathAnchor":"base","service":"api"}]}"#;
        let w = &parse(text).unwrap()[0];
        assert_eq!(w.kind, WrapperKind::Constructor);
        assert_eq!(w.method_enum.get("Get").map(String::as_str), Some("GET"));
        assert_eq!(w.service.as_deref(), Some("api"));
        assert_eq!(w.method_spec().method_arg.unwrap().index, Some(0));
    }
}
