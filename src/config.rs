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
    let src = read_bounded_text(path, MAX_FILE_BYTES)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    serde_yml::from_str(&src).map_err(|e| format!("bad {}: {e}", path.display()))
}

/// 설정·기준선 파일의 크기 상한(바이트). 기준선은 위반 하나가 한 줄이라 큰 레거시
/// 저장소도 수 MB 안이다 — 정상 파일은 넉넉히 덮되 장치를 끝없이 읽지 않게 한다.
/// 판단값이다(형식이 정한 한도가 아니다). 분석 대상 소스의 상한과 같은 64MiB다.
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// 설정·기준선 파일을 `max_bytes`까지만 읽는다 — 정규 파일만 연다. `.rustograph.yml`과
/// 그 `baseline:` 경로는 분석 대상 저장소가 정하므로 신뢰할 수 없다. FIFO는 여는 순간
/// 쓰는 쪽을 영원히 기다리고 장치(`/dev/zero`)는 끝없이 읽혀, 보지 않고 읽으면 rules·MCP가
/// 멈추거나 메모리를 다 쓴다. `metadata`는 stat이라 FIFO에서도 막히지 않는다.
/// 없으면 `NotFound` 그대로다(호출자가 "기준선 도입 전"으로 본다). 정규 파일이 아니면
/// `InvalidInput`, 상한을 넘으면 `InvalidData`이고 문구에 해결 방향을 싣는다 — 호출자가
/// 앞에 경로를 붙인다.
pub fn read_bounded_text(path: &Path, max_bytes: u64) -> std::io::Result<String> {
    use std::io::Read;
    let metadata = std::fs::metadata(path)?;
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "not a regular file (devices, FIFOs and directories are not read); \
             point it at a regular text file",
        ));
    }
    let too_large = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "larger than {max_bytes} bytes, the limit for rustograph configuration and \
                 baseline files; point it at a smaller regular text file"
            ),
        )
    };
    if metadata.len() > max_bytes {
        return Err(too_large());
    }
    let mut text = String::new();
    // stat 뒤에 파일이 자라도 상한 너머는 읽지 않는다.
    std::fs::File::open(path)?
        .take(max_bytes.saturating_add(1))
        .read_to_string(&mut text)?;
    if text.len() as u64 > max_bytes {
        return Err(too_large());
    }
    Ok(text)
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

    /// 상한 읽기는 정규 파일만 상한까지 읽는다 — 없으면 NotFound 그대로(기준선 도입 전
    /// 판정용), FIFO·장치·디렉터리는 열지 않으니 막히거나 끝없이 읽지 않는다.
    #[cfg(unix)]
    #[test]
    fn bounded_text_reads_only_regular_files_within_the_limit() {
        let dir = std::env::temp_dir().join(format!("rg-config-bounded-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let fits = dir.join("base.txt");
        std::fs::write(&fits, "allow|a|b|call\n").unwrap();
        assert_eq!(read_bounded_text(&fits, 15).unwrap(), "allow|a|b|call\n");
        let over = read_bounded_text(&fits, 14).unwrap_err();
        assert_eq!(over.kind(), std::io::ErrorKind::InvalidData);
        assert!(over.to_string().contains("larger than 14 bytes"), "{over}");
        let fifo = dir.join("pipe");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo must run");
        assert!(made.success());
        for special in [fifo.as_path(), Path::new("/dev/zero"), dir.as_path()] {
            let err = read_bounded_text(special, MAX_FILE_BYTES).unwrap_err();
            assert_eq!(
                err.kind(),
                std::io::ErrorKind::InvalidInput,
                "{}",
                special.display()
            );
            assert!(err.to_string().contains("not a regular file"), "{err}");
        }
        let missing = read_bounded_text(&dir.join("missing.txt"), MAX_FILE_BYTES).unwrap_err();
        assert_eq!(missing.kind(), std::io::ErrorKind::NotFound);
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
