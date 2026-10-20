//! SARIF 2.1.0 출력 — CI 코드 스캐닝(GitHub 등)이 소비하는 형식.
//!
//! 규칙 위반을 SARIF result로 옮긴다. 위치는 정점의 file:line을 워크스페이스 루트 상대 URI
//! (`uriBaseId: "%SRCROOT%"`, `/` 구분자, RFC 3986 퍼센트 인코딩)로 쓴다 — 올린 결과에 로컬 절대
//! 경로를 남기지 않는다. 루트 밖 파일은 위치 없이 그 사실을 message에 적고, 위치가 없으면 결과는
//! 메시지만 싣는다 — 없는 위치를 지어내지 않는다.

use crate::graph::{Document, Vertex};
use crate::rules::Violation;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// SARIF `artifactLocation.uriBaseId` — 위치 URI가 상대하는 기준(워크스페이스 루트)의 이름이다.
/// 코드 스캐닝 도구가 소스 루트에 관례로 쓰는 이름을 쓴다. 그 절대 경로는 `run.originalUriBaseIds`에
/// 싣지 않는다 — 로컬 경로를 결과에 남기지 않으려는 것이 상대 URI를 쓰는 이유다.
const SOURCE_ROOT_BASE_ID: &str = "%SRCROOT%";

/// 위치를 싣지 못한 결과의 message 꼬리 — 워크스페이스 루트 밖 파일(외부 크레이트, `#[path]`로
/// 벗어난 파일)은 루트 상대 URI로 쓸 수 없고, 절대 경로를 대신 싣지도 않는다. 위치를 모르는
/// 정점은 이 꼬리 없이 message만 싣는다.
const OUTSIDE_ROOT_NOTE: &str = " (location omitted: the file is outside the workspace root)";

/// 위반 목록을 SARIF 로그로 직렬화한다. `tool.driver.version`은 이 크레이트의 버전이다 —
/// `env!("CARGO_PKG_VERSION")`을 여기서 바로 읽는다(mcp의 serverInfo와 같은 방식). 출력 계층이
/// `crate::cli::VERSION`을 읽으면 cli ↔ sarif 모듈 순환인데, `json!` 매크로 토큰 안의 경로는
/// 수확되지 않아 자기 분석(`cycles --strict`)도 이 순환을 보지 못한다. 버전을 인자로 받지 않는
/// 것은 0.5.0 공개 시그니처(라이브러리 호출자)를 지키기 위해서다 — 드라이버 이름이 rustograph라
/// 버전도 늘 이 크레이트의 것이다. 위치 URI의 기준은 문서의 `workspace`(없으면 `root`)다.
pub fn rules_sarif(doc: &Document, violations: &[Violation]) -> String {
    // 위치 조회는 규칙 검사와 같은 색인(같은 ID의 첫 cfg 변형)을 한 번 만들어 쓴다 —
    // 위반마다 정점 배열을 훑으면 O(위반·V)다. 기준 루트도 한 번만 정한다.
    let by_id = crate::rules::vertices_by_id(doc);
    let base = SourceRoot::of(doc);
    let results: Vec<Value> = violations
        .iter()
        .map(|v| sarif_result(v, location(&by_id, &base, &v.from)))
        .collect();
    let log = json!({
        "version": "2.1.0",
        "$schema": "https://json.schemastore.org/sarif-2.1.0.json",
        "runs": [{
            "tool": {
                "driver": {
                    "name": "rustograph",
                    "version": env!("CARGO_PKG_VERSION"),
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

/// 위반 하나의 SARIF result — 루트 밖이라 위치를 뺀 결과는 그 사실을 message 끝에 적는다.
fn sarif_result(v: &Violation, placement: Placement) -> Value {
    let mut message = format!(
        "{} component '{}' may not depend on '{}' ({:?} edge: {} -> {})",
        v.rule, v.component, v.forbidden, v.kind, v.from, v.to
    );
    if matches!(placement, Placement::OutsideRoot) {
        message.push_str(OUTSIDE_ROOT_NOTE);
    }
    let mut result = Map::new();
    result.insert(
        "ruleId".to_string(),
        json!(format!("rustograph/{}", v.rule)),
    );
    result.insert(
        "level".to_string(),
        json!(if v.rule == "deny" { "error" } else { "warning" }),
    );
    result.insert("message".to_string(), json!(message));
    if let Placement::Located(loc) = placement {
        result.insert("locations".to_string(), json!([loc]));
    }
    Value::Object(result)
}

/// 위반 출발 정점의 SARIF 위치 판정.
enum Placement {
    /// 워크스페이스 루트 상대 위치.
    Located(Value),
    /// 위치는 있지만 루트 밖이라 싣지 않는다 — message가 그 사실을 적는다.
    OutsideRoot,
    /// 정점이나 위치가 없다 — 없는 위치를 지어내지 않는다.
    Unknown,
}

/// 정점의 `file:line` 위치를 워크스페이스 루트 상대 SARIF location으로 변환한다.
fn location(by_id: &BTreeMap<&str, &Vertex>, base: &SourceRoot, vertex_id: &str) -> Placement {
    let Some((file, line)) = by_id
        .get(vertex_id)
        .and_then(|v| v.position.as_deref())
        .and_then(|pos| pos.rsplit_once(':'))
    else {
        return Placement::Unknown;
    };
    let Some(uri) = base.relative_uri(file) else {
        return Placement::OutsideRoot;
    };
    let line = line.parse::<u32>().unwrap_or(1);
    Placement::Located(json!({
        "physicalLocation": {
            "artifactLocation": {"uri": uri, "uriBaseId": SOURCE_ROOT_BASE_ID},
            "region": {"startLine": line.max(1)}
        }
    }))
}

/// 위치 URI의 기준 루트 — 문서가 기록한 워크스페이스 루트, 없으면 분석 디렉터리(`root`)다.
struct SourceRoot {
    /// 기록된 표기와, 그와 다르면 이 머신에서 정규화한 표기를 어휘적으로 나눈 조각(이 순서로
    /// 견준다). 기록된 루트가 상대 경로(`--dir .`을 그대로 적은 root)면 비어 있다 — 문서를 만든
    /// 프로세스의 작업 디렉터리 기준이라 절대 위치와 견줄 수 없다.
    spellings: Vec<Vec<String>>,
}

impl SourceRoot {
    /// 문서의 기준 루트를 정한다. 정규화 표기를 더하는 이유: cargo는 심볼릭 링크를 지난 `--dir`의
    /// 워크스페이스 루트를 링크 표기 그대로 주지만 정점 위치는 정규화한 경로다(macOS의 `/tmp`·`/var`,
    /// Windows의 `\\?\` 접두). 이 머신에 없는 루트(다른 머신의 스냅샷)는 정규화하지 못해 기록된 표기만
    /// 쓴다 — 그때 떼지 못한 위치는 루트 밖으로 본다. 이 OS에서 절대 경로가 아닌 표기(유닉스에서 읽은
    /// Windows 스냅샷의 `C:\…`)는 작업 디렉터리 기준으로 풀리므로 정규화하지 않는다.
    fn of(doc: &Document) -> SourceRoot {
        let recorded = doc.workspace.as_deref().unwrap_or(&doc.root);
        if !is_absolute_text(recorded) {
            return SourceRoot { spellings: vec![] };
        }
        let mut spellings = vec![root_segments(recorded)];
        if !std::path::Path::new(recorded).is_absolute() {
            return SourceRoot { spellings };
        }
        // 정규화 실패(이 머신에 없는 루트, 읽을 수 없는 조상)와 UTF-8이 아닌 결과는 오류가 아니라
        // 예비 표기가 없다는 뜻이다 — 기록된 표기만으로 견준다.
        let real = std::fs::canonicalize(recorded).ok();
        if let Some(real) = real.as_deref().and_then(std::path::Path::to_str) {
            let real = root_segments(real);
            if !spellings.contains(&real) {
                spellings.push(real);
            }
        }
        SourceRoot { spellings }
    }

    /// 위치 파일을 루트 상대 URI로 바꾼다 — 루트 밖이거나 루트 자신이면 None이다. 상대 위치(다른
    /// 도구가 만든 문서)는 이미 루트 상대로 본다. 절대 위치는 루트 접두를 어휘적으로 뗀다 — 저장
    /// 스냅샷은 다른 머신의 경로일 수 있어 위치 파일을 이 머신에서 정규화하지 않는다.
    fn relative_uri(&self, file: &str) -> Option<String> {
        let absolute = is_absolute_text(file);
        let segments = normalized_segments(file, absolute)?;
        let inside = if absolute {
            let prefix = self.spellings.iter().find(|s| has_prefix(&segments, s))?;
            &segments[prefix.len()..]
        } else {
            &segments[..]
        };
        (!inside.is_empty()).then(|| encoded_uri(inside))
    }
}

/// 어휘적 절대 경로인가 — `/`·`\`로 시작하거나(유닉스, Windows의 UNC·`\\?\`) 드라이브 문자로
/// 시작한다. 다른 OS에서 저장한 스냅샷도 같은 규칙으로 읽으려고 `Path::is_absolute`를 쓰지 않는다.
fn is_absolute_text(path: &str) -> bool {
    let bytes = path.as_bytes();
    matches!(bytes.first(), Some(b'/' | b'\\'))
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
}

/// 경로를 어휘적으로 정규화한 조각 — `/`를 구분자로 보고, `/`로 시작하지 않는 경로(드라이브 문자나
/// `\`로 시작하는 Windows 경로, 다른 도구가 만든 상대 위치)에서는 `\`도 구분자로 본다. 유닉스 절대
/// 경로의 `\`는 파일 이름의 글자라 조각에 남긴다(`%5C`로 인코딩된다) — 구분자로 읽으면 uri가 다른
/// 파일을 가리킨다. 빈 조각·`.`은 버리고 `..`은 앞 조각을 지운다. 상대 경로가 `..`로 시작점 위로
/// 올라가면 None이다(절대 경로의 `/..`은 `/`다).
fn normalized_segments(path: &str, absolute: bool) -> Option<Vec<&str>> {
    let separators: &[char] = if path.starts_with('/') {
        &['/']
    } else {
        &['/', '\\']
    };
    let mut segments = Vec::new();
    for segment in path.split(separators) {
        match segment {
            "" | "." => {}
            ".." => {
                if segments.pop().is_none() && !absolute {
                    return None;
                }
            }
            name => segments.push(name),
        }
    }
    Some(segments)
}

/// 절대 루트 표기의 소유 조각 — 절대 경로로 정규화하면 `..`가 시작점 위로 올라가도 `/`에서
/// 멈춰 실패하지 않는다(그래서 None 갈래가 없다).
fn root_segments(path: &str) -> Vec<String> {
    normalized_segments(path, true)
        .unwrap_or_default()
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// `segments`가 루트 조각 `prefix`로 시작하는가 — 문자열이 아니라 조각 단위로 견준다
/// (`/work/space`는 `/work/spaces`의 접두가 아니다).
fn has_prefix(segments: &[&str], prefix: &[String]) -> bool {
    prefix.len() <= segments.len() && prefix.iter().zip(segments).all(|(p, s)| p == s)
}

/// 루트 상대 조각을 `/`로 이은 URI 참조 — 조각마다 RFC 3986 퍼센트 인코딩한다.
fn encoded_uri(segments: &[&str]) -> String {
    let mut uri = String::new();
    for (i, segment) in segments.iter().enumerate() {
        if i > 0 {
            uri.push('/');
        }
        percent_encode_into(segment, &mut uri);
    }
    uri
}

/// 경로 조각 하나를 RFC 3986으로 퍼센트 인코딩해 덧붙인다 — unreserved(`A-Z a-z 0-9 - . _ ~`)와
/// sub-delims(`! $ & ' ( ) * + , ; =`), `@`는 그대로 두고 나머지 바이트(비ASCII는 UTF-8 바이트)는
/// `%XX`(대문자 16진)다. `:`도 인코딩한다 — 상대 참조의 첫 조각에 든 `:`는 스킴으로 읽힌다
/// (RFC 3986 4.2). 조각 위치와 상관없이 규칙 하나를 쓴다.
fn percent_encode_into(segment: &str, out: &mut String) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &byte in segment.as_bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=@".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(byte >> 4)]));
            out.push(char::from(HEX[usize::from(byte & 0x0F)]));
        }
    }
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
        let s = rules_sarif(&doc, &[v]);
        let j: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(j["version"], "2.1.0");
        // 도구 버전은 이 크레이트의 버전이다 — sarif가 cli 상수를 읽지 않고 같은 값을 낸다.
        assert_eq!(
            j["runs"][0]["tool"]["driver"]["version"],
            env!("CARGO_PKG_VERSION")
        );
        assert_eq!(j["runs"][0]["results"][0]["ruleId"], "rustograph/deny");
        // 위치는 정점의 file:line에서 온다 — 상대 위치는 이미 루트 상대로 본다.
        let loc = &j["runs"][0]["results"][0]["locations"][0]["physicalLocation"];
        assert_eq!(loc["artifactLocation"]["uri"], "src/lib.rs");
        assert_eq!(loc["artifactLocation"]["uriBaseId"], "%SRCROOT%");
        assert_eq!(loc["region"]["startLine"], 10);
    }

    /// `root`·`workspace` 문서에 위치가 `position`인 정점 하나를 두고, 그 정점에서 나온 위반의
    /// SARIF 로그를 낸다.
    fn sarif_for(root: &str, workspace: Option<&str>, position: &str) -> Value {
        let doc = document(
            Level::Symbol,
            root.into(),
            workspace.map(str::to_string),
            vec![],
            vec![located("c::a", position, None)],
            vec![],
            vec![],
        );
        serde_json::from_str(&rules_sarif(&doc, &[violation("c::a")])).unwrap()
    }

    /// 워크스페이스가 `/work/space`인 문서에서 위치가 `position`인 위반의 uri.
    fn uri_at(position: &str) -> Value {
        let log = sarif_for(".", Some("/work/space"), position);
        log["runs"][0]["results"][0]["locations"][0]["physicalLocation"]["artifactLocation"]["uri"]
            .clone()
    }

    /// 수확한 정점 위치는 정규화한 절대 경로다 — 예전에는 그것을 uri에 그대로 실어, 코드 스캐닝에
    /// 올린 결과에 로컬 사용자 경로가 남았다. uri는 워크스페이스 루트 상대이고 기준은 `%SRCROOT%`이며,
    /// 루트의 절대 경로는 어디에도(`originalUriBaseIds` 포함) 싣지 않는다. 워크스페이스가 없는
    /// 문서는 root가 기준이고, 루트 끝의 구분자는 상관없다.
    #[test]
    fn absolute_positions_become_workspace_relative_uris() {
        let log = sarif_for(
            ".",
            Some("/work/space"),
            "/work/space/crates/core/src/lib.rs:7",
        );
        let run = &log["runs"][0];
        assert_eq!(
            place(&run["results"][0]),
            (json!("crates/core/src/lib.rs"), json!(7))
        );
        let artifact = &run["results"][0]["locations"][0]["physicalLocation"]["artifactLocation"];
        assert_eq!(artifact["uriBaseId"], "%SRCROOT%");
        assert!(run.get("originalUriBaseIds").is_none(), "{run}");
        assert!(!log.to_string().contains("/work"), "{log}");
        for (root, workspace) in [("/work/space", None), (".", Some("/work/space/"))] {
            let log = sarif_for(root, workspace, "/work/space/src/lib.rs:2");
            assert_eq!(
                place(&log["runs"][0]["results"][0]),
                (json!("src/lib.rs"), json!(2)),
                "{root} {workspace:?}"
            );
        }
    }

    /// uri는 RFC 3986 URI 참조다 — 경로 조각의 공백·`#`·`%`·`?`·`:`와 비ASCII(UTF-8 바이트)는 `%XX`로
    /// 쓰고, unreserved·sub-delims·`@`는 그대로 둔다. 인코딩하지 않으면 `#`·`?` 뒤가 fragment·query로
    /// 읽혀 파일 이름이 잘리고, 첫 조각의 `:`는 스킴으로 읽힌다.
    #[test]
    fn uri_segments_are_percent_encoded() {
        assert_eq!(
            uri_at("/work/space/src/a b#c%d/é:x?.rs:3"),
            "src/a%20b%23c%25d/%C3%A9%3Ax%3F.rs"
        );
        assert_eq!(
            uri_at("/work/space/src/keep-._~!$&'()*+,;=@.rs:1"),
            "src/keep-._~!$&'()*+,;=@.rs"
        );
        assert_eq!(uri_at("/work/space/a:b/lib.rs:1"), "a%3Ab/lib.rs");
    }

    /// `workspace` 문서에서 위치가 `position`인 위반이 `src/nested/lib.rs` 2행을 가리킨다.
    fn assert_nested_lib(workspace: Option<&str>, position: &str) {
        let log = sarif_for(".", workspace, position);
        assert_eq!(
            place(&log["runs"][0]["results"][0]),
            (json!("src/nested/lib.rs"), json!(2)),
            "{workspace:?} {position}"
        );
    }

    /// Windows 경로의 `\` 구분자는 `/`로 쓴다 — 다른 머신에서 저장한 스냅샷도 같은 규칙으로 읽는다.
    /// 예전에는 `C:\…`가 그대로 실려 `C:`가 URI 스킴으로 읽혔다.
    #[test]
    fn backslash_separators_become_slashes() {
        assert_nested_lib(Some(r"C:\work"), r"C:\work\src\nested\lib.rs:2");
        assert_nested_lib(Some(r"C:\work"), r"C:\work/src\nested/lib.rs:2");
        assert_nested_lib(None, r"src\nested\lib.rs:2");
    }

    /// 유닉스 절대 경로(`/` 시작)의 `\`는 구분자가 아니라 파일 이름의 글자다 — 조각에 남겨 `%5C`로
    /// 인코딩해야 그 파일을 가리킨다. 예전에는 `\`를 늘 구분자로 읽어 `#[path]`로 이은 `src/we\ird.rs`가
    /// 없는 파일 `src/we/ird.rs`를 가리켰다. 루트 표기도 같은 규칙이라 이름에 `\`가 든 루트의 접두가 맞는다.
    #[test]
    fn backslashes_in_unix_paths_stay_in_the_file_name() {
        assert_eq!(uri_at(r"/work/space/src/we\ird.rs:3"), "src/we%5Cird.rs");
        let log = sarif_for(".", Some(r"/work/sp\ace"), r"/work/sp\ace/src/lib.rs:2");
        assert_eq!(
            place(&log["runs"][0]["results"][0]),
            (json!("src/lib.rs"), json!(2))
        );
    }

    /// 루트 안에 머무는 `..`·`.`·겹친 구분자는 어휘적으로 접는다 — 위치 파일을 이 머신에서
    /// 정규화하지 않는다(저장 스냅샷은 다른 머신의 경로일 수 있다).
    #[test]
    fn dot_segments_inside_the_root_fold_lexically() {
        assert_eq!(uri_at("/work/space/src/../lib.rs:1"), "lib.rs");
        assert_eq!(uri_at("/work/space/./src//lib.rs:1"), "src/lib.rs");
    }

    /// 루트 밖 파일(외부 크레이트, `#[path]`로 벗어난 파일)은 루트 상대 URI가 없다 — 절대 경로를
    /// 대신 싣지 않고 location을 빼며 그 사실을 message에 적는다. `..`로 루트를 벗어난 위치, 루트
    /// 자신, 이름 앞부분만 같은 형제 디렉터리, 상대 root(`.`)만 있는 문서의 절대 위치, 시작점 위로
    /// 올라가는 상대 위치도 같다.
    #[test]
    fn files_outside_the_workspace_root_omit_the_location() {
        for (workspace, position) in [
            (Some("/work/space"), "/elsewhere/shared.rs:5"),
            (Some("/work/space"), "/work/space/src/../../shared.rs:1"),
            (Some("/work/space"), "/work/space:1"),
            (Some("/work/space"), "/work/spaces/src/lib.rs:1"),
            (None, "/work/space/src/lib.rs:1"),
            (Some("/work/space"), "../shared.rs:1"),
        ] {
            let log = sarif_for(".", workspace, position);
            let outcome = &log["runs"][0]["results"][0];
            assert!(outcome.get("locations").is_none(), "{position}: {outcome}");
            let text = outcome["message"].as_str().unwrap();
            assert!(
                text.ends_with(" (location omitted: the file is outside the workspace root)"),
                "{position}: {text}"
            );
            let rendered = log.to_string();
            assert!(
                !rendered.contains("shared") && !rendered.contains("/work"),
                "{position}: {rendered}"
            );
        }
    }

    /// 위치가 없는 정점과 정점이 아닌 출발점은 예전처럼 message만 싣는다 — 루트 밖 문구를
    /// 붙이지 않는다(위치를 몰라 생략한 것이지 루트 밖이라서가 아니다).
    #[test]
    fn missing_positions_add_no_outside_note() {
        let mut bare = located("c::a", "", None);
        bare.position = None;
        let doc = document(
            Level::Symbol,
            ".".into(),
            Some("/work/space".into()),
            vec![],
            vec![bare],
            vec![],
            vec![],
        );
        let s = rules_sarif(&doc, &[violation("c::a"), violation("c::gone")]);
        let j: Value = serde_json::from_str(&s).unwrap();
        for outcome in j["runs"][0]["results"].as_array().unwrap() {
            assert!(outcome.get("locations").is_none(), "{outcome}");
            assert!(!outcome["message"].as_str().unwrap().contains("omitted"));
        }
    }

    /// cargo는 심볼릭 링크를 지난 `--dir`의 워크스페이스 루트를 링크 표기 그대로 주지만, 정점 위치는
    /// 정규화한 경로다(macOS의 `/tmp`·`/var`도 링크다). 기록된 표기로 떼지 못하면 이 머신에서 정규화한
    /// 루트로 다시 뗀다 — 그러지 않으면 그런 실행의 위치가 전부 루트 밖으로 빠진다.
    #[cfg(unix)]
    #[test]
    fn symlinked_workspace_roots_match_canonical_positions() {
        let tmp = std::env::temp_dir().join(format!("rg-sarif-link-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let real = tmp.join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = tmp.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let position = format!("{}/src/lib.rs:4", real.canonicalize().unwrap().display());
        let log = sarif_for(".", Some(link.to_str().unwrap()), &position);
        std::fs::remove_dir_all(&tmp).ok();
        assert_eq!(
            place(&log["runs"][0]["results"][0]),
            (json!("src/lib.rs"), json!(4))
        );
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
        let j: Value = serde_json::from_str(&rules_sarif(&doc, &[violation("c::a")])).unwrap();
        assert_eq!(
            place(&j["runs"][0]["results"][0]),
            (json!("src/unix.rs"), json!(10))
        );
    }

    /// 위반마다 정점 배열을 처음부터 훑어 위치를 찾으면 O(위반·V)다 — 기준선을 만들기 전
    /// 위반이 17만 건인 합성 문서의 `--format sarif`가 22초였다. 위치 조회용 정점 색인을 정확히
    /// 한 번 만드는지(정점마다 한 번) 센다 — 상한이 아니라 정확한 횟수라서, 위반마다 색인을 다시
    /// 만들어도(넘침) 색인을 빼고 옛 선형 탐색으로 돌아가도(0) 실패한다.
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
        let s = rules_sarif(&doc, &violations);
        let examined = crate::rules::examined_vertices() - before;
        let j: Value = serde_json::from_str(&s).unwrap();
        let results = j["runs"][0]["results"].as_array().unwrap();
        assert_eq!(results.len(), n);
        assert_eq!(place(&results[0]), (json!("src/lib.rs"), json!(n)));
        assert_eq!(examined, n, "{examined} vertex visits for {n} vertices");
    }
}
