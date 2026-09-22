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
    Module, ModuleDef, PathResolution, Semantics, Static, Trait, TypeAlias,
};
use ra_ap_ide_db::RootDatabase;
use ra_ap_load_cargo::{load_workspace_at, LoadCargoConfig, ProcMacroServerChoice};
use ra_ap_proc_macro_api::ProcMacroClient;
use ra_ap_project_model::CargoConfig;
use ra_ap_syntax::ast::{self, AstNode};
use ra_ap_syntax::SyntaxNode;
use ra_ap_vfs::Vfs;
use std::collections::{BTreeMap, BTreeSet};
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
    /// db는 파일 내용을 salsa 입력으로 들고 있어 vfs 자체는 쓰지 않지만,
    /// proc 매크로 서버는 외부 프로세스라 클라이언트를 살려둬야 확장이 동작한다.
    _vfs: Vfs,
    proc_macro: Option<ProcMacroClient>,
    /// 정규 ID → 본문 소유자 정의(fn·const·static). 워크스페이스 크레이트만.
    defs: BTreeMap<String, BodyDef>,
}

/// 본문을 가질 수 있는 정의 — fn·const·static.
#[derive(Clone, Copy)]
enum BodyDef {
    Fn(Function),
    Const(Const),
    Static(Static),
}

impl Engine {
    /// `dir`의 cargo 워크스페이스를 의미 DB로 로드하고 정의 인덱스를 만든다.
    /// `cargo check`는 돌리지 않는다(load_out_dirs_from_check: false) —
    /// 빌드 스크립트 산출물(OUT_DIR·include!)은 해석 밖에 남는다.
    /// 로드 실패는 오류 — syn으로 조용히 떨어지면 --semantic이 거짓말이 된다.
    pub fn load(dir: &Path) -> Result<Engine, String> {
        let cargo_config = CargoConfig::default();
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
        let defs = ra_ap_hir::attach_db(&db, || build_index(&db));
        Ok(Engine {
            db,
            _vfs: vfs,
            proc_macro,
            defs,
        })
    }

    /// proc 매크로 서버가 붙었는가 — 없으면 proc 매크로 호출은 unexpanded로 간다.
    pub fn has_proc_macros(&self) -> bool {
        self.proc_macro.is_some()
    }

    /// 본문 하나를 의미 해석으로 걷는다 — 호출·참조·매크로 간선.
    /// None = hir이 이 정의를 모른다(cfg 비활성·매크로 생성) — 호출자는
    /// syn 본문 방문으로 되돌아간다. 시그니처 간선은 항상 syn 경로다.
    pub fn body_edges(
        &self,
        owner: &str,
        cfg: &Option<String>,
        ids: &BTreeSet<&str>,
        method_index: &BTreeMap<String, Vec<String>>,
        st: &mut Stats,
    ) -> Option<Vec<Edge>> {
        let def = *self.defs.get(owner)?;
        let sema = Semantics::new(&self.db);
        let root = def.body_root(&sema)?;
        // next-solver의 트레이트 해석은 스레드 로컬 attached db를 요구한다 —
        // resolve_method_call 안에서 panic 나지 않게 걷기 전에 붙인다.
        let mut w = Walker {
            sema: &sema,
            owner,
            cfg,
            ids,
            method_index,
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
fn build_index(db: &RootDatabase) -> BTreeMap<String, BodyDef> {
    let mut defs: BTreeMap<String, BodyDef> = BTreeMap::new();
    let mut stack: Vec<Module> = Vec::new();
    for krate in Crate::all(db) {
        if !krate.origin(db).is_local() {
            continue;
        }
        stack.push(krate.root_module(db));
        while let Some(module) = stack.pop() {
            for def in module.declarations(db) {
                match def {
                    ModuleDef::Function(f) => push_fn(db, &mut defs, f),
                    ModuleDef::Const(c) => {
                        if let Some(id) = const_id(db, c) {
                            defs.insert(id, BodyDef::Const(c));
                        }
                    }
                    ModuleDef::Static(s) => {
                        if let Some(id) = static_id(db, s) {
                            defs.insert(id, BodyDef::Static(s));
                        }
                    }
                    ModuleDef::Trait(t) => {
                        // 트레이트 기본 메서드 — 선언점 메서드 정점의 본문.
                        for item in t.items(db) {
                            if let AssocItem::Function(f) = item {
                                push_fn(db, &mut defs, f);
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
            for item in imp.items(db) {
                if let AssocItem::Function(f) = item {
                    push_fn(db, &mut defs, f);
                }
            }
        }
    }
    defs
}

impl BodyDef {
    /// 정의의 구문 루트 — 타입 자리(시그니처)는 walker가 Type 노드를
    /// 건드리지 않으므로 아이템 전체를 돌려도 본문만 잡힌다.
    fn body_root(&self, sema: &Semantics<RootDatabase>) -> Option<SyntaxNode> {
        match self {
            BodyDef::Fn(f) => Some(sema.source(*f)?.value.syntax().clone()),
            BodyDef::Const(c) => Some(sema.source(*c)?.value.syntax().clone()),
            BodyDef::Static(s) => Some(sema.source(*s)?.value.syntax().clone()),
        }
    }
}

/// 함수 한 개를 ID 계산해 entries에 넣는다 — 클로저로 쓰면 entries와
/// db 빌림이 얽혀서 자유 함수다.
fn push_fn(db: &dyn HirDatabase, defs: &mut BTreeMap<String, BodyDef>, f: Function) {
    if let Some(id) = fn_id(db, f) {
        defs.insert(id, BodyDef::Fn(f));
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
fn fn_id(db: &dyn HirDatabase, f: Function) -> Option<String> {
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
                    Some(t) => format!("{base}::<{}>::{name}", t.name(db).as_str()),
                    None => format!("{base}::{name}"),
                });
            }
        }
    }
    Some(format!("{}::{name}", module_path(db, f.module(db))))
}

fn const_id(db: &dyn HirDatabase, c: Const) -> Option<String> {
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
    /// 단, 확장 안의 `아이템`(생성된 fn·impl 등)은 들어가지 않는다 — 그
    /// 본문의 호출은 호출자의 것이 아니라 생성된 정의의 것이므로, 정점도
    /// 없는 생성 정의에 귀속시키면 간선이 거짓이 된다.
    fn walk(&mut self, node: &SyntaxNode, in_unsafe: bool, depth: usize) {
        if depth > 0 && ast::Item::cast(node.clone()).is_some() {
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
            self.emit_function(f, EdgeKind::Call, un);
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
            // 필드 접근(left()==None)은 호출이 아니니 버린다.
            Some((res, _)) => {
                if let Some(f) = res.left() {
                    self.emit_function(f, EdgeKind::Call, un);
                }
            }
            None => self.fan_out(name.as_deref(), un),
        }
    }

    /// 해석된 함수를 간선으로 — 트레이트 정의 메서드면 impl 행렬로 펼친다.
    fn emit_function(&mut self, f: Function, kind: EdgeKind, un: bool) {
        let db = self.db();
        if let Some(t) = f.as_assoc_item(db).and_then(|a| a.container_trait(db)) {
            return self.trait_matrix(t, f, kind, un);
        }
        match fn_id(db, f) {
            Some(id) => {
                if let Pushed::Miss = self.push(id, kind, false, un) {
                    self.st.external += 1;
                }
            }
            None => self.st.external += 1,
        }
    }

    /// 트레이트 정의 메서드 호출 — 수신자가 dyn/제네릭이라 실제 대상을 못
    /// 고른다. 워크스페이스의 모든 impl 후보 + 기본 구현 정의점에 추정
    /// 간선을 단다 — "살아 있다" 편향은 syn 팬아웃과 같은 계약이다.
    fn trait_matrix(&mut self, t: Trait, def_fn: Function, kind: EdgeKind, un: bool) {
        self.st.trait_sites += 1;
        let db = self.db();
        if let Some(id) = fn_id(db, def_fn) {
            self.push(id, kind, true, un);
        }
        let name = def_fn.name(db);
        for imp in Impl::all_for_trait(db, t) {
            for item in imp.items(db) {
                let AssocItem::Function(mf) = item else {
                    continue;
                };
                if mf.name(db) != name {
                    continue;
                }
                if let Some(id) = fn_id(db, mf) {
                    self.push(id, kind, true, un);
                }
            }
        }
    }

    /// ra도 못 잡은 메서드 호출 — syn과 같은 이름 팬아웃 폴백.
    fn fan_out(&mut self, name: Option<&str>, un: bool) {
        let Some(name) = name else { return };
        if let Some(ids) = self.method_index.get(name) {
            for mid in ids.clone() {
                self.push(mid, EdgeKind::Call, true, un);
            }
            self.st.fanned += 1;
        }
    }

    /// `f()`·`T::assoc()`·`S(..)` — 경로 호출. 비경로 호출자(클로저·
    /// fn 포인터)는 callable 해석으로 한 번 더 시도한다.
    fn call(&mut self, ce: &ast::CallExpr, un: bool) {
        let Some(func) = ce.expr() else { return };
        if let ast::Expr::PathExpr(pe) = &func {
            if let Some(path) = pe.path() {
                self.emit_path(path, EdgeKind::Call, un);
            }
            return;
        }
        if let Some(callable) = self.sema.resolve_expr_as_callable(&func) {
            match callable.kind() {
                ra_ap_hir::CallableKind::Function(f) => self.emit_function(f, EdgeKind::Call, un),
                ra_ap_hir::CallableKind::TupleStruct(s) => {
                    let id = adt_id(self.db(), Adt::Struct(s));
                    if let Pushed::Miss = self.push(id, EdgeKind::Call, false, un) {
                        self.st.external += 1;
                    }
                }
                ra_ap_hir::CallableKind::TupleEnumVariant(v) => {
                    let id = enum_id(self.db(), v.parent_enum(self.db()));
                    if let Pushed::Miss = self.push(id, EdgeKind::References, false, un) {
                        self.st.external += 1;
                    }
                }
                // 클로저·fn 포인터·Fn 트레이트 객체 — 정적 해석 밖.
                _ => {}
            }
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
                if let Some(adt) = imp.self_ty(self.db()).as_adt() {
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
            ModuleDef::Function(f) => return self.emit_function(f, kind, un),
            ModuleDef::Adt(a) => adt_id(db, a),
            ModuleDef::EnumVariant(v) => enum_id(db, v.parent_enum(db)),
            ModuleDef::Const(c) => match const_id(db, c) {
                Some(id) => id,
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
        if let Pushed::Miss = self.push(id, kind, false, un) {
            self.st.external += 1;
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
            return;
        }
        match self.sema.expand_macro_call(mac) {
            Some(exp) => {
                self.st.expanded += 1;
                self.walk(&exp.value, un, depth + 1);
            }
            None => self.st.unexpanded += 1,
        }
    }
}
