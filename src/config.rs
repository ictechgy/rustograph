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
            return load_file(&path).map(Some);
        }
    }
    Ok(None)
}

/// 설정 파일 하나를 읽어 파싱한다 — `rules --config`·MCP rules·디렉터리 탐색이 모두 쓴다.
/// YAML 파서 호출을 이 함수 하나에 두는 이유: 파서가 config 밖에서도 불리면 파서
/// 교체·보안 수정이 여러 모듈로 퍼지고, 외부 크레이트 사용은 `rules --strict`가 보지 못한다.
pub fn load_file(path: &Path) -> Result<Config, String> {
    let src = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_yml::from_str(&src).map_err(|e| format!("bad {}: {e}", path.display()))
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

    /// `--config FILE`·MCP·디렉터리 탐색이 모두 load_file 하나로 읽는다 — 오류는
    /// 경로와 원인을 싣고, 읽기 실패와 형식 오류를 구분한다.
    #[test]
    fn load_file_parses_one_config_and_names_the_failure() {
        let dir = std::env::temp_dir().join(format!("rg-config-load-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("rules.yml");
        std::fs::write(
            &good,
            "components:\n  core: [\"c::core/**\"]\nbaseline: base.txt\n",
        )
        .unwrap();
        let cfg = load_file(&good).unwrap();
        assert_eq!(cfg.components["core"], vec!["c::core/**"]);
        assert_eq!(cfg.baseline.as_deref(), Some("base.txt"));
        let bad = dir.join("bad.yml");
        std::fs::write(&bad, "components: [unclosed\n").unwrap();
        let err = load_file(&bad).unwrap_err();
        assert!(
            err.starts_with(&format!("bad {}: ", bad.display())),
            "{err}"
        );
        let missing = dir.join("missing.yml");
        let err = load_file(&missing).unwrap_err();
        assert!(
            err.starts_with(&format!("cannot read {}: ", missing.display())),
            "{err}"
        );
        // 디렉터리 탐색도 같은 경로로 읽는다.
        assert!(load(&dir).unwrap().is_none());
        std::fs::copy(&good, dir.join(".rustograph.yml")).unwrap();
        let found = load(&dir).unwrap().unwrap();
        assert_eq!(found.baseline.as_deref(), Some("base.txt"));
        std::fs::remove_dir_all(&dir).ok();
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
