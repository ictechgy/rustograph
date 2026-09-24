//! `cargo metadata` 소비자 — 크레이트·워크스페이스 사실을 가져온다.
//!
//! cargo_metadata 크레이트를 쓰지 않고 JSON을 직접 파싱한다: 필요한 필드가
//! 적어 의존 하나를 아끼는 게 합리적이고, 스키마의 이 부분은 안정적이다.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// cargo 패키지 하나.
#[derive(Debug)]
pub struct Package {
    /// 크레이트 이름(`-`는 `_`로 정규화해 모듈 경로와 맞춘다).
    pub name: String,
    /// 패키지 버전 — 중복 버전 탐지에 쓴다.
    pub version: String,
    /// lib/bin 타깃의 엔트리 파일들.
    pub targets: Vec<Target>,
    /// 이 패키지가 워크스페이스 멤버인가.
    pub workspace_member: bool,
    /// proc-macro 크레이트인가 — 사용 증거가 derive/속성 경로라 호출
    /// 그래프에 안 잡힐 수 있어 deps 보고서가 약한 증거로 표시한다.
    /// targets에서 proc-macro 타깃을 걸러내기 전에 따로 잡아야 한다.
    pub proc_macro: bool,
}

impl Package {
    /// 이 패키지의 그래프상 크레이트 정점 ID. 워크스페이스 멤버는
    /// lib/bin 타깃의 루트 모듈이 크레이트 정점을 겸하니 타깃 이름이
    /// ID다 — 패키지 이름은 `[lib] name`이 다르면 정점이 아니다.
    /// 외부 패키지는 이름이 곧 정점이다.
    pub fn crate_vertex(&self) -> String {
        self.targets
            .iter()
            .find(|t| t.kind == "lib")
            .or_else(|| self.targets.iter().find(|t| t.kind == "bin"))
            .map(|t| t.name.clone())
            .unwrap_or_else(|| self.name.clone())
    }
}

/// lib/bin 타깃 하나 — 모듈 트리의 루트가 된다.
#[derive(Debug)]
pub struct Target {
    /// 타깃 이름(= 루트 모듈 이름).
    pub name: String,
    /// "lib" | "bin" | "example" | "test" | "bench" ...
    pub kind: String,
    /// 엔트리 소스 파일.
    pub src: PathBuf,
}

/// 워크스페이스 의존 해석 결과 — `from → to` 크레이트 간선과 종류.
#[derive(Debug)]
pub struct DepEdge {
    pub from: String,
    pub to: String,
    /// ""(normal) | "dev" | "build".
    pub kind: String,
    /// 코드가 실제로 쓰는 라이브러리 이름 — `foo = { package = "real" }`의
    /// rename을 거친 이름이라 use 경로의 첫 세그먼트와 같다.
    pub lib_name: String,
}

/// 수확에 필요한 메타데이터 전부.
#[derive(Debug)]
pub struct Metadata {
    pub packages: Vec<Package>,
    /// 패키지 ID → 패키지 인덱스.
    pub by_id: BTreeMap<String, usize>,
    /// resolve의 크레이트 간선(resolve가 없으면 빈 Vec).
    pub dep_edges: Vec<DepEdge>,
    pub workspace_root: PathBuf,
    /// resolve가 없을 때 등의 실측 한계.
    pub limitations: Vec<String>,
}

#[derive(Deserialize)]
struct RawMetadata {
    packages: Vec<RawPackage>,
    resolve: Option<RawResolve>,
    workspace_members: Vec<String>,
    workspace_root: String,
}

#[derive(Deserialize)]
struct RawPackage {
    id: String,
    name: String,
    version: String,
    targets: Vec<RawTarget>,
}

#[derive(Deserialize)]
struct RawTarget {
    name: String,
    kind: Vec<String>,
    src_path: String,
}

#[derive(Deserialize)]
struct RawResolve {
    nodes: Vec<RawNode>,
}

#[derive(Deserialize)]
struct RawNode {
    id: String,
    deps: Vec<RawDep>,
}

#[derive(Deserialize)]
struct RawDep {
    pkg: String,
    /// 코드에서 보이는 라이브러리 이름 — rename을 반영한다.
    name: Option<String>,
    dep_kinds: Vec<RawDepKind>,
}

#[derive(Deserialize)]
struct RawDepKind {
    kind: Option<String>,
}

/// 크레이트 이름을 모듈 경로용으로 정규화한다 — `my-crate`는 Rust에서 `my_crate`다.
pub fn normalize_name(name: &str) -> String {
    name.replace('-', "_")
}

/// `rustc --print cfg --target`의 원시 출력 — 타깃의 권위 있는 cfg
/// 팩트다. rustc가 없거나 트리플을 모르면 None — 호출자가 트리플
/// 추정으로 폴백한다. 반환값은 해석하지 않은 줄 목록이다 — 파싱은
/// 순수 도메인(cfgeval)의 일이라 여기선 그대로 넘긴다.
pub fn rustc_cfg_lines(triple: &str) -> Option<Vec<String>> {
    let out = Command::new("rustc")
        .args(["--print", "cfg", "--target", triple])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .collect(),
    )
}

/// `rustc -vV`의 원시 출력 — 툴체인 버전은 수확 결과를 바꾸는
/// 빌드 입력이라 캐시 지문에 넣는다. rustc가 없으면 None — 호출자는
/// 지문을 만들 수 없으니 캐시를 쓰지 않는다.
pub fn rustc_version() -> Option<String> {
    let out = Command::new("rustc").arg("-vV").output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).to_string())
}

/// `cargo metadata`를 실행해 워크스페이스 사실을 읽는다.
/// dir이 cargo 프로젝트가 아니면 오류 — 빈 그래프로 속이지 않는다.
pub fn load(dir: &Path) -> Result<Metadata, String> {
    let out = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--manifest-path"])
        .arg(dir.join("Cargo.toml"))
        .output()
        .map_err(|e| format!("failed to run cargo metadata: {e}"))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "cargo metadata failed in {}: {}",
            dir.display(),
            stderr.trim()
        ));
    }
    parse(&out.stdout)
}

fn parse(bytes: &[u8]) -> Result<Metadata, String> {
    let raw: RawMetadata =
        serde_json::from_slice(bytes).map_err(|e| format!("bad cargo metadata JSON: {e}"))?;
    let members: std::collections::BTreeSet<&str> =
        raw.workspace_members.iter().map(|s| s.as_str()).collect();
    // deps[].name이 없을 때의 폴백용 — packages를 소비하기 전에 뺀다.
    let raw_names: BTreeMap<String, String> = raw
        .packages
        .iter()
        .map(|p| (p.id.clone(), p.name.clone()))
        .collect();
    let mut by_id = BTreeMap::new();
    let mut packages = Vec::new();
    for p in raw.packages {
        by_id.insert(p.id.clone(), packages.len());
        // proc-macro 타깃은 아래 필터에서 버려진다 — 걸러내기 전에
        // 패키지 수준 플래그로 따로 잡는다.
        let proc_macro = p
            .targets
            .iter()
            .any(|t| t.kind.iter().any(|k| k == "proc-macro"));
        packages.push(Package {
            workspace_member: members.contains(p.id.as_str()),
            name: normalize_name(&p.name),
            version: p.version,
            proc_macro,
            targets: p
                .targets
                .into_iter()
                .filter(|t| {
                    t.kind
                        .iter()
                        .any(|k| matches!(k.as_str(), "lib" | "bin" | "example" | "test" | "bench"))
                })
                .map(|t| Target {
                    name: normalize_name(&t.name),
                    kind: t.kind.first().cloned().unwrap_or_default(),
                    src: PathBuf::from(t.src_path),
                })
                .collect(),
        });
    }
    let mut dep_edges = Vec::new();
    let mut limitations = Vec::new();
    match raw.resolve {
        Some(resolve) => {
            for n in resolve.nodes {
                for d in n.deps {
                    // deps[].name이 없으면 패키지 이름이 곧 lib 이름이다.
                    let lib = d
                        .name
                        .clone()
                        .or_else(|| raw_names.get(&d.pkg).cloned())
                        .unwrap_or_default();
                    for k in d.dep_kinds {
                        dep_edges.push(DepEdge {
                            from: n.id.clone(),
                            to: d.pkg.clone(),
                            kind: k.kind.unwrap_or_default(),
                            lib_name: normalize_name(&lib),
                        });
                    }
                }
            }
        }
        None => limitations.push(
            "cargo metadata returned no resolve graph; dependency edges unavailable".to_string(),
        ),
    }
    Ok(Metadata {
        packages,
        by_id,
        dep_edges,
        workspace_root: PathBuf::from(raw.workspace_root),
        limitations,
    })
}
