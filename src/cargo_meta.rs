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
    /// lib/bin 타깃의 엔트리 파일들.
    pub targets: Vec<Target>,
    /// 이 패키지가 워크스페이스 멤버인가.
    pub workspace_member: bool,
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
    let mut by_id = BTreeMap::new();
    let mut packages = Vec::new();
    for p in raw.packages {
        by_id.insert(p.id.clone(), packages.len());
        packages.push(Package {
            workspace_member: members.contains(p.id.as_str()),
            name: normalize_name(&p.name),
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
                    for k in d.dep_kinds {
                        dep_edges.push(DepEdge {
                            from: n.id.clone(),
                            to: d.pkg.clone(),
                            kind: k.kind.unwrap_or_default(),
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
