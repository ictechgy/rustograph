//! Rust 경로 문자열이 생략한 선언 정체를 소스 위치로 보완한다.
//!
//! 타입 인자·서로 다른 트레이트 경로·cfg·이름공간을 문자열 경로만으로 구분할 수 없다.
//! 충돌한 경로에만 workspace 상대 파일·선언 바이트 위치·종류를 붙인다. 타입 추론을
//! 흉내 내지 않으며, 소스가 확정하지 못한 대상 참조는 후보 간선으로 남긴다.

use crate::graph::{Edge, EdgeKind, Kind, Vertex};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

type VertexKey = (String, Kind, Option<String>, Option<String>);
type SiteKey = (String, PathBuf, usize);

#[derive(Default)]
pub(crate) struct Registry {
    sites: Vec<Site>,
}

struct Site {
    key: VertexKey,
    file: PathBuf,
    start: usize,
}

#[derive(Clone)]
struct Target {
    id: String,
    kind: Kind,
    cfg: Option<String>,
}

#[derive(Default)]
pub(crate) struct Identities {
    sites: BTreeMap<SiteKey, String>,
    aliases: BTreeMap<String, Vec<Target>>,
    pub groups: usize,
    pub extra: usize,
}

fn vertex_key(v: &Vertex) -> VertexKey {
    (v.id.clone(), v.kind, v.position.clone(), v.cfg.clone())
}

impl Registry {
    pub fn note(&mut self, v: &Vertex, file: &Path, start: usize) {
        self.sites.push(Site {
            key: vertex_key(v),
            file: file.to_path_buf(),
            start,
        });
    }

    pub fn identify(&self, vertices: &mut Vec<Vertex>, root: &Path) -> Identities {
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for v in vertices.iter() {
            *counts.entry(v.id.clone()).or_default() += 1;
        }
        let mut notes: BTreeMap<VertexKey, Vec<&Site>> = BTreeMap::new();
        for s in &self.sites {
            notes.entry(s.key.clone()).or_default().push(s);
        }
        // 같은 행에 여러 선언이 있어도 뒤바뀌지 않게 기록한 생성 순서를 지킨다.
        let mut used: BTreeMap<VertexKey, usize> = BTreeMap::new();
        let mut out = Identities::default();
        for v in vertices.iter_mut() {
            let key = vertex_key(v);
            if counts[v.id.as_str()] < 2 {
                continue;
            }
            // 외부 의존은 소스를 수확하지 않는 패키지 이름 경계 정점이다.
            // 같은 이름의 여러 버전은 기존 모델대로 그 경계를 공유한다.
            if v.kind == Kind::Crate && v.position.is_none() {
                continue;
            }
            let n = used.entry(key.clone()).or_default();
            let site = notes
                .get(&key)
                .and_then(|ss| ss.get(*n))
                .expect("every harvested vertex has a declaration site");
            *n += 1;
            let legacy = v.id.clone();
            // 모듈 경로는 트리 해석과 하위 정점의 기준이다. 같은 경로의 값 아이템만
            // 바꾸면 모듈 경로와 자식 경로를 모두 다시 쓰지 않아도 이름공간이 분리된다.
            if !matches!(v.kind, Kind::Crate | Kind::Module) {
                let file = site.file.strip_prefix(root).unwrap_or(&site.file);
                let encoded = encode_path(file);
                let (parent, name) = legacy
                    .rsplit_once("::")
                    .expect("non-root items have an owner");
                v.id = format!("{parent}::@{encoded}#{}#{:?}::{name}", site.start, v.kind);
            }
            if !matches!(v.kind, Kind::Crate | Kind::Module) {
                out.sites.insert(
                    (legacy.clone(), site.file.clone(), site.start),
                    v.id.clone(),
                );
            }
            out.aliases.entry(legacy).or_default().push(Target {
                id: v.id.clone(),
                kind: v.kind,
                cfg: v.cfg.clone(),
            });
        }
        for targets in out.aliases.values_mut() {
            targets.sort_by(|a, b| a.id.cmp(&b.id));
            targets.dedup_by(|a, b| a.id == b.id);
        }
        out.groups = out.aliases.len();
        out.extra = out
            .aliases
            .values()
            .map(|ts| ts.len().saturating_sub(1))
            .sum();
        // 같은 파일·범위가 공유 lib/bin 루트로 다시 들어온 경우는 같은 선언이다.
        let mut seen = BTreeSet::new();
        vertices.retain(|v| seen.insert(v.id.clone()));
        out
    }
}

fn encode_path(path: &Path) -> String {
    let mut out = String::new();
    for byte in path.to_string_lossy().as_bytes() {
        if byte.is_ascii_alphanumeric() || b"/_-.".contains(byte) {
            out.push(char::from(*byte));
        } else {
            use std::fmt::Write;
            write!(out, "%{byte:02X}").expect("writing a String cannot fail");
        }
    }
    out
}

impl Identities {
    pub fn owner(&self, id: &str, file: &Path, start: usize) -> String {
        self.sites
            .get(&(id.to_string(), file.to_path_buf(), start))
            .cloned()
            .unwrap_or_else(|| id.to_string())
    }

    pub fn roots(&self, roots: &mut Vec<String>) {
        *roots = roots
            .iter()
            .flat_map(|id| {
                self.aliases
                    .get(id)
                    .map(|ts| ts.iter().map(|t| t.id.clone()).collect())
                    .unwrap_or_else(|| vec![id.clone()])
            })
            .collect();
    }

    /// 본문 소유자는 호출 전에 소스로 확정한다. 구조·속성 간선은 from도
    /// 옛 경로이므로 호출자가 그 경우를 명시한다.
    pub fn edges(&self, edges: Vec<Edge>, legacy_from: bool) -> Vec<Edge> {
        if self.aliases.is_empty() {
            return edges;
        }
        let mut out = Vec::new();
        for edge in edges {
            let from = legacy_from.then(|| self.targets(&edge, true)).flatten();
            let to = self.targets(&edge, false);
            if from.is_none() && to.is_none() {
                out.push(edge);
                continue;
            }
            let fs: Vec<_> = from.unwrap_or_else(|| vec![edge.from.as_str()]);
            let ts: Vec<_> = to.unwrap_or_else(|| vec![edge.to.as_str()]);
            for f in &fs {
                for t in &ts {
                    let mut e = edge.clone();
                    e.from = (*f).to_string();
                    e.to = (*t).to_string();
                    if e.kind.is_dependency() && (fs.len() > 1 || ts.len() > 1) {
                        e.tentative = true;
                    }
                    out.push(e);
                }
            }
        }
        out
    }

    fn targets<'a>(&'a self, edge: &'a Edge, from: bool) -> Option<Vec<&'a str>> {
        let id = if from { &edge.from } else { &edge.to };
        let candidates = self.aliases.get(id)?;
        let eligible = |t: &&Target| {
            if from
                && matches!(edge.kind, EdgeKind::Contains | EdgeKind::Uses)
                && candidates
                    .iter()
                    .any(|c| matches!(c.kind, Kind::Crate | Kind::Module))
            {
                return matches!(t.kind, Kind::Crate | Kind::Module);
            }
            if !from && edge.kind == EdgeKind::Call {
                return !matches!(t.kind, Kind::Crate | Kind::Module);
            }
            if !from && edge.kind == EdgeKind::Signature {
                return !matches!(
                    t.kind,
                    Kind::Fn | Kind::Method | Kind::Const | Kind::Static | Kind::Macro
                );
            }
            if edge.kind == EdgeKind::Implements {
                return if from {
                    matches!(
                        t.kind,
                        Kind::Struct | Kind::Enum | Kind::Union | Kind::TypeAlias
                    )
                } else {
                    t.kind == Kind::Trait
                };
            }
            true
        };
        let mut picked: Vec<_> = candidates.iter().filter(eligible).collect();
        if !from && edge.kind == EdgeKind::Contains && picked.iter().any(|t| t.cfg == edge.cfg) {
            picked.retain(|t| t.cfg == edge.cfg);
        }
        // 어떤 이름공간인지 증명하지 못하면 후보를 버리는 대신 모두 보존한다.
        if picked.is_empty() {
            picked.extend(candidates);
        }
        Some(picked.into_iter().map(|t| t.id.as_str()).collect())
    }
}
