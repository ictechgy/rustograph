//! `.rustograph.yml` — 컴포넌트 매핑과 의존 규칙.
//!
//! YAML 파서(serde_norway)는 이 모듈에만 격리한다 — 도메인이 설정 형식을 모르게.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

/// 규칙 파일. `components`는 경로 패턴 → 컴포넌트, `deps`는 허용 목록,
/// `deny`는 무조건 금지(deps를 이긴다), `signature`는 공개 API 타입 누출 제한.
/// `baseline`은 기존 위반을 얼린 파일의 경로 — 설정 파일 기준 상대 경로이고 `--dir` 안이어야
/// 한다([`resolve_baseline`]).
/// 모르는 키는 거부한다 — 필드가 전부 선택이라, 받아 주면 `components:`를 `component:`로
/// 잘못 쓴 설정 하나로 규칙이 통째로 꺼진 채 `rules --strict`가 0으로 통과한다.
/// [`load_file`]은 규칙 키가 정의된 컴포넌트를 가리키는지도 본다.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
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
    /// 실패하게 하는 장치. 파일이 없으면 조용히 무시한다. 분석 대상 저장소가 고른 값이라
    /// 읽고 쓰기 전에 [`resolve_baseline`]으로 `--dir` 안에 가둔다.
    #[serde(default)]
    pub baseline: Option<String>,
}

/// 설정 파일을 읽는다. 파일이 없으면 None — 규칙 없는 프로젝트는 오류가 아니다.
/// 디렉터리에서 찾은 파일은 분석 대상 저장소가 둔 것이라 정규 파일만 연다.
pub fn load(dir: &Path) -> Result<Option<Config>, String> {
    for name in [".rustograph.yml", ".rustograph.yaml"] {
        let path = dir.join(name);
        if path.exists() {
            return load_file(&path, FileKinds::RegularOnly).map(Some);
        }
    }
    Ok(None)
}

/// 설정 파일 하나를 읽어 파싱하고 검증한다 — `rules --config`·MCP rules·디렉터리 탐색이 모두 쓴다.
/// `kinds`는 경로를 누가 골랐고 몇 번 읽는지에 따라 호출자가 정한다([`FileKinds`]).
/// YAML 파서 호출을 이 함수 하나에 두는 이유: 파서가 config 밖에서도 불리면 파서
/// 교체·보안 수정이 여러 모듈로 퍼지고, 외부 크레이트 사용은 `rules --strict`가 보지 못한다.
/// 검증도 여기서 한다 — CLI(종료 2)와 MCP(isError)가 같은 설정을 같은 이유로 거부한다.
pub fn load_file(path: &Path, kinds: FileKinds) -> Result<Config, String> {
    let src = read_bounded_text(path, MAX_FILE_BYTES, kinds)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let parsed: Config = serde_norway::from_str(&src).map_err(|e| {
        format!(
            "bad {}: {}",
            path.display(),
            escape_control_chars(&e.to_string())
        )
    })?;
    check_components(&parsed).map_err(|e| format!("bad {}: {e}", path.display()))?;
    Ok(parsed)
}

/// 규칙이 정의된 컴포넌트에만 걸리는지 본다. 컴포넌트가 하나도 없으면 모든 모듈이 unmapped라
/// 어떤 규칙도 걸리지 않는다(빈 파일도 그렇다). `deps`·`deny`·`signature`의 키가 `components`에
/// 없는 이름이면 그 규칙은 어떤 간선에도 걸리지 않는다 — 오타 하나가 `deny`·`signature` 규칙을
/// 조용히 끈다. 값은 컴포넌트 이름이나 모듈 경로 패턴이라([`matches`]) 키만 엄격히 본다.
/// 이름은 `{:?}`로 실어 설정이 고른 제어 문자가 터미널에 닿지 않게 한다.
fn check_components(cfg: &Config) -> Result<(), String> {
    if cfg.components.is_empty() {
        return Err(
            "components defines no component, so every module is unmapped and no rule can \
             apply; map at least one component to module patterns under components"
                .to_string(),
        );
    }
    for (section, keyed) in [
        ("deps", &cfg.deps),
        ("deny", &cfg.deny),
        ("signature", &cfg.signature),
    ] {
        if let Some(name) = keyed.keys().find(|n| !cfg.components.contains_key(*n)) {
            return Err(format!(
                "{section} has rules for component {name:?}, which components does not define; \
                 add it under components or fix the name"
            ));
        }
    }
    Ok(())
}

/// 파서 오류 문구의 제어 문자(C0·DEL·C1)와 U+2028·U+2029를 `\u{..}`로 바꾼다 — 모르는 키 오류는
/// 설정이 고른 키 이름을 그대로 싣는데, 그 설정은 분석 대상 저장소에서 온다. 그대로 stderr에
/// 쓰면 터미널 제어 문자가 사용자 터미널을 조작한다(저장 문서 ID에 같은 문자를 거부하는 이유와 같다).
fn escape_control_chars(message: &str) -> String {
    let mut escaped = String::with_capacity(message.len());
    for c in message.chars() {
        if c.is_control() || c == '\u{2028}' || c == '\u{2029}' {
            escaped.extend(c.escape_unicode());
        } else {
            escaped.push(c);
        }
    }
    escaped
}

/// 설정의 `baseline:` 값을 읽고 쓸 경로로 푼다 — `cfg_dir`(설정 파일의 디렉터리) 기준 상대 경로이고,
/// 푼 결과는 `root`(`--dir` 디렉터리) 안이어야 한다. 값은 분석 대상 저장소가 고른다:
/// `rules --write-baseline`이 이 경로에 중간 디렉터리까지 만들어 쓰므로, 가두지 않으면 기여자 PR이
/// 바꾼 설정 한 줄이 메인테이너의 저장소 밖 파일을 기준선 텍스트로 덮어쓴다. 명시 `--baseline FILE`은
/// 실행한 사람이 고른 입력이라 이 함수를 거치지 않는다(호출자 몫).
///
/// 거부하는 것: 절대 경로와 루트 접두(Windows 드라이브·`\` 시작 포함), `.`·`..`를 어휘적으로 푼 결과가
/// `root` 밖인 경로, 존재하는 가장 가까운 조상이 심볼릭 링크를 따라 `root` 밖으로 나가는 경로(풀리지
/// 않는 링크 포함). 루트 안의 `..`는 받는다 — 모노레포 하위 설정 `sub/.rustograph.yml`의 `../base.txt`.
/// 기준은 cargo 워크스페이스 루트가 아니라 `--dir`이다: `--graph`에서는 cargo를 돌리지 않고, 스냅샷이
/// 적은 워크스페이스는 신뢰할 수 없는 값이다.
///
/// 돌려주는 경로: `..`가 없으면 `cfg_dir`에 그대로 이은 경로다(이전과 같은 표기 — 이때는 OS가 푸는
/// 경로가 검사한 경로와 같다). `..`가 있으면 어휘적으로 푼 정규 경로다 — OS는 `link/..`를 링크 대상의
/// 부모로 풀어 어휘 검사와 어긋나므로, 검사한 경로 그대로 읽고 쓰게 한다. 오류는 원인과 해결 방향을
/// 싣고, 설정에서 온 값은 `{:?}`로 실어 제어 문자가 터미널에 닿지 않게 한다.
pub fn resolve_baseline(cfg_dir: &Path, root: &Path, raw: &str) -> Result<PathBuf, String> {
    let relative = Path::new(raw);
    let refuse = |cause: String| {
        format!(
            "baseline {raw:?} from the rules file {cause}; write it relative to the rules file \
             and keep it inside the --dir directory"
        )
    };
    let (target, climbs) = lexical_target(cfg_dir, relative).map_err(refuse)?;
    let real_root = root.canonicalize().map_err(|e| {
        refuse(format!(
            "cannot be checked: the --dir directory {} cannot be resolved ({e})",
            root.display()
        ))
    })?;
    if !target.starts_with(&real_root) {
        return Err(refuse(format!(
            "resolves to {target:?}, outside the --dir directory {}",
            real_root.display()
        )));
    }
    linked_ancestor_within(&target, &real_root).map_err(refuse)?;
    Ok(if climbs {
        target
    } else {
        cfg_dir.join(relative)
    })
}

/// `cfg_dir`의 정규 경로에 `relative`의 구성 요소를 어휘적으로 잇는다 — `.`는 건너뛰고 `..`는 한 단계
/// 올라간다(심볼릭 링크를 따라가지 않는다). 정규 경로에서 시작하므로 결과의 앞부분에는 링크가 없다.
/// `..`를 하나라도 만났는지 함께 돌려준다. 접두·루트 구성 요소(절대 경로, Windows 드라이브와 `\`
/// 시작)는 거부한다. 빈 `cfg_dir`(경로 없는 `--config rules.yml`의 부모)은 현재 디렉터리다.
fn lexical_target(cfg_dir: &Path, relative: &Path) -> Result<(PathBuf, bool), String> {
    let start = if cfg_dir.as_os_str().is_empty() {
        Path::new(".")
    } else {
        cfg_dir
    };
    let mut target = start.canonicalize().map_err(|e| {
        format!(
            "cannot be checked: the rules file's directory {} cannot be resolved ({e})",
            start.display()
        )
    })?;
    let mut climbs = false;
    for part in relative.components() {
        match part {
            Component::Normal(name) => target.push(name),
            Component::ParentDir => {
                climbs = true;
                target.pop();
            }
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir => {
                return Err("is an absolute path".to_string())
            }
        }
    }
    Ok((target, climbs))
}

/// `target`이나 그 조상 가운데 존재하는 가장 가까운 것을 끝까지 풀어 `real_root` 안인지 본다 — 어휘적으로
/// 루트 안이어도 저장소 안의 심볼릭 링크(디렉터리나 파일)가 밖을 가리키면 읽기·쓰기가 밖에 닿는다.
/// 링크인데 풀리지 않으면(대상 없는 링크) 나중에 무엇을 가리킬지 몰라 거부한다. 재귀 대신 반복으로
/// 올라간다 — `target`은 `real_root`로 시작하니 늦어도 `real_root`에서 멈춘다.
fn linked_ancestor_within(target: &Path, real_root: &Path) -> Result<(), String> {
    let mut existing = target;
    loop {
        match std::fs::symlink_metadata(existing) {
            Ok(_) => break,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                existing = existing
                    .parent()
                    .ok_or_else(|| format!("has no existing directory above {target:?}"))?;
            }
            Err(e) => return Err(format!("cannot be inspected at {existing:?} ({e})")),
        }
    }
    let resolved = existing.canonicalize().map_err(|e| {
        format!("leads through {existing:?}, a symbolic link that cannot be resolved ({e})")
    })?;
    if resolved.starts_with(real_root) {
        Ok(())
    } else {
        Err(format!(
            "leads through {existing:?} to {resolved:?}, outside the --dir directory {}",
            real_root.display()
        ))
    }
}

/// 설정·기준선 파일의 크기 상한(바이트). 기준선은 위반 하나가 한 줄이라 큰 레거시
/// 저장소도 수 MB 안이다 — 정상 파일은 넉넉히 덮되 장치를 끝없이 읽지 않게 한다.
/// 판단값이다(형식이 정한 한도가 아니다). 분석 대상 소스의 상한과 같은 64MiB다.
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// 설정·기준선 읽기가 여는 파일 종류 — 경로를 누가 골랐고 몇 번 읽는지에 따라 호출자가 정한다.
/// 크기 상한([`MAX_FILE_BYTES`] 등)은 어느 쪽이든 건다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKinds {
    /// 정규 파일만 연다. 분석 대상 저장소가 고른 경로(디렉터리에서 찾은 `.rustograph.yml`과
    /// 설정의 `baseline:`)는 신뢰할 수 없다 — FIFO는 여는 순간 쓰는 쪽을 영원히 기다리고 장치는
    /// 끝없이 읽힌다. 같은 경로를 호출마다 다시 읽는 곳(MCP rules)도 이쪽이다 — 한 번만 읽히는
    /// 파이프는 두 번째 호출에서 멈추거나 빈 내용을 읽는다.
    RegularOnly,
    /// 명령을 실행한 사람이 인자로 한 번 읽으라고 준 경로(`rules --config`·`--baseline`)다 —
    /// 파이프와 프로세스 치환(`<(…)`, `/dev/stdin`)도 읽는다. 셸에서 흔한 쓰임이고, 사용자가
    /// 고른 입력이라 저장소가 고르는 경로의 위협 모델 밖이다. 장치를 주어도 상한에서 끝난다.
    AnyReadable,
}

/// 설정·기준선 파일을 `max_bytes`까지만 읽는다. [`FileKinds::RegularOnly`]면 정규 파일만
/// 연다 — `metadata`는 stat이라 FIFO에서도 막히지 않는다. [`FileKinds::AnyReadable`]이면
/// 종류를 보지 않고 열어 상한까지만 읽는다.
/// 없으면 `NotFound` 그대로다(호출자가 "기준선 도입 전"으로 본다). 정규 파일이 아니면
/// `InvalidInput`, 상한을 넘으면 `InvalidData`이고 문구에 해결 방향을 싣는다 — 호출자가
/// 앞에 경로를 붙인다.
pub fn read_bounded_text(path: &Path, max_bytes: u64, kinds: FileKinds) -> std::io::Result<String> {
    use std::io::Read;
    let too_large = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "larger than {max_bytes} bytes, the limit for rustograph configuration and \
                 baseline files; point it at a smaller regular text file"
            ),
        )
    };
    if kinds == FileKinds::RegularOnly {
        let metadata = std::fs::metadata(path)?;
        if !metadata.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "not a regular file (devices, FIFOs and directories are not read); \
                 point it at a regular text file",
            ));
        }
        if metadata.len() > max_bytes {
            return Err(too_large());
        }
    }
    let mut text = String::new();
    // stat 뒤에 파일이 자라거나 파이프·장치라 길이를 모르더라도 상한 너머는 읽지 않는다.
    std::fs::File::open(path)?
        .take(max_bytes.saturating_add(1))
        .read_to_string(&mut text)?;
    if text.len() as u64 > max_bytes {
        return Err(too_large());
    }
    Ok(text)
}

/// 패턴 매칭 — 정확 일치, `x/**` 재귀 접두사, 세그먼트 안의 `*`·`?` 글롭.
/// 긴 패턴이 이긴다(호출자가 정렬해 쓴다).
pub fn matches(pattern: &str, path: &str) -> bool {
    let recursive = pattern.ends_with("/**");
    let pp: Vec<&str> = pattern
        .strip_suffix("/**")
        .unwrap_or(pattern)
        .split("::")
        .collect();
    let tp: Vec<&str> = path.split("::").collect();
    if pp.len() != tp.len() && !(recursive && pp.len() < tp.len()) {
        return false;
    }
    pp.iter().zip(tp.iter()).all(|(p, t)| matches_segment(p, t))
}

/// 별표의 마지막 위치로만 되돌아가며 유니코드 문자 단위로 한 세그먼트를 맞춘다.
fn matches_segment(pattern: &str, value: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let value: Vec<char> = value.chars().collect();
    let (mut p, mut v, mut retry) = (0, 0, 0);
    let mut star = None;
    while v < value.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == value[v]) {
            p += 1;
            v += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            p += 1;
            retry = v;
        } else if let Some(index) = star {
            p = index + 1;
            retry += 1;
            v = retry;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
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
        let c: Config = serde_norway::from_str(src).unwrap();
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
        let cfg = load_file(&good, FileKinds::RegularOnly).unwrap();
        assert_eq!(cfg.components["core"], vec!["c::core/**"]);
        assert_eq!(cfg.baseline.as_deref(), Some("base.txt"));
        let bad = dir.join("bad.yml");
        std::fs::write(&bad, "components: [unclosed\n").unwrap();
        let err = load_file(&bad, FileKinds::AnyReadable).unwrap_err();
        assert!(
            err.starts_with(&format!("bad {}: ", bad.display())),
            "{err}"
        );
        let missing = dir.join("missing.yml");
        let err = load_file(&missing, FileKinds::AnyReadable).unwrap_err();
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

    /// 설정 실수가 규칙을 조용히 끄지 못한다 — 모든 필드가 선택이라 예전에는 `components:`를
    /// `component:`로 잘못 쓴 설정 하나로 모든 모듈이 unmapped가 되고 `rules --strict`가 0 위반·
    /// 종료 0으로 통과했다. `signature`·`deny` 키 오타나 정의되지 않은 컴포넌트의 규칙은 그 규칙만
    /// 사라졌다. 모르는 키, 정의되지 않은 컴포넌트를 가리키는 규칙 키, 컴포넌트가 하나도 없는
    /// 설정(빈 파일 포함)은 경로·원인·해결 방향을 담은 오류다 — 디렉터리에서 찾은 설정도 같다.
    #[test]
    fn load_file_rejects_configs_that_would_silently_disable_rules() {
        let dir = std::env::temp_dir().join(format!("rg-config-strict-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let app = "components:\n  app: [\"c::app/**\"]\n";
        for (text, cause, direction) in [
            (
                "component:\n  app: [\"c::app/**\"]\ndeny:\n  app: [core]\n".to_string(),
                "unknown field `component`",
                "expected one of `components`",
            ),
            (
                format!("{app}signatures:\n  app: [app]\n"),
                "unknown field `signatures`",
                "expected one of",
            ),
            (
                format!("{app}deny:\n  nope: [app]\n"),
                "deny has rules for component \"nope\", which components does not define",
                "add it under components or fix the name",
            ),
            (
                format!("{app}deps:\n  ap: []\n"),
                "deps has rules for component \"ap\"",
                "add it under components or fix the name",
            ),
            (
                format!("{app}signature:\n  core: [app]\n"),
                "signature has rules for component \"core\"",
                "add it under components or fix the name",
            ),
            (
                "components: {}\ndeps: {}\n".to_string(),
                "components defines no component",
                "map at least one component",
            ),
            (
                String::new(),
                "components defines no component",
                "map at least one component",
            ),
        ] {
            let path = dir.join("rules.yml");
            std::fs::write(&path, &text).unwrap();
            let err = load_file(&path, FileKinds::RegularOnly).unwrap_err();
            assert!(
                err.starts_with(&format!("bad {}: ", path.display())),
                "{text:?}: {err}"
            );
            assert!(
                err.contains(cause) && err.contains(direction),
                "{text:?}: {err}"
            );
            std::fs::rename(&path, dir.join(".rustograph.yml")).unwrap();
            let found = load(&dir).unwrap_err();
            assert!(found.contains(cause), "{text:?}: {found}");
            std::fs::remove_file(dir.join(".rustograph.yml")).unwrap();
        }
        // 모든 규칙이 정의된 컴포넌트를 가리키면 그대로 읽는다(값은 컴포넌트 이름이나 패턴이라 보지 않는다).
        let good = dir.join("good.yml");
        std::fs::write(
            &good,
            format!("{app}  core: [\"c::core/**\"]\ndeps:\n  app: [core, \"c::util/**\"]\ndeny:\n  core: [app]\nsignature:\n  app: [app, core]\n"),
        )
        .unwrap();
        let cfg = load_file(&good, FileKinds::RegularOnly).unwrap();
        assert_eq!(cfg.deps["app"], vec!["core", "c::util/**"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 설정 오류 문구에 실리는 키 이름은 분석 대상 저장소가 고른 문자열이다 — 제어 문자를 그대로
    /// 실으면 `rules`의 stderr가 사용자 터미널을 조작한다. 이스케이프해서 싣는다.
    #[test]
    fn config_errors_escape_control_characters_from_the_file() {
        let dir = std::env::temp_dir().join(format!("rg-config-escape-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        for text in [
            "\"\\e[2Jcomponents\":\n  app: [\"c::app/**\"]\n",
            "components:\n  app: [\"c::app/**\"]\ndeny:\n  \"\\e[2Japp\": [app]\n",
            "components:\n  app: [\"c::app/**\"]\n\"\\L\": 1\n",
        ] {
            let path = dir.join("rules.yml");
            std::fs::write(&path, text).unwrap();
            let err = load_file(&path, FileKinds::RegularOnly).unwrap_err();
            assert!(
                !err.contains('\u{1b}') && !err.contains('\u{2028}'),
                "{text:?}: {err:?}"
            );
            assert!(
                err.contains("\\u{1b}[2J") || err.contains("\\u{2028}"),
                "{text:?}: {err:?}"
            );
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 정규 파일만 읽는 쪽은 상한까지만 읽는다 — 없으면 NotFound 그대로(기준선 도입 전
    /// 판정용), FIFO·장치·디렉터리는 열지 않으니 막히거나 끝없이 읽지 않는다. 명시 인자 쪽은
    /// 파이프를 읽고, 장치를 주어도 상한에서 끝난다.
    #[cfg(unix)]
    #[test]
    fn bounded_text_reads_only_regular_files_within_the_limit() {
        let dir = std::env::temp_dir().join(format!("rg-config-bounded-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let fits = dir.join("base.txt");
        std::fs::write(&fits, "allow|a|b|call\n").unwrap();
        for kinds in [FileKinds::RegularOnly, FileKinds::AnyReadable] {
            assert_eq!(
                read_bounded_text(&fits, 15, kinds).unwrap(),
                "allow|a|b|call\n"
            );
            let over = read_bounded_text(&fits, 14, kinds).unwrap_err();
            assert_eq!(over.kind(), std::io::ErrorKind::InvalidData);
            assert!(over.to_string().contains("larger than 14 bytes"), "{over}");
            let missing =
                read_bounded_text(&dir.join("missing.txt"), MAX_FILE_BYTES, kinds).unwrap_err();
            assert_eq!(missing.kind(), std::io::ErrorKind::NotFound);
        }
        let fifo = dir.join("pipe");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .expect("mkfifo must run");
        assert!(made.success());
        for special in [fifo.as_path(), Path::new("/dev/zero"), dir.as_path()] {
            let err =
                read_bounded_text(special, MAX_FILE_BYTES, FileKinds::RegularOnly).unwrap_err();
            assert_eq!(
                err.kind(),
                std::io::ErrorKind::InvalidInput,
                "{}",
                special.display()
            );
            assert!(err.to_string().contains("not a regular file"), "{err}");
        }
        // 명시 인자 쪽: 쓰는 쪽이 있는 FIFO(셸의 `<(…)`와 같다)는 끝까지 읽는다. 쓰는 쪽은
        // 읽는 쪽이 열 때까지 open에서 기다린다.
        let writer_path = fifo.clone();
        let writer = std::thread::spawn(move || {
            use std::io::Write;
            std::fs::OpenOptions::new()
                .write(true)
                .open(&writer_path)
                .and_then(|mut f| f.write_all(b"allow|a|b|call\n"))
        });
        assert_eq!(
            read_bounded_text(&fifo, MAX_FILE_BYTES, FileKinds::AnyReadable).unwrap(),
            "allow|a|b|call\n"
        );
        writer
            .join()
            .expect("writer thread must not panic")
            .expect("writing into the FIFO must succeed");
        // 장치는 끝없이 읽지 않고 상한에서 InvalidData로 끝난다.
        let zero =
            read_bounded_text(Path::new("/dev/zero"), 4096, FileKinds::AnyReadable).unwrap_err();
        assert_eq!(zero.kind(), std::io::ErrorKind::InvalidData);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// 설정의 `baseline:`은 설정 파일 디렉터리 기준이고 `--dir` 안이어야 한다 — 분석 대상 저장소가
    /// 고른 값이라, 가두지 않으면 `--write-baseline`이 저장소 밖 파일을 덮어쓰고 중간 디렉터리까지
    /// 만든다. 루트 안의 `..`(모노레포 하위 설정의 `../base.txt`)는 받는다. `..`가 없으면 이전과 같은
    /// 표기를, 있으면 정규화한 경로를 돌려준다. 루트 밖 설정의 상대 경로도 루트 밖이면 거부한다.
    #[test]
    fn resolve_baseline_keeps_config_paths_inside_the_root() {
        let tmp = std::env::temp_dir().join(format!("rg-config-baseline-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let (root, outside) = (tmp.join("root"), tmp.join("outside"));
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let real_root = root.canonicalize().unwrap();
        assert_eq!(
            resolve_baseline(&root, &root, ".rustograph/rules-baseline.txt").unwrap(),
            root.join(".rustograph/rules-baseline.txt")
        );
        assert_eq!(
            resolve_baseline(Path::new(""), Path::new("."), "base.txt").unwrap(),
            PathBuf::from("base.txt")
        );
        assert_eq!(
            resolve_baseline(&root.join("sub"), &root, "../base.txt").unwrap(),
            real_root.join("base.txt")
        );
        assert_eq!(
            resolve_baseline(&root, &root, "sub/./../sub/base.txt").unwrap(),
            real_root.join("sub").join("base.txt")
        );
        let absolute = outside.join("absolute.txt").display().to_string();
        for (cfg_dir, raw, cause) in [
            (root.clone(), absolute.as_str(), "is an absolute path"),
            (
                root.clone(),
                "../outside/new/deep/x.txt",
                "outside the --dir directory",
            ),
            (
                root.join("sub"),
                "../../outside/x.txt",
                "outside the --dir directory",
            ),
            (
                root.clone(),
                "sub/../../root-sibling.txt",
                "outside the --dir directory",
            ),
            (outside.clone(), "base.txt", "outside the --dir directory"),
        ] {
            let err = resolve_baseline(&cfg_dir, &root, raw).unwrap_err();
            assert!(
                err.starts_with(&format!("baseline {raw:?} from the rules file "))
                    && err.contains(cause)
                    && err.contains("inside the --dir directory"),
                "{raw}: {err}"
            );
        }
        assert!(!outside.join("new").exists());
        // 루트를 풀 수 없으면 안인지 확인할 수 없어 거부한다.
        let err = resolve_baseline(&root, &tmp.join("gone"), "base.txt").unwrap_err();
        assert!(err.contains("cannot be checked"), "{err}");
        std::fs::remove_dir_all(&tmp).ok();
    }

    /// 어휘적으로 루트 안이어도 저장소 안의 심볼릭 링크가 밖을 가리키면 쓰기가 밖에 닿는다 — 존재하는
    /// 가장 가까운 조상을 따라가 보고, 풀리지 않는 링크는 어디를 가리킬지 몰라 거부한다. `..`는 링크를
    /// 따라가지 않고 어휘적으로 푼 경로를 돌려준다(`escape/..`를 OS는 링크 대상의 부모로 푼다).
    #[cfg(unix)]
    #[test]
    fn resolve_baseline_refuses_symbolic_links_that_leave_the_root() {
        use std::os::unix::fs::symlink;
        let tmp = std::env::temp_dir().join(format!("rg-config-links-{}", std::process::id()));
        std::fs::remove_dir_all(&tmp).ok();
        let (root, outside) = (tmp.join("root"), tmp.join("outside"));
        std::fs::create_dir_all(root.join("inner")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("victim.txt"), "keep\n").unwrap();
        symlink(&outside, root.join("escape")).unwrap();
        symlink(outside.join("victim.txt"), root.join("victim-link.txt")).unwrap();
        symlink(tmp.join("missing"), root.join("dangling")).unwrap();
        symlink(root.join("inner"), root.join("inner-link")).unwrap();
        for (raw, cause) in [
            ("escape/new.txt", "outside the --dir directory"),
            ("escape/victim.txt", "outside the --dir directory"),
            ("victim-link.txt", "outside the --dir directory"),
            ("dangling", "cannot be resolved"),
            ("dangling/x.txt", "cannot be resolved"),
        ] {
            let err = resolve_baseline(&root, &root, raw).unwrap_err();
            assert!(
                err.contains(cause) && err.contains("inside the --dir directory"),
                "{raw}: {err}"
            );
        }
        assert_eq!(
            resolve_baseline(&root, &root, "inner-link/base.txt").unwrap(),
            root.join("inner-link/base.txt")
        );
        assert_eq!(
            resolve_baseline(&root, &root, "escape/../base.txt").unwrap(),
            root.canonicalize().unwrap().join("base.txt")
        );
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn matches_glob_last_segment() {
        assert!(matches("a::b::*", "a::b::c"));
        assert!(matches("a::*", "a::b"));
        assert!(!matches("a::*", "a::b::c")); // 길이가 다르면 불일치.
        assert!(matches("a::b::c", "a::b::c"));
        assert!(!matches("a::b", "a::b::c"));
    }

    #[test]
    fn feedback_globs_match_within_segments_without_crossing_modules() {
        assert!(matches("my::core*", "my::core_utils"));
        assert!(matches("my::c?re*", "my::core_utils"));
        assert!(!matches("my::core*", "my::core::utils"));
        assert!(matches("my::core*/**", "my::core_utils::nested"));
        assert!(!matches("my::core*/**", "my::other::nested"));
        assert!(matches("my::?", "my::한"));
    }
}
