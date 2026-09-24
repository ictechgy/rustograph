//! `.rustograph.yml` — 컴포넌트 매핑과 의존 규칙.
//!
//! serde_yml은 이 모듈에만 격리한다 — 도메인이 설정 형식을 모르게.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

/// 규칙 파일. `components`는 경로 패턴 → 컴포넌트, `deps`는 허용 목록,
/// `deny`는 무조건 금지(deps를 이긴다), `signature`는 공개 API 타입 누출 제한.
/// `baseline`은 기존 위반을 얼린 파일의 경로 — 설정 파일 기준 상대 경로다.
#[derive(Debug, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub components: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub deps: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub deny: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub signature: BTreeMap<String, Vec<String>>,
    /// 기준선 파일 경로 — 레거시 도입 시 기존 위반을 얼려 새 위반만
    /// 실패하게 하는 장치. 파일이 없으면 조용히 무시한다.
    #[serde(default)]
    pub baseline: Option<String>,
}

/// 설정 파일을 읽는다. 파일이 없으면 None — 규칙 없는 프로젝트는 오류가 아니다.
pub fn load(dir: &Path) -> Result<Option<Config>, String> {
    for name in [".rustograph.yml", ".rustograph.yaml"] {
        let path = dir.join(name);
        if path.exists() {
            let src = std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
            let cfg: Config =
                serde_yml::from_str(&src).map_err(|e| format!("bad {}: {e}", path.display()))?;
            return Ok(Some(cfg));
        }
    }
    Ok(None)
}

/// 패턴 매칭 — 정확 일치, `x/**` 재귀 접두사, `*` 한 세그먼트 글롭.
/// 긴 패턴이 이긴다(호출자가 정렬해 쓴다).
pub fn matches(pattern: &str, path: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix("/**") {
        return path == prefix || path.starts_with(&format!("{prefix}::"));
    }
    if !pattern.contains('*') {
        return pattern == path;
    }
    let pp: Vec<&str> = pattern.split("::").collect();
    let tp: Vec<&str> = path.split("::").collect();
    if pp.len() != tp.len() {
        return false;
    }
    pp.iter().zip(tp.iter()).all(|(p, t)| *p == "*" || p == t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_full_config() {
        let src = r#"
components:
  core: ["my::core*"]
  ui: ["my::ui*"]
deps:
  ui: [core]
  core: []
deny:
  core: [ui]
signature:
  ui: [ui, core]
"#;
        let c: Config = serde_yml::from_str(src).unwrap();
        assert_eq!(c.components.len(), 2);
        assert_eq!(c.deps["ui"], vec!["core"]);
        assert_eq!(c.deny["core"], vec!["ui"]);
    }

    #[test]
    fn matches_glob_last_segment() {
        assert!(matches("a::b::*", "a::b::c"));
        assert!(matches("a::*", "a::b"));
        assert!(!matches("a::*", "a::b::c")); // 길이가 다르면 불일치.
        assert!(matches("a::b::c", "a::b::c"));
        assert!(!matches("a::b", "a::b::c"));
    }
}
