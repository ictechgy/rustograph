//! 순수 그래프 도메인 — 외부 의존 0.
//!
//! 이 모듈이 산출물이다. 수확·분석·출력은 전부 이 문서를 만들거나 소비하는
//! 역할일 뿐, 도메인 규칙은 여기에만 존재한다. serde 직렬화조차 여기서
//! 정의해서 문서 형식의 권위가 하나가 되게 한다.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// 문서 스키마 버전. 형식이 깨지는 변경이 있으면 올린다.
pub const DOCUMENT_VERSION: u32 = 1;

/// serde skip 조건 — 0인 카운터는 키를 뺀다("없는 것"과 "0"을 구분하는 계약).
pub fn is_zero(n: &usize) -> bool {
    *n == 0
}

/// 수확·투영 깊이. 얕은 레벨은 깊은 레벨의 투영이다 — 수확 결과가 다르면
/// 같은 입력이 레벨에 따라 다른 그래프가 되어 신뢰가 깨진다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// cargo 패키지 단위 — 워크스페이스 크레이트 간 의존.
    Crate,
    /// mod 트리 단위 — 모듈 간 use 관계.
    Module,
    /// 구조체·열거형·트레이트 단위 — 타입 간 implements·references.
    Type,
    /// 함수·메서드·const·매크로 단위 — call·signature까지.
    Symbol,
}

impl Level {
    /// 문자열 플래그 값을 파싱한다. 알 수 없는 값은 호출자가 사용법 오류로 처리한다.
    pub fn parse(s: &str) -> Option<Level> {
        Some(match s {
            "crate" => Level::Crate,
            "module" => Level::Module,
            "type" => Level::Type,
            "symbol" => Level::Symbol,
            _ => return None,
        })
    }
}

/// 정점 종류. 출력 문자열이므로 snake_case로 고정한다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Crate,
    Module,
    Struct,
    Enum,
    Trait,
    Union,
    TypeAlias,
    Fn,
    Method,
    Const,
    Static,
    Macro,
}

/// 간선 종류. 의존 계열(uses/depends/call/references/implements/signature)과
/// 소유 계열(contains)을 섞지 않는다 — contains는 의존이 아니라 소유다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EdgeKind {
    /// 크레이트 → 크레이트 (Cargo.toml 의존).
    Depends,
    /// 모듈 → 모듈/심볼 (use 선언).
    Uses,
    /// 소유: 모듈→아이템, 타입→메서드, 트레이트→메서드.
    Contains,
    /// 함수/메서드 → 함수/메서드/매크로.
    Call,
    /// 타입/심볼 → 타입/심볼 (본문·필드의 타입 참조).
    References,
    /// 타입 → 트레이트 (impl Trait for Type).
    Implements,
    /// 선언 시그니처 안의 타입 참조 — 공개 API 누출 검사용.
    Signature,
}

impl EdgeKind {
    /// 의존성 질의(도달성·순환·규칙)가 따라가는 간선인가.
    /// contains는 소유 관계라 의존으로 오독되면 안 된다.
    pub fn is_dependency(self) -> bool {
        !matches!(self, EdgeKind::Contains)
    }
}

/// 그래프 정점. `generated`·`cfg`는 사실 표시이지 필터가 아니다 — 숨기지 않는다.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vertex {
    /// 정규 ID: `crate` / `crate::module` / `crate::module::Item` /
    /// `crate::module::Type::method` / `crate::module::<Type as Trait>::method`.
    pub id: String,
    pub kind: Kind,
    /// 소유 크레이트 이름.
    #[serde(rename = "crate")]
    pub krate: String,
    /// 소유 모듈 경로(`crate::a::b` 형태의 crate 내부 경로).
    pub module: String,
    /// `file:line`. 크레이트 정점처럼 위치가 없는 종류는 None.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<String>,
    /// `pub`(workspace 외부 노출 포함) 여부.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exported: bool,
    /// cfg/생성 파일 등 조건부 출처 표시.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub generated: bool,
    /// 이 정점 자체에 붙은 `#[cfg(...)]` 조건 — 토큰 그대로(예: `feature = "x"`).
    /// 조상 모듈의 조건은 조상 정점에 있다 — contains 사슬을 따라 합성하면 된다.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cfg: Option<String>,
    /// unsafe fn·unsafe trait이거나 unsafe 블록을 품은 본문 — 경계의 안쪽 표시.
    #[serde(default, rename = "unsafe", skip_serializing_if = "std::ops::Not::not")]
    pub unsafe_: bool,
}

/// 방향 간선. from→to. 의존 계열은 "from이 to를 필요로 한다"로 읽는다.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
    /// 이름 기반 추정 간선(메서드 팬아웃). 도달성은 이를 따라가고
    /// ("살아 있다" 편향), 순환·규칙은 제외하고 제외 수를 센다 —
    /// 가능한 디스패치가 확정 위반·순환으로 둔갑하면 안 된다.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub tentative: bool,
    /// 이 간선이 성립하는 `#[cfg(...)]` 조건 — cfg가 다른 같은 (from,to,kind)는
    /// 별개 간선이다. 조건이 다르면 존재 자체가 다른 빌드에서만 성립하므로.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cfg: Option<String>,
    /// 호출·참조 지점이 `unsafe {}` 블록 안에 있다 — 경계를 넘는 진입 간선.
    #[serde(default, rename = "unsafe", skip_serializing_if = "std::ops::Not::not")]
    pub unsafe_: bool,
}

impl Edge {
    /// 확정 간선.
    pub fn new(from: String, to: String, kind: EdgeKind) -> Edge {
        Edge {
            from,
            to,
            kind,
            tentative: false,
            cfg: None,
            unsafe_: false,
        }
    }

    /// 추정 간선 — 팬아웃으로 만든 가능한 호출.
    pub fn maybe(from: String, to: String, kind: EdgeKind) -> Edge {
        Edge {
            from,
            to,
            kind,
            tentative: true,
            cfg: None,
            unsafe_: false,
        }
    }
}

/// 한 번의 수확 결과 — 버전ed 그래프 산출물.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub version: u32,
    pub tool: String,
    pub level: Level,
    /// 분석한 디렉터리(파일시스템 경로).
    pub root: String,
    /// cargo 워크스페이스 루트 — root와 다를 수 있다.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// 도달성 분석의 보존 루트 ID.
    pub roots: Vec<String>,
    pub vertices: Vec<Vertex>,
    pub edges: Vec<Edge>,
    /// 이 실행에서 실제로 센 분석 한계. 없으면 키가 빠진다.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limitations: Vec<String>,
}

impl Document {
    /// 정렬된 문서 — 같은 입력이 같은 바이트가 되게 하는 계약의 핵심.
    /// (from,to,kind,cfg)가 같은 간선이 겹치면 하나로 합친다 — 정렬에서
    /// 확정·unsafe 표시가 먼저 오므로 dedup이 확정·경계 진입 쪽을 남긴다.
    /// cfg가 다르면 다른 빌드 조건의 별개 간선이라 합치지 않는다.
    pub fn sort(&mut self) {
        self.vertices.sort_by(|a, b| a.id.cmp(&b.id));
        self.edges.sort_by(|a, b| {
            (&a.from, &a.to, a.kind, a.tentative, &a.cfg, !a.unsafe_).cmp(&(
                &b.from,
                &b.to,
                b.kind,
                b.tentative,
                &b.cfg,
                !b.unsafe_,
            ))
        });
        self.edges.dedup_by(|a, b| {
            a.from == b.from && a.to == b.to && a.kind == b.kind && a.cfg == b.cfg
        });
        self.roots.sort();
        self.roots.dedup();
        self.limitations.sort();
        self.limitations.dedup();
    }

    /// 정점 ID 집합 — 존재 확인과 인덱스 구축용.
    pub fn vertex_ids(&self) -> BTreeSet<&str> {
        self.vertices.iter().map(|v| v.id.as_str()).collect()
    }

    /// outgoing 인접 목록 — 의존 간선만. contains는 소유라 제외한다.
    pub fn outgoing(&self) -> BTreeMap<&str, Vec<(&EdgeKind, &str)>> {
        let mut map: BTreeMap<&str, Vec<(&EdgeKind, &str)>> = BTreeMap::new();
        for e in &self.edges {
            if e.kind.is_dependency() {
                map.entry(e.from.as_str())
                    .or_default()
                    .push((&e.kind, e.to.as_str()));
            }
        }
        map
    }

    /// incoming 인접 목록 — 의존 간선만.
    pub fn incoming(&self) -> BTreeMap<&str, Vec<(&EdgeKind, &str)>> {
        let mut map: BTreeMap<&str, Vec<(&EdgeKind, &str)>> = BTreeMap::new();
        for e in &self.edges {
            if e.kind.is_dependency() {
                map.entry(e.to.as_str())
                    .or_default()
                    .push((&e.kind, e.from.as_str()));
            }
        }
        map
    }

    /// 이웃 정점과의 간선 종류 전부 — 하나만 고르면 나머지 관계가 사라진다.
    pub fn edge_kinds_between(&self, a: &str, b: &str) -> Vec<EdgeKind> {
        let mut kinds: Vec<EdgeKind> = self
            .edges
            .iter()
            .filter(|e| (e.from == a && e.to == b) || (e.from == b && e.to == a))
            .map(|e| e.kind)
            .collect();
        kinds.sort();
        kinds.dedup();
        kinds
    }

    /// 상위 레벨로 투영한다. 각 정점·간선을 해당 레벨의 소유 정점으로 귀속시킨다.
    /// 심볼 → 타입 → 모듈 → 크레이트 순으로 올라간다.
    pub fn view(&self, level: Level) -> Document {
        if level >= self.level {
            let mut d = self.clone();
            d.level = level.max(self.level);
            return d;
        }
        let by_id: BTreeMap<&str, &Vertex> =
            self.vertices.iter().map(|v| (v.id.as_str(), v)).collect();
        let owner = |id: &str| -> Option<String> { self.owner_at_level(id, level, &by_id) };
        let mut vertices: BTreeMap<String, Vertex> = BTreeMap::new();
        // 같은 (from,to,kind,cfg)에 확정·추정 간선이 섞여 투영되면 확정으로 —
        // AND 병합이 아니라 "하나라도 확정이면 확정"이다. unsafe는 OR — 하나의
        // 진입 지점이라도 경계 안에 있으면 투영 간선도 경계를 넘는다.
        // 투영 간선 키: (from,to,kind,cfg) → (tentative AND, unsafe OR).
        type ProjectedKey = (String, String, EdgeKind, Option<String>);
        let mut edges: BTreeMap<ProjectedKey, (bool, bool)> = BTreeMap::new();
        for v in &self.vertices {
            if let Some(oid) = owner(&v.id) {
                if let Some(src) = by_id.get(oid.as_str()) {
                    vertices.entry(oid).or_insert_with(|| (*src).clone());
                }
            }
        }
        for e in &self.edges {
            let (Some(f), Some(t)) = (owner(&e.from), owner(&e.to)) else {
                continue;
            };
            if f != t {
                edges
                    .entry((f, t, e.kind, e.cfg.clone()))
                    .and_modify(|(tent, uns)| {
                        *tent &= e.tentative;
                        *uns |= e.unsafe_;
                    })
                    .or_insert((e.tentative, e.unsafe_));
            }
        }
        Document {
            version: self.version,
            tool: self.tool.clone(),
            level,
            root: self.root.clone(),
            workspace: self.workspace.clone(),
            roots: self
                .roots
                .iter()
                .filter_map(|r| owner(r))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
            vertices: vertices.into_values().collect(),
            edges: edges
                .into_iter()
                .map(|((from, to, kind, cfg), (tentative, unsafe_))| Edge {
                    from,
                    to,
                    kind,
                    tentative,
                    cfg,
                    unsafe_,
                })
                .collect(),
            limitations: self.limitations.clone(),
        }
    }

    /// 정점 ID를 주어진 레벨의 소유자로 귀속한다. 상위로 올라가는 기준은
    /// ID의 `::` 경로 접두사 — ID가 곧 소유 경로다.
    fn owner_at_level(
        &self,
        id: &str,
        level: Level,
        by_id: &BTreeMap<&str, &Vertex>,
    ) -> Option<String> {
        let mut cur = id.to_string();
        loop {
            if let Some(v) = by_id.get(cur.as_str()) {
                let in_level = match level {
                    Level::Crate => v.kind == Kind::Crate,
                    Level::Module => v.kind == Kind::Module || v.kind == Kind::Crate,
                    Level::Type => !matches!(
                        v.kind,
                        Kind::Fn | Kind::Method | Kind::Const | Kind::Static | Kind::Macro
                    ),
                    Level::Symbol => true,
                };
                if in_level {
                    return Some(cur);
                }
            }
            // 부모로 한 단계: 마지막 ::segment를 자른다. 크레이트 이름까지 가면 끝.
            match cur.rfind("::") {
                Some(i) => cur.truncate(i),
                None => {
                    return by_id.get(cur.as_str()).map(|_| cur);
                }
            }
        }
    }
}

/// 정렬·중복 제거된 최종 문서를 만든다.
pub fn document(
    level: Level,
    root: String,
    workspace: Option<String>,
    roots: Vec<String>,
    vertices: Vec<Vertex>,
    edges: Vec<Edge>,
    limitations: Vec<String>,
) -> Document {
    let mut d = Document {
        version: DOCUMENT_VERSION,
        tool: "rustograph".to_string(),
        level,
        root,
        workspace,
        roots,
        vertices,
        edges,
        limitations,
    };
    d.sort();
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(id: &str, kind: Kind) -> Vertex {
        Vertex {
            id: id.to_string(),
            kind,
            krate: "c".to_string(),
            module: "c".to_string(),
            position: None,
            exported: false,
            generated: false,
            cfg: None,
            unsafe_: false,
        }
    }

    fn doc() -> Document {
        document(
            Level::Symbol,
            ".".into(),
            None,
            vec!["c::a::f".into()],
            vec![
                v("c", Kind::Crate),
                v("c::a", Kind::Module),
                v("c::a::f", Kind::Fn),
                v("c::b", Kind::Module),
                v("c::b::T", Kind::Struct),
                v("c::b::T::m", Kind::Method),
            ],
            vec![
                Edge::new("c::a".into(), "c::b".into(), EdgeKind::Uses),
                Edge::new("c::a::f".into(), "c::b::T::m".into(), EdgeKind::Call),
                Edge::maybe("c::a::f".into(), "c::b::T".into(), EdgeKind::Call),
                // 같은 쌍의 추정+확정 — dedup은 확정을 남겨야 한다.
                Edge::maybe("c::a".into(), "c::b".into(), EdgeKind::Uses),
            ],
            vec![],
        )
    }

    #[test]
    fn sort_is_deterministic_and_prefers_firm_edges() {
        let d = doc();
        let json1 = serde_json::to_string_pretty(&d).unwrap();
        // 두 번 직렬화해도 같은 바이트.
        let json2 = serde_json::to_string_pretty(&d).unwrap();
        assert_eq!(json1, json2);
        // 중복 (c::a→c::b,uses)는 확정 간선 하나만 남는다.
        let uses: Vec<&Edge> = d
            .edges
            .iter()
            .filter(|e| e.from == "c::a" && e.to == "c::b" && e.kind == EdgeKind::Uses)
            .collect();
        assert_eq!(uses.len(), 1);
        assert!(!uses[0].tentative);
    }

    #[test]
    fn view_projects_symbols_to_modules() {
        let d = doc().view(Level::Module);
        assert!(d.vertex_ids().contains("c::a"));
        assert!(d.vertex_ids().contains("c::b"));
        assert!(!d.vertex_ids().contains("c::a::f"));
        // f -> T::m 호출은 모듈 레벨에서 a -> b call이 된다.
        assert!(d
            .edges
            .iter()
            .any(|e| e.from == "c::a" && e.to == "c::b" && e.kind == EdgeKind::Call));
        // 루트도 투영된다.
        assert!(d.roots.contains(&"c::a".to_string()));
    }

    #[test]
    fn view_merges_tentative_only_when_all_tentative() {
        // a::f -> T::m (확정 call) 과 a::f -> T (추정 call)는 다른 to라 따로 남는다.
        let d = doc().view(Level::Module);
        let call = d
            .edges
            .iter()
            .find(|e| e.from == "c::a" && e.to == "c::b" && e.kind == EdgeKind::Call)
            .unwrap();
        // f->T::m은 확정, f->T는 추정 — 투영 후 AND 병합으로 확정이 이긴다.
        assert!(!call.tentative);
    }

    #[test]
    fn deserialize_defaults_missing_flags() {
        // tentative/exported/generated 없는 옛 문서도 읽힌다.
        let j = r#"{"version":1,"tool":"rustograph","level":"symbol","root":".",
            "roots":[],"vertices":[{"id":"c","kind":"crate","crate":"c","module":"c"}],
            "edges":[{"from":"a","to":"b","kind":"call"}]}"#;
        let d: Document = serde_json::from_str(j).unwrap();
        assert!(!d.edges[0].tentative);
        assert!(!d.vertices[0].exported);
    }
}
