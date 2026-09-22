//! 의미 해석 엔진 — ra_ap(rust-analyzer) 기반.
//!
//! `semantic` cargo feature로만 컴파일된다 — 의존이 크고 주 단위 API 변동이
//! 있어 opt-in이다. syn 수확이 이름으로 추정하던 부분을 실제 타입 해석으로
//! 대체한다: 메서드 호출은 수신자 타입으로 확정하고, dyn·제네릭 수신자의
//! 트레이트 호출은 워크스페이스 impl 후보로 추정 간선을 펼치며(trait impl
//! 행렬), 매크로 호출은 확장 트리까지 내려가 내부의 호출·참조를 수확한다.
//! 그래프 계약(정점·간선 스키마, tentative 의미)은 바꾸지 않는다 —
//! 정확도만 올리는 것이 이 모듈의 존재 이유다.
//!
//! 경계 규칙: ra_ap은 여기서만 만진다. 구조(정점·contains·uses·implements·
//! signature)는 전부 syn 수확이 권위다 — 이 모듈은 본문 간선만 낸다.

use crate::cargo_meta::normalize_name;
use crate::graph::{Edge, EdgeKind};
use ra_ap_hir::db::HirDatabase;
use ra_ap_hir::{
    Adt, AsAssocItem, AssocItem, AssocItemContainer, Const, Crate, Enum, Function, Impl, Macro,
    Module, ModuleDef, PathResolution, Semantics, Static, Trait, Type, TypeAlias,
};
use ra_ap_ide_db::RootDatabase;
use ra_ap_load_cargo::{load_workspace_at, LoadCargoConfig, ProcMacroServerChoice};
use ra_ap_proc_macro_api::ProcMacroClient;
use ra_ap_project_model::{CargoConfig, RustLibSource};
use ra_ap_syntax::ast::{self, AstNode};
use ra_ap_syntax::{SyntaxNode, TextRange, TextSize};
use ra_ap_vfs::{FileId, Vfs, VfsPath};
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;
use std::path::Path;

/// 매크로 확장 재귀 한계 — 재귀 매크로(`m!() => { m!() }`류)는 유한해야 한다.
const MAX_EXPANSION_DEPTH: usize = 16;

/// 의미 해석 실측 — 호출자가 limitation 문장으로 번역한다.
#[derive(Debug, Default)]
pub struct Stats {
    /// 타입 정보로 확정한 호출·참조 간선 수(추정이 아닌 것).
    pub resolved: usize,
    /// 트레이트 디스패치 지점 수 — dyn/제네릭 수신자라 impl 후보로 펼쳤다.
    pub trait_sites: usize,
    /// ra도 해석하지 못해 이름 팬아웃으로 되돌아간 메서드 호출 수.
    pub fanned: usize,
    /// 그래프 밖(의존 크레이트·std·derive 생성 정의)으로 해석된 타깃 수.
    pub external: usize,
    /// 해석된 외부·std 매크로 호출 수 — syn의 external_macros와 같은 버킷.
    pub ext_macros: usize,
    /// 확장에 실패한 매크로 호출 수(proc 서버 부재·확장 오류).
    pub unexpanded: usize,
    /// 확장된 매크로 호출 수 — limitation이 아니라 커버리지 지표.
    pub expanded: usize,
    /// ra도 해석 못 한 경로 참조 수 — syn의 unresolved_paths와 같은 버킷.
    pub unresolved: usize,
    /// hir이 모르는 본문 수 — cfg 비활성·매크로 생성 정의는 syn 폴백으로 간다.
    pub unmapped: usize,
}

/// 의미 해석 세션 — 로드된 워크스페이스 DB와 정규 ID → 정의 인덱스.
/// DB는 salsa 스냅샷이다 — load 이후 소스가 바뀌면 다시 load해야 한다.
pub struct Engine {
    db: RootDatabase,
    /// vfs는 본문 소유자의 소스 정체(파일)를 맞출 때 쓴다 — 정규 ID만으로는
    /// cfg 변형·lib/bin 합본을 구분 못 한다. proc 매크로 서버는 외부
    /// 프로세스라 클라이언트를 살려둬야 확장이 동작한다.
    vfs: Vfs,
    proc_macro: Option<ProcMacroClient>,
    /// 정규 ID → 본문 소유자 정의(fn·const·static)와 소스 정체.
    /// 워크스페이스 크레이트만, 블록 지역·매크로 생성 정의는 제외.
    defs: BTreeMap<String, BodyEntry>,
    /// (트레이트 정규 ID, 메서드 이름) → 워크스페이스 impl 메서드 ID들.
    /// 디스패치 지점마다 impl을 다시 훑지 않게 빌드 때 한 번 만든다.
    matrix: BTreeMap<(String, String), Vec<String>>,
}

/// 본문을 가질 수 있는 정의 — fn·const·static.
#[derive(Clone, Copy)]
enum BodyDef {
    Fn(Function),
    Const(Const),
    Static(Static),
}

/// 인덱스 항목 — 정의 + 소스 정체. 같은 정규 ID를 가리키는 정의가
/// 여럿일 수 있어(cfg 변형 등) 파일·범위로 진짜 소유자를 가린다.
struct BodyEntry {
    def: BodyDef,
    file: FileId,
    range: TextRange,
}

/// 빌드 결과 — 본문 소유자 인덱스 + 트레이트 디스패치 후보 표.
struct Index {
    /// 정규 ID → 본문 소유자 정의와 소스 정체.
    defs: BTreeMap<String, BodyEntry>,
    /// (트레이트 정규 ID, 메서드 이름) → 워크스페이스 impl 메서드 ID들.
    matrix: BTreeMap<(String, String), Vec<String>>,
}

/// 본문 소유자 항목 — 정규 ID + 소스 정체(파일·바이트 범위) + cfg.
/// 정규 ID만으로는 cfg 변형·lib/bin 합본을 구분 못 해 소스 정체까지 받는다.
pub struct OwnerSite<'a> {
    /// 정규 정점 ID — syn 수확과 같은 스킴.
    pub id: &'a str,
    /// 소유 아이템의 `#[cfg]` —보내는 간선이 물려받는다.
    pub cfg: &'a Option<String>,
    /// 소유 아이템이 선언된 파일.
    pub file: &'a Path,
    /// 소유 아이템의 바이트 범위.
    pub range: &'a Range<usize>,
}

impl Engine {
    /// `dir`의 cargo 워크스페이스를 의미 DB로 로드하고 정의 인덱스를 만든다.
    /// `cargo check`는 돌리지 않는다(load_out_dirs_from_check: false) —
    /// 빌드 스크립트 산출물(OUT_DIR·include!)은 해석 밖에 남는다.
    /// 로드 실패는 오류 — syn으로 조용히 떨어지면 --semantic이 거짓말이 된다.
    pub fn load(dir: &Path) -> Result<Engine, String> {
        // sysroot 없이는 std 매크로(println! 류)조차 해석되지 않는다 —
        // 확장 실패는 인자 속 호출까지 통째로 잃으므로 반드시 켠다.
        let cargo_config = CargoConfig {
            sysroot: Some(RustLibSource::Discover),
            ..CargoConfig::default()
        };
        let load_config = LoadCargoConfig {
            load_out_dirs_from_check: false,
            with_proc_macro_server: ProcMacroServerChoice::Sysroot,
            prefill_caches: false,
            num_worker_threads: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
            proc_macro_processes: 1,
        };
        let (db, vfs, proc_macro) = load_workspace_at(dir, &cargo_config, &load_config, &|_| {})
            .map_err(|e| format!("semantic engine could not load {}: {e:#}", dir.display()))?;
        // 타입 소속(self_ty)·트레이트 쿼리는 next-solver의 스레드 로컬
        // attached db를 요구한다 — 인덱스 빌드부터 붙여야 panic이 안 난다.
        let index = ra_ap_hir::attach_db(&db, || build_index(&db));
        Ok(Engine {
            db,
            vfs,
            proc_macro,
            defs: index.defs,
            matrix: index.matrix,
        })
    }

    /// proc 매크로 서버가 붙었는가 — 없으면 proc 매크로 호출은 unexpanded로 간다.
    pub fn has_proc_macros(&self) -> bool {
        self.proc_macro.is_some()
    }

    /// 본문 하나를 의미 해석으로 걷는다 — 호출·참조·매크로 간선.
    /// `site`의 파일·범위는 syn이 본 소유 아이템의 소스 위치다 — 정규 ID가
    /// 같아도 소스 정체가 다르면(cfg 변형·블록 지역 정의) 다른 정의이므로
    /// None으로 되돌려 syn 폴백시킨다. 시그니처 간선은 항상 syn 경로다.
    pub fn body_edges(
        &self,
        site: &OwnerSite<'_>,
        ids: &BTreeSet<&str>,
        method_index: &BTreeMap<String, Vec<String>>,
        st: &mut Stats,
    ) -> Option<Vec<Edge>> {
        let entry = self.defs.get(site.id)?;
        let (file_id, _) = self.vfs.file_id(&VfsPath::new_real_path(
            site.file.to_string_lossy().into_owned(),
        ))?;
        if entry.file != file_id {
            return None;
        }
        let (Ok(start), Ok(end)) = (
            u32::try_from(site.range.start),
            u32::try_from(site.range.end),
        ) else {
            return None;
        };
        entry
            .range
            .intersect(TextRange::new(TextSize::from(start), TextSize::from(end)))?;
        let sema = Semantics::new(&self.db);
        let root = entry.def.body_root(&sema)?;
        // next-solver의 트레이트 해석은 스레드 로컬 attached db를 요구한다 —
        // resolve_method_call 안에서 panic 나지 않게 걷기 전에 붙인다.
        let mut w = Walker {
            sema: &sema,
            owner: site.id,
            cfg: site.cfg,
            ids,
            method_index,
            matrix: &self.matrix,
            st,
            edges: Vec::new(),
        };
        ra_ap_hir::attach_db(&self.db, || w.walk(&root, false, 0));
        Some(w.edges)
    }
}

/// 정규 ID → hir 정의 인덱스를 만든다 — 워크스페이스(Local) 크레이트만.
/// 정점은 syn이 만들었으므로 여기서는 "본문 소유자를 찾는" 매핑만 필요하다.
/// 모듈은 루트에서 children으로만 걷는다 — `krate.modules()`는 fn 안의
/// 블록 모듈까지 포함해 정규 경로가 부모 체인과 어긋날 수 있다.
fn build_index(db: &RootDatabase) -> Index {
    let sema = Semantics::new(db);
    let mut defs: BTreeMap<String, BodyEntry> = BTreeMap::new();
    let mut matrix: BTreeMap<(String, String), Vec<String>> = BTreeMap::new();
    let mut stack: Vec<Module> = Vec::new();
    for krate in Crate::all(db) {
        if !krate.origin(db).is_local() {
            continue;
        }
        stack.push(krate.root_module(db));
        while let Some(module) = stack.pop() {
            for def in module.declarations(db) {
                match def {
                    ModuleDef::Function(f) => push_fn(&sema, &mut defs, f),
                    ModuleDef::Const(c) => {
                        if let Some(id) = const_id(db, c) {
                            if let Some(e) = body_entry(&sema, BodyDef::Const(c)) {
                                defs.insert(id, e);
                            }
                        }
                    }
                    ModuleDef::Static(s) => {
                        if let Some(id) = static_id(db, s) {
                            if let Some(e) = body_entry(&sema, BodyDef::Static(s)) {
                                defs.insert(id, e);
                            }
                        }
                    }
                    ModuleDef::Trait(t) => {
                        // 트레이트 기본 메서드 — 선언점 메서드 정점의 본문.
                        for item in t.items(db) {
                            if let AssocItem::Function(f) = item {
                                push_fn(&sema, &mut defs, f);
                            }
                        }
                    }
                    _ => {}
                }
            }
            stack.extend(module.children(db));
        }
        // impl 블록 메서드 — 선언 모듈이 아니라 self 타입 소속으로 ID를 만든다.
        for imp in Impl::all_in_crate(db, krate) {
            // 트레이트 impl이면 디스패치 후보 표에도 넣는다.
            let tid = imp.trait_(db).map(|t| trait_id(db, t));
            for item in imp.items(db) {
                let AssocItem::Function(f) = item else {
                    continue;
                };
                push_fn(&sema, &mut defs, f);
                if let (Some(tid), Some(id)) = (&tid, fn_id(&sema, f)) {
                    matrix
                        .entry((tid.clone(), f.name(db).as_str().to_string()))
                        .or_default()
                        .push(id);
                }
            }
        }
    }
    Index { defs, matrix }
}

impl BodyDef {
    /// 정의의 본문 루트 — 아이템이 아니라 본문(또는 초기값)부터 걷는다.
    /// 블록 안에 선언된 중첩 정의의 본문이 소유자에게 귀속되지 않도록,
    /// 그리고 시그니처 표면이 본문 간선으로 새지 않도록 하는 시작점이다.
    fn body_root(&self, sema: &Semantics<RootDatabase>) -> Option<SyntaxNode> {
        match self {
            BodyDef::Fn(f) => Some(sema.source(*f)?.value.body()?.syntax().clone()),
            BodyDef::Const(c) => Some(sema.source(*c)?.value.body()?.syntax().clone()),
            BodyDef::Static(s) => Some(sema.source(*s)?.value.body()?.syntax().clone()),
        }
    }
}

/// 정의의 소스 정체를 잡아 인덱스 항목으로 만든다 — 블록(fn 본문) 안에
/// 선언된 정의와 매크로가 만든 정의(원본 파일 범위가 없는 것)는 인덱스하지
/// 않는다: 둘 다 정규 ID가 실제 정점과 충돌할 수 있기 때문이다.
fn body_entry(sema: &Semantics<RootDatabase>, def: BodyDef) -> Option<BodyEntry> {
    let node = match def {
        BodyDef::Fn(f) => sema.source(f)?.value.syntax().clone(),
        BodyDef::Const(c) => sema.source(c)?.value.syntax().clone(),
        BodyDef::Static(s) => sema.source(s)?.value.syntax().clone(),
    };
    if block_local(&node) {
        return None;
    }
    let fr = sema.original_range_opt(&node)?;
    Some(BodyEntry {
        def,
        file: fr.file_id.file_id(sema.db),
        range: fr.range,
    })
}

/// 구문 노드가 본문(블록) 안에 선언됐는가 — 정규 모듈 경로는 이 위치를
/// 무시하므로, 이런 정의를 ID로 올리면 같은 이름의 정점과 충돌한다.
fn block_local(node: &SyntaxNode) -> bool {
    node.ancestors().any(|a| ast::BlockExpr::cast(a).is_some())
}

/// hir 정의가 블록 지역 선언인가 — 소스를 얻을 수 없는 정의는 false.
fn block_local_def<D: ra_ap_hir::HasSource>(sema: &Semantics<RootDatabase>, d: D) -> bool
where
    D::Ast: AstNode,
{
    sema.source(d)
        .is_some_and(|s| block_local(s.value.syntax()))
}

/// 함수 한 개를 ID 계산해 entries에 넣는다 — 클로저로 쓰면 entries와
/// db 빌림이 얽혀서 자유 함수다.
fn push_fn(sema: &Semantics<RootDatabase>, defs: &mut BTreeMap<String, BodyEntry>, f: Function) {
    if let Some(id) = fn_id(sema, f) {
        if let Some(e) = body_entry(sema, BodyDef::Fn(f)) {
            defs.insert(id, e);
        }
    }
}

/// 모듈의 정규 경로 — `crate` 또는 `crate::a::b`. 루트 모듈은 이름이 없어
/// 크레이트 이름을 그대로 쓴다(syn 경로의 루트 정점과 같은 ID).
fn module_path(db: &dyn HirDatabase, module: Module) -> String {
    let mut segs = Vec::new();
    let mut cur = module;
    while let Some(parent) = cur.parent(db) {
        if let Some(name) = cur.name(db) {
            segs.push(name.as_str().to_string());
        }
        cur = parent;
    }
    let krate = crate_name(db, cur.krate(db));
    if segs.is_empty() {
        krate
    } else {
        segs.reverse();
        format!("{krate}::{}", segs.join("::"))
    }
}

/// 크레이트 표시 이름을 정규 ID용으로 — cargo 이름의 `-`는 `_`로.
fn crate_name(db: &dyn HirDatabase, krate: Crate) -> String {
    krate
        .display_name(db)
        .map(|d| normalize_name(d.canonical_name().as_str()))
        .unwrap_or_else(|| "?".to_string())
}

/// ADT의 정규 ID — `crate::mod::Type`.
fn adt_id(db: &dyn HirDatabase, adt: Adt) -> String {
    format!(
        "{}::{}",
        module_path(db, adt.module(db)),
        adt.name(db).as_str()
    )
}

/// 함수 정규 ID — 자유 함수 `m::f`, 고유 메서드 `T::m`, 트레이트 impl
/// 메서드 `T::<Tr>::m`, 트레이트 선언 메서드 `m::Tr::m`. syn 수확과 같은
/// 스킴이다 — 같은 코드가 같은 ID를 가져야 간선이 만난다.
fn fn_id(sema: &Semantics<RootDatabase>, f: Function) -> Option<String> {
    let db = sema.db;
    let name = f.name(db).as_str().to_string();
    if let Some(assoc) = f.as_assoc_item(db) {
        match assoc.container(db) {
            AssocItemContainer::Trait(t) => {
                return Some(format!(
                    "{}::{}::{name}",
                    module_path(db, t.module(db)),
                    t.name(db).as_str()
                ));
            }
            AssocItemContainer::Impl(i) => {
                let ty = i.self_ty(db).as_adt()?;
                let base = adt_id(db, ty);
                return Some(match i.trait_(db) {
                    // syn 정점은 해석된 이름이 아니라 impl에 쓰인 이름을 쓴다 —
                    // `use Tr as Alias` 뒤 `impl Alias for T`면 정점은
                    // `T::<Alias>::m`이다. 쓰인 세그먼트를 우선한다.
                    Some(t) => {
                        let tname = written_trait_name(sema, i)
                            .unwrap_or_else(|| t.name(db).as_str().to_string());
                        format!("{base}::<{tname}>::{name}")
                    }
                    None => format!("{base}::{name}"),
                });
            }
        }
    }
    Some(format!("{}::{name}", module_path(db, f.module(db))))
}

/// impl 구문에 쓰인 트레이트 경로의 마지막 세그먼트 — 별칭이면 hir의
/// 정식 이름 대신 이것이 syn 정점 ID의 트레이트 부분과 일치한다.
fn written_trait_name(sema: &Semantics<RootDatabase>, i: Impl) -> Option<String> {
    let ty = sema.source(i)?.value.trait_()?;
    let ast::Type::PathType(pt) = ty else {
        return None;
    };
    let seg = pt.path()?.segment()?;
    Some(seg.name_ref()?.ident_token()?.text().to_string())
}

/// 상수 정규 ID — 모듈 소속 상수만. 연관 상수(`impl T { const N }`)는
/// syn이 `m::N` 정점을 만들지 않으므로 정규 ID로 표현할 수 없다 —
/// 여기서 None을 주면 호출자가 그래프 밖(external)으로 센다.
fn const_id(db: &dyn HirDatabase, c: Const) -> Option<String> {
    if c.as_assoc_item(db).is_some() {
        return None;
    }
    Some(format!(
        "{}::{}",
        module_path(db, c.module(db)),
        c.name(db)?.as_str()
    ))
}

fn static_id(db: &dyn HirDatabase, s: Static) -> Option<String> {
    Some(format!(
        "{}::{}",
        module_path(db, s.module(db)),
        s.name(db).as_str()
    ))
}

fn trait_id(db: &dyn HirDatabase, t: Trait) -> String {
    format!("{}::{}", module_path(db, t.module(db)), t.name(db).as_str())
}

fn alias_id(db: &dyn HirDatabase, t: TypeAlias) -> String {
    format!("{}::{}", module_path(db, t.module(db)), t.name(db).as_str())
}

fn enum_id(db: &dyn HirDatabase, e: Enum) -> String {
    format!("{}::{}", module_path(db, e.module(db)), e.name(db).as_str())
}

fn macro_id(db: &dyn HirDatabase, m: Macro) -> String {
    format!("{}::{}", module_path(db, m.module(db)), m.name(db).as_str())
}

/// push의 결과 — 호출자가 실패 종류별로 다른 버킷에 센다.
enum Pushed {
    Yes,
    /// 자기 자신에게로 가는 간선 — 조용히 버린다(syn과 같은 규칙).
    SelfEdge,
    /// 해석은 됐지만 그래프에 정점이 없다 — 외부·생성 정의.
    Miss,
}

/// 본문 방문자 — ra 구문 트리를 걸으며 호출·참조를 간선으로 옮긴다.
/// `unsafe {}` 블록 안의 간선은 경계 진입으로 표시한다(syn 계약과 동일).
struct Walker<'a, 'b> {
    sema: &'a Semantics<'a, RootDatabase>,
    owner: &'a str,
    cfg: &'a Option<String>,
    ids: &'a BTreeSet<&'a str>,
    method_index: &'a BTreeMap<String, Vec<String>>,
    /// (트레이트 ID, 메서드 이름) → impl 메서드 후보 — 빌드 때 계산됐다.
    matrix: &'a BTreeMap<(String, String), Vec<String>>,
    st: &'b mut Stats,
    edges: Vec<Edge>,
}

impl<'a, 'b> Walker<'a, 'b> {
    /// DB 참조 — 'a 생명주기로 꺼내므로 self를 빌리지 않는다.
    /// 이걸 &self 생명주기로 주면 push 같은 &mut self와 겹쳐 빌드가 안 된다.
    fn db(&self) -> &'a dyn HirDatabase {
        self.sema.db
    }

    /// 간선 하나. tentative면 추정(디스패치 후보·이름 팬아웃), 아니면 확정.
    fn push(&mut self, to: String, kind: EdgeKind, tentative: bool, in_unsafe: bool) -> Pushed {
        if to == self.owner {
            return Pushed::SelfEdge;
        }
        if !self.ids.contains(to.as_str()) {
            return Pushed::Miss;
        }
        let mut e = Edge::new(self.owner.to_string(), to, kind);
        e.tentative = tentative;
        e.cfg = self.cfg.clone();
        e.unsafe_ = in_unsafe;
        self.edges.push(e);
        if !tentative {
            self.st.resolved += 1;
        }
        Pushed::Yes
    }

    /// 구문 트리 순회 — preorder, unsafe 블록 깊이를 따라간다.
    /// 매크로 호출은 확장 트리로도 내려간다(depth는 확장 재귀만 센다).
    /// 본문 안의 `아이템`(중첩 fn·impl·static 등)은 어떤 깊이에서도 들어가지
    /// 않는다 — 그 본문의 호출은 이 소유자의 것이 아니다. 다만 MacroCall은
    /// 아이템이면서 확장의 입구라 통과시킨다 — 중첩 매크로의 내부 호출을
    /// 수확하려면 확장으로 내려가는 길을 막으면 안 된다.
    fn walk(&mut self, node: &SyntaxNode, in_unsafe: bool, depth: usize) {
        if ast::Item::cast(node.clone()).is_some_and(|i| !matches!(i, ast::Item::MacroCall(_))) {
            return;
        }
        let un = in_unsafe
            || ast::BlockExpr::cast(node.clone()).is_some_and(|b| b.unsafe_token().is_some());
        if let Some(mc) = ast::MethodCallExpr::cast(node.clone()) {
            self.method_call(&mc, un);
        } else if let Some(call) = ast::CallExpr::cast(node.clone()) {
            self.call(&call, un);
        } else if let Some(re) = ast::RecordExpr::cast(node.clone()) {
            self.record(&re, un);
        } else if let Some(pe) = ast::PathExpr::cast(node.clone()) {
            self.path_ref(&pe, un);
        } else if let Some(p) = ast::PathPat::cast(node.clone()) {
            // `match` 패턴의 경로 — 상수·variant·타입 참조다.
            if let Some(path) = p.path() {
                self.emit_path(path, EdgeKind::References, un);
            }
        } else if let Some(p) = ast::RecordPat::cast(node.clone()) {
            if let Some(path) = p.path() {
                self.emit_path(path, EdgeKind::References, un);
            }
        } else if let Some(p) = ast::TupleStructPat::cast(node.clone()) {
            if let Some(path) = p.path() {
                self.emit_path(path, EdgeKind::References, un);
            }
        } else if let Some(mac) = ast::MacroCall::cast(node.clone()) {
            self.macro_call(&mac, un, depth);
        }
        for child in node.children() {
            self.walk(&child, un, depth);
        }
    }

    /// `x.m()` — 수신자 타입으로 정확히 해석한다. 해석되면 확정 간선,
    /// 트레이트 정의 메서드면 impl 행렬로, 끝까지 안 되면 이름 팬아웃.
    fn method_call(&mut self, mc: &ast::MethodCallExpr, un: bool) {
        if let Some(f) = self.sema.resolve_method_call(mc) {
            self.emit_function(f, EdgeKind::Call, un, self.receiver_is_concrete(mc));
            return;
        }
        // resolve_method_call_fallback은 dyn/제네릭에서도 트레이트 정의나
        // 필드를 돌려준다 — 필드면 호출이 아니니 버린다.
        // `m::<T>(..)`의 제네릭 인자까지 텍스트로 잡지 않게 토큰만 쓴다.
        let name = mc
            .name_ref()
            .and_then(|n| n.ident_token())
            .map(|t| t.text().to_string());
        match self.sema.resolve_method_call_fallback(mc) {
            // 필드 접근(left()==None)은 호출이 아니다 — 컴파일되지도 않는
            // 코드라 간선 없이 미해석으로만 센다.
            Some((res, _)) => {
                if let Some(f) = res.left() {
                    self.emit_function(f, EdgeKind::Call, un, self.receiver_is_concrete(mc));
                } else {
                    self.st.unresolved += 1;
                }
            }
            None => self.fan_out(name.as_deref(), un),
        }
    }

    /// 수신자 타입이 완전히 구체적인가 — dyn/impl Trait/제네릭이면
    /// 디스패치가 열려 있으므로 false다. `Box<dyn Tr>` 같은 스마트 포인터는
    /// 조정 전 타입이 ADT이므로 역참조·자동 참조가 반영된 조정 후 타입을 본다.
    fn receiver_is_concrete(&self, mc: &ast::MethodCallExpr) -> bool {
        mc.receiver()
            .and_then(|r| self.sema.type_of_expr(&r))
            .is_some_and(|t| self.type_is_concrete(&t.adjusted.unwrap_or(t.original)))
    }

    /// 타입이 완전히 구체적인가 — dyn·타입 파라미터·연관 타입·opaque·
    /// 미해석(infer 실패) 종류가 하나라도 섞이면 디스패치는 열려 있다.
    /// 겉이 ADT(Box 등)여도 인자 안에 그런 종류가 있으면 열린다 —
    /// `self: Box<Self>` 메서드는 조정 후에도 `Box<dyn Tr>` 형태가
    /// 유지되므로 겉 타입이 아니라 구성 타입 전부를 walk로 본다.
    /// 열린 종류를 나열하지 않고 구체로 확인된 종류만 허용한다 —
    /// `dyn Send`처럼 principal trait이 없어 `as_dyn_trait`가 못 잡는
    /// 객체나 미지의 kind도 안전한 쪽(열림)으로 떨어진다.
    fn type_is_concrete(&self, ty: &Type) -> bool {
        let mut concrete = true;
        ty.walk(self.db(), |t| {
            concrete &= t.as_adt().is_some()
                || t.as_builtin().is_some()
                || t.is_tuple()
                || t.is_slice()
                || t.is_array()
                || t.as_reference().is_some()
                || t.is_raw_ptr()
                || t.is_fn()
                || t.is_closure()
                || t.as_coroutine().is_some();
        });
        concrete
    }

    /// 해석된 함수를 간선으로 — 트레이트 정의 메서드는, 수신자가 구체
    /// 타입으로 확인됐으면(기본 구현을 그대로 상속) 정의점이 확정 타깃이고,
    /// 디스패치가 열려 있으면(dyn·제네릭) impl 행렬로 펼친다.
    fn emit_function(&mut self, f: Function, kind: EdgeKind, un: bool, concrete: bool) {
        let db = self.db();
        if let Some(t) = f.as_assoc_item(db).and_then(|a| a.container_trait(db)) {
            if concrete {
                return self.emit_fn_vertex(f, kind, un);
            }
            return self.trait_matrix(t, f, kind, un);
        }
        // 블록 지역 fn은 정규 ID가 없다 — 같은 이름의 정점으로 보내지 않는다.
        if block_local_def(self.sema, f) {
            self.st.external += 1;
            return;
        }
        self.emit_fn_vertex(f, kind, un)
    }

    /// 함수 정점으로의 확정 간선 — 정점이 없으면 외부 정의다.
    fn emit_fn_vertex(&mut self, f: Function, kind: EdgeKind, un: bool) {
        match fn_id(self.sema, f) {
            Some(id) => {
                if let Pushed::Miss = self.push(id, kind, false, un) {
                    self.st.external += 1;
                }
            }
            None => self.st.external += 1,
        }
    }

    /// 트레이트 정의 메서드 호출 — 수신자가 dyn/제네릭이라 실제 대상을 못
    /// 고른다. 워크스페이스의 impl 후보(빌드 때 만든 표)와 기본 구현
    /// 정의점에 추정 간선을 단다 — "살아 있다" 편향은 syn 팬아웃과 같은
    /// 계약이다.
    fn trait_matrix(&mut self, t: Trait, def_fn: Function, kind: EdgeKind, un: bool) {
        self.st.trait_sites += 1;
        let db = self.db();
        if let Some(id) = fn_id(self.sema, def_fn) {
            if let Pushed::Miss = self.push(id, kind, true, un) {
                self.st.external += 1;
            }
        }
        let key = (trait_id(db, t), def_fn.name(db).as_str().to_string());
        let Some(candidates) = self.matrix.get(&key) else {
            return;
        };
        for id in candidates {
            // impl 메서드 정점이 없으면(cfg·생성) 해석됐지만 그래프 밖이다.
            if let Pushed::Miss = self.push(id.clone(), kind, true, un) {
                self.st.external += 1;
            }
        }
    }

    /// ra도 못 잡은 메서드 호출 — syn과 같은 이름 팬아웃 폴백.
    /// 후보가 하나도 없으면 간선 없는 미해석이다 — 세지 않으면
    /// limitation이 거짓말을 한다.
    fn fan_out(&mut self, name: Option<&str>, un: bool) {
        let Some(name) = name else {
            self.st.unresolved += 1;
            return;
        };
        match self.method_index.get(name) {
            Some(ids) => {
                for mid in ids {
                    self.push(mid.clone(), EdgeKind::Call, true, un);
                }
                self.st.fanned += 1;
            }
            None => self.st.unresolved += 1,
        }
    }

    /// `f()`·`T::assoc()`·`S(..)` — 경로 호출. 호출자가 지역 바인딩이면
    /// (`let f = helper; f()`) 경로 해석은 Local을 주므로 callable 해석이
    /// 실제 대상(fn 아이템)까지 따라간다.
    fn call(&mut self, ce: &ast::CallExpr, un: bool) {
        let Some(func) = ce.expr() else { return };
        if let ast::Expr::PathExpr(pe) = &func {
            if let Some(path) = pe.path() {
                match self.sema.resolve_path(&path) {
                    Some(PathResolution::Local(_)) => {}
                    _ => {
                        self.emit_path(path, EdgeKind::Call, un);
                        return;
                    }
                }
            }
        }
        let Some(callable) = self.sema.resolve_expr_as_callable(&func) else {
            // 호출 대상이 완전히 불투명 — 간선 없이 미해석으로만 센다.
            self.st.unresolved += 1;
            return;
        };
        match callable.kind() {
            ra_ap_hir::CallableKind::Function(f) => {
                self.emit_function(f, EdgeKind::Call, un, false)
            }
            ra_ap_hir::CallableKind::TupleStruct(s) => {
                // 블록 지역 튜플 구조체의 정규 ID는 같은 이름의 정점과 충돌한다.
                if block_local_def(self.sema, Adt::Struct(s)) {
                    self.st.external += 1;
                } else {
                    let id = adt_id(self.db(), Adt::Struct(s));
                    if let Pushed::Miss = self.push(id, EdgeKind::Call, false, un) {
                        self.st.external += 1;
                    }
                }
            }
            ra_ap_hir::CallableKind::TupleEnumVariant(v) => {
                if block_local_def(self.sema, v) {
                    self.st.external += 1;
                } else {
                    let id = enum_id(self.db(), v.parent_enum(self.db()));
                    if let Pushed::Miss = self.push(id, EdgeKind::References, false, un) {
                        self.st.external += 1;
                    }
                }
            }
            // 클로저·fn 포인터·Fn 트레이트 객체 — 정적 해석 밖.
            _ => self.st.unresolved += 1,
        }
    }

    /// `S { .. }` 구조체 리터럴 — 생성자 참조다(syn과 같은 종류).
    fn record(&mut self, re: &ast::RecordExpr, un: bool) {
        if let Some(path) = re.path() {
            self.emit_path(path, EdgeKind::References, un);
        }
    }

    /// 호출이 아닌 경로 참조 — `filter_map(f)` 같은 함수 값·상수·타입.
    /// 지역 바인딩은 ra가 구분해준다 — 이름이 같아도 오인하지 않는다.
    fn path_ref(&mut self, pe: &ast::PathExpr, un: bool) {
        // 호출자 위치의 경로는 call()이 Call로 처리했다 — 이중 계수 금지.
        if pe
            .syntax()
            .parent()
            .is_some_and(|p| ast::CallExpr::cast(p).is_some())
        {
            return;
        }
        if let Some(path) = pe.path() {
            self.emit_path(path, EdgeKind::References, un);
        }
    }

    /// 경로 해석 결과를 간선으로. MethodCallExpr 안의 자체 참조는 위에서
    /// 처리했으므로 여기는 순수 경로다.
    fn emit_path(&mut self, path: ast::Path, kind: EdgeKind, un: bool) {
        match self.sema.resolve_path(&path) {
            Some(PathResolution::Def(d)) => self.emit_def(d, kind, un),
            // `Self::x` — impl의 self 타입으로의 참조다.
            Some(PathResolution::SelfType(imp)) => {
                // 블록 지역 impl의 self 타입도 정규 ID 충돌이 가능하다.
                if block_local_def(self.sema, imp) {
                    self.st.external += 1;
                } else if let Some(adt) = imp.self_ty(self.db()).as_adt() {
                    let id = adt_id(self.db(), adt);
                    if let Pushed::Miss = self.push(id, EdgeKind::References, false, un) {
                        self.st.external += 1;
                    }
                }
            }
            // 지역 바인딩·타입 파라미터·derive 헬퍼 — 그래프 정점이 아니다.
            Some(_) => {}
            None => self.st.unresolved += 1,
        }
    }

    /// ModuleDef → 정규 ID → 간선. 해석됐는데 정점이 없으면 외부다.
    fn emit_def(&mut self, d: ModuleDef, kind: EdgeKind, un: bool) {
        let db = self.db();
        let id = match d {
            ModuleDef::Function(f) => return self.emit_function(f, kind, un, false),
            ModuleDef::Adt(a) => adt_id(db, a),
            ModuleDef::EnumVariant(v) => enum_id(db, v.parent_enum(db)),
            ModuleDef::Const(c) => match const_id(db, c) {
                Some(id) => id,
                // 연관 상수 — 정점 스킴이 없어 외부로 센다.
                None => {
                    self.st.external += 1;
                    return;
                }
            },
            ModuleDef::Static(s) => static_id(db, s).unwrap_or_default(),
            ModuleDef::Trait(t) => trait_id(db, t),
            ModuleDef::TypeAlias(t) => alias_id(db, t),
            ModuleDef::Module(m) => module_path(db, m),
            ModuleDef::Macro(m) => macro_id(db, m),
            // prelude 내장 타입 — 그래프 정점이 아니다.
            ModuleDef::BuiltinType(_) => return,
        };
        // 블록 지역 정의의 정규 ID는 같은 이름의 정점과 충돌할 수 있다 —
        // 확인된 간선을 만들지 않고 그래프 밖으로 본다.
        if self.def_block_local(d) {
            self.st.external += 1;
            return;
        }
        if let Pushed::Miss = self.push(id, kind, false, un) {
            self.st.external += 1;
        }
    }

    /// 정의가 본문 안에 선언된 블록 지역 정의인가 — Module은 여러 선언
    /// 지점이 있어 소스를 하나로 못 잡으므로 false(보수적으로 통과).
    fn def_block_local(&self, d: ModuleDef) -> bool {
        let sema = self.sema;
        match d {
            ModuleDef::Adt(Adt::Struct(x)) => block_local_def(sema, x),
            ModuleDef::Adt(Adt::Enum(x)) => block_local_def(sema, x),
            ModuleDef::Adt(Adt::Union(x)) => block_local_def(sema, x),
            ModuleDef::EnumVariant(x) => block_local_def(sema, x),
            ModuleDef::Const(x) => block_local_def(sema, x),
            ModuleDef::Static(x) => block_local_def(sema, x),
            ModuleDef::Trait(x) => block_local_def(sema, x),
            ModuleDef::TypeAlias(x) => block_local_def(sema, x),
            ModuleDef::Macro(x) => block_local_def(sema, x),
            _ => false,
        }
    }

    /// `name!()` — 크레이트 안 매크로면 call 간선, 외부면 실측 생략.
    /// 그리고 확장 트리로 내려가 확장 안의 호출·참조를 수확한다 —
    /// 이것이 syn의 토큰 파싱 폴백을 대체하는 근본적 개선이다.
    fn macro_call(&mut self, mac: &ast::MacroCall, un: bool, depth: usize) {
        if let Some(m) = self.sema.resolve_macro_call(mac) {
            let id = macro_id(self.db(), m);
            if let Pushed::Miss = self.push(id, EdgeKind::Call, false, un) {
                // 해석됐지만 정점이 없는 매크로 = std/외부 매크로.
                self.st.ext_macros += 1;
            }
        } else {
            self.st.ext_macros += 1;
        }
        if depth >= MAX_EXPANSION_DEPTH {
            self.st.unexpanded += 1;
            return;
        }
        // expand는 ValueResult — 구문이 나와도 오류가 실리면 부분 확장이다.
        // 조각 안의 호출은 진짜라 걷되, 성공으로는 세지 않는다.
        let Some(call_id) = self.sema.to_def(mac) else {
            self.st.unexpanded += 1;
            return;
        };
        let res = self.sema.expand(call_id);
        if res.err.is_some() {
            self.st.unexpanded += 1;
        } else {
            self.st.expanded += 1;
        }
        self.walk(&res.value, un, depth + 1);
    }
}
