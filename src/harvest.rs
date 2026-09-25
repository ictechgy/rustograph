//! syn AST에서 정점과 간선을 수확한다.
//!
//! 구문 수준 수확이라 할 수 있는 것과 없는 것을 구분한다: 경로 호출·use·impl은
//! 정확히 해석되고, 메서드 호출(`x.m()`)은 타입이 없어 이름 팬아웃으로
//! 과대 근사한다 — 오탐은 "살아 있다" 쪽으로만 기울게 하는 계약이다.

use crate::graph::{Edge, EdgeKind, Kind, Vertex};
use crate::modtree::{cfg_of, DepCrates, ModTree};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use syn::parse::Parser;
use syn::spanned::Spanned;
use syn::visit::Visit;

/// 수확 중간 산출물 — 문서 조립 전의 실측 카운터.
#[derive(Default)]
pub struct Harvest {
    /// 해석에 실패한 경로 수 — 외부 참조 limitation의 실측값.
    pub unresolved_paths: usize,
    /// 이름 팬아웃으로 해석한 메서드 호출 수.
    pub fanned_method_calls: usize,
    /// 외부/std 매크로 호출 수(유령 정점 없이 생략).
    pub external_macros: usize,
    /// cfg 조건부로 포함한 아이템 수.
    pub cfg_items: usize,
    /// 속성 목록을 Meta로 읽지 못한 cfg_attr 수 — 안쪽 경로가 참조로
    /// 잡히지 않았으니 속성으로만 쓰는 dep이 미사용으로 보일 수 있다.
    pub unparsed_attrs: usize,
}

/// 한 모듈의 아이템 목록을 정점으로 만든다(1패스 — 선언만).
/// 반환값: (이 모듈이 만든 정점들, 이 모듈의 impl 블록들).
pub struct ModuleDecls<'a> {
    pub vertices: Vec<Vertex>,
    pub impls: Vec<ImplBlock>,
    /// fn/method/const/static/macro 본문을 담은 항목(2패스용).
    pub bodies: Vec<BodyItem<'a>>,
    /// 아이템 속성의 경로 참조 — `#[dep::attr]`, `#[derive(dep::X)]`.
    /// 해석은 임포트가 채워진 뒤 2패스에서 한다.
    pub attr_refs: Vec<AttrRef>,
    /// #[no_mangle]·proc_macro 같은 외부 호출 진입점 — 항상 보존 루트.
    pub entry_roots: Vec<String>,
    /// #[test]/#[bench] 진입점 — --tests일 때만 보존 루트가 된다.
    pub test_roots: Vec<String>,
}

/// 아이템 속성이 담은 경로 참조 하나.
/// `#[fixture_macros::keep]`나 `#[derive(serde::Serialize)]`는 그 크레이트의
/// 실제 사용이다 — 모으지 않으면 속성으로만 쓰는 dep이 미사용으로 오보된다.
pub struct AttrRef {
    /// 참조를 단 아이템의 정점 ID — 아이템 정점이 없으면 소유 모듈.
    pub owner: String,
    /// owner가 `mod x;` 선언인가 — 파일이 없어 모듈이 트리에 없으면
    /// owner 정점은 존재하지 않으니 선언 모듈로 폴백해야 한다.
    pub mod_decl: bool,
    /// 참조를 해석할 모듈 — 임포트는 모듈 스코프에 산다.
    pub module: String,
    /// 속성 경로의 세그먼트(`fixture_macros::keep` → ["fixture_macros","keep"]).
    pub path: Vec<String>,
    /// `#[cfg_attr(pred, attr)]` 안쪽 속성은 술어 아래서만 성립한다.
    pub cfg: Option<String>,
}

/// impl 블록 — self 타입·트레이트·메서드 목록.
#[derive(Clone)]
pub struct ImplBlock {
    pub self_ty: Vec<String>,
    pub trait_path: Option<Vec<String>>,
    pub methods: Vec<syn::ImplItemFn>,
    pub items_module: String,
    /// `#[cfg]`가 붙은 impl — 메서드 정점과 그 본문 간선이 조건을 물려받는다.
    pub cfg: Option<String>,
    /// `unsafe impl` — 이 구현이 만드는 정점·implements 간선은 경계의 일부다.
    pub unsafe_: bool,
    /// impl이 실제로 선언된 파일 — lib/bin 합본 루트는 아이템마다 파일이
    /// 다르므로 모듈의 대표 파일이 아니라 블록 자신의 파일을 들고 다닌다.
    pub file: PathBuf,
    /// 선언 파일의 생성 코드 마커 — 메서드 정점의 generated 플래그.
    pub generated: bool,
}

/// 본문을 나중에 방문할 항목.
pub struct BodyItem<'a> {
    pub id: String,
    pub module: String,
    /// Self가 가리키는 타입(impl 안이면 Some).
    pub self_ty: Option<Vec<String>>,
    /// 블록의 표현식들 — 문 위치 매크로는 ExprMacro로 감싸져 있다.
    pub exprs: Vec<syn::Expr>,
    pub signature_surface: Vec<&'a syn::Type>,
    /// 소유 아이템의 `#[cfg]` — 이 본문이 만드는 간선 전부가 그 조건 아래 있다.
    pub cfg: Option<String>,
    /// 소유 아이템이 선언된 파일 — semantic 엔진이 소스 정체를 맞출 때 쓴다.
    pub file: PathBuf,
    /// 소유 아이템의 바이트 범위 — cfg 변형·블록 지역 정의 같은
    /// 정규 ID 충돌을 소스 위치로 걸러내는 데 쓴다.
    pub range: std::ops::Range<usize>,
}

/// 블록 `{ ... }`의 구문들을 표현식 목록으로 펼친다.
/// `println!(..)` 같은 문 위치 매크로는 syn이 Stmt::Macro로 파싱한다 —
/// ExprMacro로 감싸지 않으면 토큰 속 호출이 통째로 사라진다.
fn block_exprs(b: &syn::Block) -> Vec<syn::Expr> {
    let mut out = Vec::new();
    for s in &b.stmts {
        match s {
            syn::Stmt::Expr(e, _) => out.push(e.clone()),
            syn::Stmt::Local(l) => {
                if let Some(i) = &l.init {
                    out.push((*i.expr).clone());
                }
            }
            syn::Stmt::Macro(m) => out.push(syn::Expr::Macro(syn::ExprMacro {
                attrs: m.attrs.clone(),
                mac: m.mac.clone(),
            })),
            _ => {}
        }
    }
    out
}

/// Option<&Block>의 map에 바로 쓰기 위한 참조 어댑터 — 클로저 감쌈을 피한다.
fn block_exprs_ref(b: &syn::Block) -> Vec<syn::Expr> {
    block_exprs(b)
}

/// 모듈의 파일 AST를 1패스로 돌려 선언 정점을 수확한다.
/// `groups`는 (선언 파일, 그 파일의 아이템 목록) — lib/bin 합본 루트처럼
/// 한 모듈의 아이템이 여러 파일에 걸칠 수 있어 위치·본문 정체는 아이템의
/// 실제 파일을 따라야 한다.
pub fn decls<'a>(
    module_path: &str,
    krate: &str,
    groups: &[(PathBuf, &'a [syn::Item])],
    harvest: &mut Harvest,
) -> ModuleDecls<'a> {
    let mut out = ModuleDecls {
        vertices: Vec::new(),
        impls: Vec::new(),
        bodies: Vec::new(),
        attr_refs: Vec::new(),
        entry_roots: Vec::new(),
        test_roots: Vec::new(),
    };
    for (file, items) in groups {
        let generated = file_has_generated_marker(file);
        for item in items.iter() {
            let cfg = cfg_of(attrs_of(item));
            if cfg.is_some() {
                harvest.cfg_items += 1;
            }
            let pos = position_of(file, item);
            let v = |id: String, kind: Kind, exported: bool, unsafe_: bool| Vertex {
                id,
                kind,
                krate: krate.to_string(),
                module: module_path.to_string(),
                position: pos.clone(),
                exported,
                generated,
                cfg: cfg.clone(),
                unsafe_,
            };
            match item {
                syn::Item::Fn(f) => {
                    let id = format!("{module_path}::{}", f.sig.ident);
                    let exprs = block_exprs(&f.block);
                    // unsafe fn이거나 본문에 unsafe 블록이 있으면 경계의 안쪽이다.
                    let unsafe_ = f.sig.unsafety.is_some() || exprs_have_unsafe(&exprs);
                    out.vertices
                        .push(v(id.clone(), Kind::Fn, is_pub(&f.vis), unsafe_));
                    if is_extern_entry(&f.attrs) {
                        out.entry_roots.push(id.clone());
                    } else if is_test_entry(&f.attrs) {
                        out.test_roots.push(id.clone());
                    }
                    out.bodies.push(BodyItem {
                        id,
                        module: module_path.to_string(),
                        self_ty: None,
                        exprs,
                        signature_surface: fn_signature_types(&f.sig),
                        cfg: cfg.clone(),
                        file: file.to_path_buf(),
                        range: f.span().byte_range(),
                    });
                }
                syn::Item::Struct(s) => {
                    out.vertices.push(v(
                        format!("{module_path}::{}", s.ident),
                        Kind::Struct,
                        is_pub(&s.vis),
                        false,
                    ));
                }
                syn::Item::Enum(e) => {
                    out.vertices.push(v(
                        format!("{module_path}::{}", e.ident),
                        Kind::Enum,
                        is_pub(&e.vis),
                        false,
                    ));
                }
                syn::Item::Trait(t) => {
                    // unsafe trait — 구현·사용이 전부 경계를 넘는다.
                    out.vertices.push(v(
                        format!("{module_path}::{}", t.ident),
                        Kind::Trait,
                        is_pub(&t.vis),
                        t.unsafety.is_some(),
                    ));
                    // 트레이트 기본 메서드는 트레이트 소유 메서드 정점이다.
                    for ti in &t.items {
                        if let syn::TraitItem::Fn(m) = ti {
                            let mid = format!("{module_path}::{}::{}", t.ident, m.sig.ident);
                            let exprs = m.default.as_ref().map(block_exprs_ref).unwrap_or_default();
                            let unsafe_ = m.sig.unsafety.is_some() || exprs_have_unsafe(&exprs);
                            out.vertices.push(v(
                                mid.clone(),
                                Kind::Method,
                                is_pub(&t.vis),
                                unsafe_,
                            ));
                            if m.default.is_some() {
                                out.bodies.push(BodyItem {
                                    id: mid,
                                    module: module_path.to_string(),
                                    self_ty: None,
                                    exprs,
                                    signature_surface: fn_signature_types(&m.sig),
                                    cfg: cfg.clone(),
                                    file: file.to_path_buf(),
                                    range: m.span().byte_range(),
                                });
                            }
                        }
                    }
                }
                syn::Item::Union(u) => {
                    out.vertices.push(v(
                        format!("{module_path}::{}", u.ident),
                        Kind::Union,
                        is_pub(&u.vis),
                        false,
                    ));
                }
                syn::Item::Type(t) => {
                    out.vertices.push(v(
                        format!("{module_path}::{}", t.ident),
                        Kind::TypeAlias,
                        is_pub(&t.vis),
                        false,
                    ));
                }
                syn::Item::Const(c) => {
                    let id = format!("{module_path}::{}", c.ident);
                    let exprs = vec![(*c.expr).clone()];
                    let unsafe_ = exprs_have_unsafe(&exprs);
                    out.vertices
                        .push(v(id.clone(), Kind::Const, is_pub(&c.vis), unsafe_));
                    out.bodies.push(BodyItem {
                        id,
                        module: module_path.to_string(),
                        self_ty: None,
                        exprs,
                        signature_surface: vec![&c.ty],
                        cfg: cfg.clone(),
                        file: file.to_path_buf(),
                        range: c.span().byte_range(),
                    });
                }
                syn::Item::Static(s) => {
                    let id = format!("{module_path}::{}", s.ident);
                    let exprs = vec![(*s.expr).clone()];
                    // static mut 초기화의 unsafe 블록도 경계다.
                    let unsafe_ = exprs_have_unsafe(&exprs);
                    out.vertices
                        .push(v(id.clone(), Kind::Static, is_pub(&s.vis), unsafe_));
                    out.bodies.push(BodyItem {
                        id,
                        module: module_path.to_string(),
                        self_ty: None,
                        exprs,
                        signature_surface: vec![&s.ty],
                        cfg: cfg.clone(),
                        file: file.to_path_buf(),
                        range: s.span().byte_range(),
                    });
                }
                syn::Item::Macro(m) => {
                    if let Some(id) = &m.ident {
                        if m.mac.path.is_ident("macro_rules") {
                            out.vertices.push(v(
                                format!("{module_path}::{id}"),
                                Kind::Macro,
                                true,
                                false,
                            ));
                        }
                    }
                }
                syn::Item::Impl(i) => {
                    out.impls.push(ImplBlock {
                        self_ty: type_path(&i.self_ty),
                        trait_path: i.trait_.as_ref().map(|(_, p, _)| path_segments(p)),
                        methods: i
                            .items
                            .iter()
                            .filter_map(|x| match x {
                                syn::ImplItem::Fn(f) => Some(f.clone()),
                                _ => None,
                            })
                            .collect(),
                        items_module: module_path.to_string(),
                        cfg: cfg.clone(),
                        unsafe_: i.unsafety.is_some(),
                        file: file.clone(),
                        generated,
                    });
                }
                _ => {}
            }
            // 아이템 속성의 경로 참조 — #[dep::attr]·#[derive(dep::X)]는
            // 그 크레이트의 실제 사용 증거다. impl 자체의 속성도 여기서 잡되
            // owner는 모듈이다(self 타입 정점은 아직 해석 전).
            // `mod x;` 선언은 파일이 없으면 정점이 안 만들어지므로
            // mod_decl 표시를 남긴다 — attr_edges가 선언 모듈로 폴백한다.
            let owner = item
                .ident()
                .map(|i| format!("{module_path}::{i}"))
                .unwrap_or_else(|| module_path.to_string());
            let mod_decl = matches!(item, syn::Item::Mod(m) if m.content.is_none());
            harvest.unparsed_attrs += collect_attr_refs(
                attrs_of(item),
                &owner,
                module_path,
                mod_decl,
                &cfg,
                &mut out.attr_refs,
            );
            if let syn::Item::Macro(m) = item {
                // 아이템 위치의 매크로 호출 — `dep::mac!()` 경로 자체가 참조다.
                let segs = path_segments(&m.mac.path);
                if m.ident.is_none() && segs.len() >= 2 {
                    out.attr_refs.push(AttrRef {
                        owner: owner.clone(),
                        mod_decl: false,
                        module: module_path.to_string(),
                        path: segs,
                        cfg: cfg.clone(),
                    });
                }
            }
        }
    }
    out
}

/// impl 블록들을 정점·간선으로 변환한다(1패스 후속).
/// 반환: (정점, 간선, 속성 경로 참조, 2패스용 본문 목록).
pub fn impls<'a>(
    blocks: &'a [ImplBlock],
    krate: &str,
    tree: &ModTree,
    harvest: &mut Harvest,
) -> (Vec<Vertex>, Vec<Edge>, Vec<AttrRef>, Vec<BodyItem<'a>>) {
    let mut vertices = Vec::new();
    let mut edges = Vec::new();
    let mut attr_refs = Vec::new();
    let mut bodies = Vec::new();
    for b in blocks {
        // self 타입을 해석한다 — 모듈 로컬이면 정점이 있다.
        let Some(self_id) = tree.resolve(&b.items_module, &b.self_ty, &DepCrates::new()) else {
            harvest.unresolved_paths += 1;
            continue;
        };
        if let Some(tp) = &b.trait_path {
            if let Some(trait_id) = tree.resolve(&b.items_module, tp, &DepCrates::new()) {
                // unsafe impl의 implements는 경계의 일부다.
                let mut e = Edge::new(self_id.clone(), trait_id.clone(), EdgeKind::Implements);
                e.cfg = b.cfg.clone();
                e.unsafe_ = b.unsafe_;
                edges.push(e);
            } else {
                harvest.unresolved_paths += 1;
            }
        }
        for m in &b.methods {
            // 트레이트 impl 메서드는 `Type::<Trait>::m` — 고유 선언과 충돌 안 함.
            let mid = match &b.trait_path {
                Some(tp) => {
                    let tname = tp.last().cloned().unwrap_or_default();
                    format!("{self_id}::<{tname}>::{}", m.sig.ident)
                }
                None => format!("{self_id}::{}", m.sig.ident),
            };
            let exprs = block_exprs(&m.block);
            // unsafe impl의 메서드, unsafe fn 메서드, unsafe 본문 모두 경계다.
            let unsafe_ = b.unsafe_ || m.sig.unsafety.is_some() || exprs_have_unsafe(&exprs);
            vertices.push(Vertex {
                id: mid.clone(),
                kind: Kind::Method,
                krate: krate.to_string(),
                module: b.items_module.clone(),
                position: Some(format!("{}:{}", b.file.display(), line_of(&m.sig.ident))),
                exported: matches!(m.vis, syn::Visibility::Public(_)),
                generated: b.generated,
                cfg: b.cfg.clone(),
                unsafe_,
            });
            let mut contains = Edge::new(self_id.clone(), mid.clone(), EdgeKind::Contains);
            contains.cfg = b.cfg.clone();
            contains.unsafe_ = b.unsafe_;
            edges.push(contains);
            // 메서드 속성의 경로 참조 — #[dep::attr] fn m()도 사용 증거다.
            // impl과 메서드 자신의 cfg를 둘 다 물린다 — 둘 다 성립해야
            // 이 참조가 존재한다.
            let mcfg = match (&b.cfg, cfg_of(&m.attrs)) {
                (a, Some(b)) => conjoin(a, b.as_str()),
                (a, None) => a.clone(),
            };
            harvest.unparsed_attrs += collect_attr_refs(
                &m.attrs,
                &mid,
                &b.items_module,
                false,
                &mcfg,
                &mut attr_refs,
            );
            bodies.push(BodyItem {
                id: mid,
                module: b.items_module.clone(),
                self_ty: Some(b.self_ty.clone()),
                exprs,
                signature_surface: fn_signature_types(&m.sig),
                cfg: b.cfg.clone(),
                file: b.file.clone(),
                range: m.span().byte_range(),
            });
        }
    }
    (vertices, edges, attr_refs, bodies)
}

/// 시그니처 표면(파라미터·반환·where)의 타입 목록.
fn fn_signature_types(sig: &syn::Signature) -> Vec<&syn::Type> {
    let mut types: Vec<&syn::Type> = Vec::new();
    for arg in &sig.inputs {
        if let syn::FnArg::Typed(t) = arg {
            types.push(&t.ty);
        }
    }
    if let syn::ReturnType::Type(_, t) = &sig.output {
        types.push(t);
    }
    types
}

/// 모은 속성 경로 참조를 references 간선으로 해석한다(2패스).
/// 해석 실패는 unresolved_paths로 센다 — 외부 속성 경로는 그 자체로
/// 미해석 참조다. owner와 목적지가 같으면 자기 참조라 버린다.
pub fn attr_edges(
    refs: &[AttrRef],
    tree: &ModTree,
    dep_crates: &DepCrates,
    harvest: &mut Harvest,
) -> Vec<Edge> {
    let mut edges = Vec::new();
    for r in refs {
        // `mod x;` 선언에 단 속성의 owner는 모듈 정점 — 파일이 없어
        // 모듈이 트리에 없으면 그 정점은 존재하지 않으니 선언 모듈로
        // 폴백한다. 없는 정점에서 간선을내면 유령이 된다.
        let owner = if r.mod_decl && !tree.modules.contains_key(r.owner.as_str()) {
            r.module.as_str()
        } else {
            r.owner.as_str()
        };
        match tree.resolve(&r.module, &r.path, dep_crates) {
            Some(to) if to != owner => {
                let mut e = Edge::new(owner.to_string(), to, EdgeKind::References);
                e.cfg = r.cfg.clone();
                edges.push(e);
            }
            Some(_) => {}
            None => harvest.unresolved_paths += 1,
        }
    }
    edges
}

/// 본문을 방문해 call/references/매크로 간선을 만든다(2패스).
pub fn bodies(
    items: &[BodyItem],
    tree: &ModTree,
    dep_crates: &DepCrates,
    method_index: &BTreeMap<String, Vec<String>>,
    harvest: &mut Harvest,
) -> Vec<Edge> {
    let mut edges = Vec::new();
    for b in items {
        edges.extend(body_edges(b, tree, dep_crates, method_index, harvest));
        edges.extend(signature_edges(b, tree, dep_crates));
    }
    edges
}

/// 본문 하나의 syn 방문 — 표현식 안의 호출·참조·매크로 간선.
/// semantic 엔진이 못 보는 본문(cfg 비활성·매크로 생성 정의)의 폴백이기도 하다.
pub fn body_edges(
    b: &BodyItem,
    tree: &ModTree,
    dep_crates: &DepCrates,
    method_index: &BTreeMap<String, Vec<String>>,
    harvest: &mut Harvest,
) -> Vec<Edge> {
    let mut vis = BodyVisitor {
        owner: &b.id,
        module: &b.module,
        self_ty: b.self_ty.as_deref(),
        tree,
        dep_crates,
        method_index,
        edge_cfg: &b.cfg,
        in_unsafe: 0,
        edges: Vec::new(),
        unresolved: 0,
        fanned: 0,
        ext_macros: 0,
    };
    for e in &b.exprs {
        vis.visit_expr(e);
    }
    harvest.unresolved_paths += vis.unresolved;
    harvest.fanned_method_calls += vis.fanned;
    harvest.external_macros += vis.ext_macros;
    vis.edges
}

/// 시그니처 표면(파라미터·반환)의 타입 참조를 signature 간선으로 만든다.
/// semantic 엔진이 본문을 맡을 때도 이 부분은 syn이 권위다 — 두 경로가
/// 같은 간선을 내므로 엔진 선택과 무관하게 일관된다.
pub fn signature_edges(b: &BodyItem, tree: &ModTree, dep_crates: &DepCrates) -> Vec<Edge> {
    let mut edges = Vec::new();
    // 소유 아이템이 cfg면 시그니처 자체가 그 조건 아래 있으니 간선도 물려받는다.
    for t in &b.signature_surface {
        let mut sv = TypeVisitor { paths: Vec::new() };
        sv.visit_type(t);
        for p in sv.paths {
            if let Some(id) = tree.resolve(&b.module, &p, dep_crates) {
                if id != b.id {
                    let mut e = Edge::new(b.id.clone(), id, EdgeKind::Signature);
                    e.cfg = b.cfg.clone();
                    edges.push(e);
                }
            }
        }
    }
    edges
}

/// 본문 방문자 — 호출·경로 참조·매크로 호출을 간선으로 옮긴다.
/// `unsafe {}` 블록 안에서 만든 간선은 경계 진입으로 표시한다.
struct BodyVisitor<'a> {
    owner: &'a str,
    module: &'a str,
    self_ty: Option<&'a [String]>,
    tree: &'a ModTree,
    dep_crates: &'a DepCrates,
    method_index: &'a BTreeMap<String, Vec<String>>,
    /// 소유 아이템의 cfg — 이 본문의 간선은 전부 그 조건 아래 있다.
    edge_cfg: &'a Option<String>,
    /// 현재 unsafe 블록 깊이 — 0보다 크면 간선에 unsafe를 찍는다.
    in_unsafe: usize,
    edges: Vec<Edge>,
    unresolved: usize,
    fanned: usize,
    ext_macros: usize,
}

impl BodyVisitor<'_> {
    fn push(&mut self, to: String, kind: EdgeKind) {
        // 외부 크레이트 정점으로의 간선 — 크레이트 안은 안 보이니
        // `dep::f()`의 call도 실은 "크레이트 경계 참조"다. 멤버 크레이트
        // 정점은 external에 없다 — 멤버 안은 실제 정점이다.
        let kind = if self.dep_crates.external.contains(to.as_str()) {
            EdgeKind::References
        } else {
            kind
        };
        if to != self.owner {
            let mut e = Edge::new(self.owner.to_string(), to, kind);
            e.cfg = self.edge_cfg.clone();
            e.unsafe_ = self.in_unsafe > 0;
            self.edges.push(e);
        }
    }

    /// 추정 간선 — 팬아웃의 "이 중 하나일 수 있다"는 확정이 아니다.
    fn push_maybe(&mut self, to: String, kind: EdgeKind) {
        if to != self.owner {
            let mut e = Edge::maybe(self.owner.to_string(), to, kind);
            e.cfg = self.edge_cfg.clone();
            e.unsafe_ = self.in_unsafe > 0;
            self.edges.push(e);
        }
    }

    fn resolve(&mut self, segs: &[String]) -> Option<String> {
        self.tree.resolve(self.module, segs, self.dep_crates)
    }
}

impl Visit<'_> for BodyVisitor<'_> {
    fn visit_expr_call(&mut self, e: &syn::ExprCall) {
        // foo::bar() — 경로 호출은 정확히 해석된다.
        if let syn::Expr::Path(p) = &*e.func {
            let segs = path_segments(&p.path);
            match self.resolve(&segs) {
                Some(id) => self.push(id, EdgeKind::Call),
                None => {
                    // Type::assoc_fn() — 마지막 세그먼트가 연관 함수다.
                    // 앞부분이 타입 정점이면 그 타입의 메서드로 좁힌다.
                    let n = segs.len();
                    let mut hit = false;
                    if n >= 2 {
                        if let Some(ty) = self.resolve(&segs[..n - 1]) {
                            if let Some(ids) = self.method_index.get(&segs[n - 1]) {
                                for mid in ids {
                                    if mid.starts_with(&format!("{ty}::")) {
                                        self.push(mid.clone(), EdgeKind::Call);
                                        hit = true;
                                    }
                                }
                            }
                        }
                    }
                    if !hit {
                        self.unresolved += 1;
                    }
                }
            }
            // func 경로는 call로 처리했으니 references로 이중 계수하지 않는다.
            for arg in &e.args {
                self.visit_expr(arg);
            }
        } else {
            syn::visit::visit_expr_call(self, e);
        }
    }

    fn visit_expr_method_call(&mut self, e: &syn::ExprMethodCall) {
        // x.m() — 수신자 타입을 모르므로 이름 팬아웃으로 과대 근사한다.
        // self.m()은 enclosing 타입의 메서드로만 좁힐 수 있다.
        let name = e.method.to_string();
        let mut scoped = false;
        if let Some(self_ty) = self.self_ty {
            if matches!(&*e.receiver, syn::Expr::Path(p) if p.path.is_ident("self")) {
                if let Some(sid) = self.resolve(self_ty) {
                    if let Some(ids) = self.method_index.get(&name) {
                        for mid in ids {
                            if mid.starts_with(&format!("{sid}::")) {
                                self.push_maybe(mid.clone(), EdgeKind::Call);
                            }
                        }
                    }
                    scoped = true;
                }
            }
        }
        if !scoped {
            if let Some(ids) = self.method_index.get(&name) {
                for mid in ids {
                    self.push_maybe(mid.clone(), EdgeKind::Call);
                }
                self.fanned += 1;
            }
        }
        syn::visit::visit_expr_method_call(self, e);
    }

    fn visit_expr_macro(&mut self, e: &syn::ExprMacro) {
        // name!() — 크레이트 안 매크로면 간선, std/외부면 실측 생략.
        let segs = path_segments(&e.mac.path);
        match self.resolve(&segs) {
            Some(id) => self.push(id, EdgeKind::Call),
            None => self.ext_macros += 1,
        }
        // 매크로 인자는 토큰 스트림이라 방문자가 못 내린다 — write!·format!·
        // assert_eq! 같은 흔한 형태는 쉼표로 나뉜 표현식 목록이라 직접 파싱을
        // 시도한다. 파싱이 안 되면 매크로 문법이 독자적이라는 뜻이고, 그 안의
        // 호출은 구문 수준에서 영원히 보이지 않는다(ext_macros가 그 수를 잰다).
        if let Ok(exprs) =
            syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated
                .parse2(e.mac.tokens.clone())
        {
            for ex in &exprs {
                self.visit_expr(ex);
                // 포맷 문자열의 `{name}` 캡처 — 토큰이 아니라 문자열 안에 있어
                // AST에 나타나지 않지만 실제 참조다.
                if let syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(s),
                    ..
                }) = ex
                {
                    for name in format_captures(&s.value()) {
                        if let Some(id) = self.resolve(&[name]) {
                            self.push(id, EdgeKind::References);
                        }
                    }
                }
            }
        }
        syn::visit::visit_expr_macro(self, e);
    }

    fn visit_expr_struct(&mut self, e: &syn::ExprStruct) {
        // S{..} 구조체 리터럴 — 생성자 호출과 같은 의미의 참조다.
        let segs = path_segments(&e.path);
        match self.resolve(&segs) {
            Some(id) => self.push(id, EdgeKind::References),
            None if segs.len() >= 2 => self.unresolved += 1,
            None => {}
        }
        syn::visit::visit_expr_struct(self, e);
    }

    fn visit_expr_unsafe(&mut self, e: &syn::ExprUnsafe) {
        // unsafe 블록 안의 호출·참조는 경계를 넘는 진입이다 — 깊이를 세어
        // 이 블록에서 나오는 동안 만드는 간선 전부에 표시한다.
        self.in_unsafe += 1;
        syn::visit::visit_expr_unsafe(self, e);
        self.in_unsafe -= 1;
    }

    fn visit_expr_path(&mut self, e: &syn::ExprPath) {
        // 호출이 아닌 경로 참조 — filter_map(f) 같은 함수 값 포함.
        // 단일 식별자는 모듈 아이템이면 잡고 지역 변수면 조용히 넘긴다.
        let segs = path_segments(&e.path);
        match self.resolve(&segs) {
            Some(id) => self.push(id, EdgeKind::References),
            None => {
                // E::V 형태 — 마지막 세그먼트가 열거형 배리언트/상수면
                // 앞부분 타입에의 참조다. 그것도 안 되면 진짜 미해석.
                if segs.len() >= 2 {
                    match self.resolve(&segs[..segs.len() - 1]) {
                        Some(ty) => self.push(ty, EdgeKind::References),
                        None => self.unresolved += 1,
                    }
                }
            }
        }
        syn::visit::visit_expr_path(self, e);
    }
}

/// 타입 안의 경로들을 수집하는 방문자 — 제네릭 인자까지 재귀한다.
struct TypeVisitor {
    paths: Vec<Vec<String>>,
}

impl Visit<'_> for TypeVisitor {
    fn visit_path(&mut self, p: &syn::Path) {
        self.paths.push(path_segments(p));
        syn::visit::visit_path(self, p);
    }
}

/// `a::b::C` 경로를 세그먼트 목록으로 — 제네릭 인자는 버린다.
pub fn path_segments(p: &syn::Path) -> Vec<String> {
    p.segments.iter().map(|s| s.ident.to_string()).collect()
}

/// 타입을 경로 세그먼트로 — Named 타입만 경로가 된다.
fn type_path(t: &syn::Type) -> Vec<String> {
    match t {
        syn::Type::Path(tp) => path_segments(&tp.path),
        _ => Vec::new(),
    }
}

/// 구문 아이템의 속성 목록을 꺼낸다.
fn attrs_of(item: &syn::Item) -> &[syn::Attribute] {
    match item {
        syn::Item::Fn(x) => &x.attrs,
        syn::Item::Struct(x) => &x.attrs,
        syn::Item::Enum(x) => &x.attrs,
        syn::Item::Trait(x) => &x.attrs,
        syn::Item::Union(x) => &x.attrs,
        syn::Item::Type(x) => &x.attrs,
        syn::Item::Const(x) => &x.attrs,
        syn::Item::Static(x) => &x.attrs,
        syn::Item::Mod(x) => &x.attrs,
        syn::Item::Impl(x) => &x.attrs,
        syn::Item::Macro(x) => &x.attrs,
        _ => &[],
    }
}

/// `pub`만 exported로 본다 — pub(crate)/pub(super)는 공개 API가 아니다.
fn is_pub(v: &syn::Visibility) -> bool {
    matches!(v, syn::Visibility::Public(_))
}

/// 두 cfg 조건을 `all(...)`로 결합한다 — 둘 다 성립해야 참조가 존재한다.
fn conjoin(a: &Option<String>, b: &str) -> Option<String> {
    match a {
        Some(a) => Some(format!("all({a} , {b})")),
        None => Some(b.to_string()),
    }
}

/// 아이템 속성에서 경로 참조를 모은다 — 해석은 임포트 완성 뒤 2패스에서.
/// 한 세그먼트 이름(test·cfg·derive·allow...)은 내장이거나 임포트로
/// 이미 잡히므로 두 세그먼트 이상만 모은다. cfg_attr 안쪽 속성은
/// 그 술어를 cfg로 물려받는다 — 조건 없이 성립한다고 속이면 안 된다.
/// 반환값: 속성 목록을 읽지 못한 cfg_attr 수(unparsed_attrs로 간다).
fn collect_attr_refs(
    attrs: &[syn::Attribute],
    owner: &str,
    module: &str,
    mod_decl: bool,
    cfg: &Option<String>,
    out: &mut Vec<AttrRef>,
) -> usize {
    let mut unparsed = 0;
    let push = |segs: Vec<String>, cfg: &Option<String>, out: &mut Vec<AttrRef>| {
        if segs.len() >= 2
            // 도구 네임스페이스 속성(rustfmt·clippy·diagnostic)은 크레이트
            // 참조가 아니다 — 해석은 항상 실패하니 수집하면 미해석
            // 카운터만 부푼다.
            && !matches!(
                segs[0].as_str(),
                "rustfmt" | "clippy" | "diagnostic"
            )
        {
            out.push(AttrRef {
                owner: owner.to_string(),
                mod_decl,
                module: module.to_string(),
                path: segs,
                cfg: cfg.clone(),
            });
        }
    };
    for a in attrs {
        if a.path().is_ident("derive") {
            // #[derive(a::b::C, D)] — 다중 세그먼트 인자만 참조다.
            let args = a.parse_args_with(
                syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated,
            );
            if let Ok(paths) = args {
                for p in paths {
                    push(path_segments(&p), cfg, out);
                }
            }
        } else if a.path().is_ident("cfg_attr") {
            // #[cfg_attr(pred, meta, ...)] — 첫 인자는 술어, 나머지는 속성.
            if let syn::Meta::List(l) = &a.meta {
                match split_cfg_attr(&l.tokens) {
                    Some((pred, Ok(metas))) => {
                        for m in metas {
                            unparsed +=
                                collect_meta_refs(&m, owner, module, mod_decl, &pred, cfg, out);
                        }
                    }
                    // 속성 목록이 Meta 문법이 아니면 안쪽 경로를 읽을 수 없다.
                    Some((_, Err(_))) => unparsed += 1,
                    // 쉼표 없는 cfg_attr(pred)는 적용할 속성이 없는 형태다.
                    None => {}
                }
            }
        } else {
            push(path_segments(a.path()), cfg, out);
        }
    }
    unparsed
}

/// cfg_attr 안쪽 메타 하나를 참조로 모은다 — 술어와 아이템 자신의
/// cfg를 all()로 합성해 단다. `derive(dep::T)` 인자와 중첩 `cfg_attr`도
/// 재귀로 파낸다 — 그 안의 경로도 실제 참조다.
/// 반환값: 중첩 cfg_attr 중 속성 목록을 읽지 못한 수.
fn collect_meta_refs(
    m: &syn::Meta,
    owner: &str,
    module: &str,
    mod_decl: bool,
    pred: &str,
    cfg: &Option<String>,
    out: &mut Vec<AttrRef>,
) -> usize {
    // 이 참조가 성립하는 조건 — 아이템 cfg와 cfg_attr 술어의 합성.
    let cond = conjoin(cfg, pred);
    match m {
        // cfg_attr(pred, derive(dep::T)) — derive 인자가 진짜 참조다.
        syn::Meta::List(l) if l.path.is_ident("derive") => {
            let args = l.parse_args_with(
                syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated,
            );
            if let Ok(paths) = args {
                for p in paths {
                    let segs = path_segments(&p);
                    if segs.len() >= 2 {
                        out.push(AttrRef {
                            owner: owner.to_string(),
                            mod_decl,
                            module: module.to_string(),
                            path: segs,
                            cfg: cond.clone(),
                        });
                    }
                }
            }
        }
        // 중첩 cfg_attr — 바깥 술어와 안쪽 술어를 둘 다 성립 조건으로 쌓는다.
        syn::Meta::List(l) if l.path.is_ident("cfg_attr") => {
            return match split_cfg_attr(&l.tokens) {
                Some((inner_pred, Ok(metas))) => metas
                    .iter()
                    .map(|m| collect_meta_refs(m, owner, module, mod_decl, &inner_pred, &cond, out))
                    .sum(),
                Some((_, Err(_))) => 1,
                None => 0,
            };
        }
        _ => {
            let path = match m {
                syn::Meta::Path(p) => p,
                syn::Meta::List(l) => &l.path,
                syn::Meta::NameValue(nv) => &nv.path,
            };
            let segs = path_segments(path);
            if segs.len() >= 2 {
                out.push(AttrRef {
                    owner: owner.to_string(),
                    mod_decl,
                    module: module.to_string(),
                    path: segs,
                    cfg: cond,
                });
            }
        }
    }
    0
}

/// cfg_attr의 인자를 (술어 원문, 적용 메타 목록)으로 쪼갠다.
/// 첫 최상위 쉼표가 술어와 속성의 경계다. 술어는 토큰 원문 그대로
/// 간다 — `Meta`로 재파싱해 LitStr의 value()를 다시 따옴표로 감싸면
/// `\\x6c` 같은 이스케이프가 디코드된 채 남아 거짓 조건이 참으로
/// 뒤집힌다. 속성 목록의 파싱 실패는 Err로 남긴다 — 호출자가 세야
/// 안쪽 경로의 유실이 limitation으로 드러난다.
fn split_cfg_attr(
    tokens: &proc_macro2::TokenStream,
) -> Option<(String, syn::Result<Vec<syn::Meta>>)> {
    let mut pred_ts = proc_macro2::TokenStream::new();
    let mut rest_ts = proc_macro2::TokenStream::new();
    let mut seen_comma = false;
    for tt in tokens.clone() {
        if !seen_comma && matches!(&tt, proc_macro2::TokenTree::Punct(p) if p.as_char() == ',') {
            seen_comma = true;
            continue;
        }
        if seen_comma {
            rest_ts.extend(std::iter::once(tt));
        } else {
            pred_ts.extend(std::iter::once(tt));
        }
    }
    if !seen_comma {
        return None;
    }
    use syn::parse::Parser;
    let metas = syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated
        .parse2(rest_ts)
        .map(|m| m.into_iter().collect());
    Some((pred_ts.to_string(), metas))
}

/// `file:line` 위치 — 아이템의 이름 span 줄을 쓴다.
fn position_of(file: &Path, item: &syn::Item) -> Option<String> {
    let line = item.ident().map(line_of).unwrap_or(0);
    Some(format!("{}:{line}", file.display()))
}

/// 식별자의 소스 줄 — span-locations로 얻는다. 없으면 0.
fn line_of(ident: &syn::Ident) -> usize {
    ident.span().start().line
}

/// 아이템의 이름 식별자를 꺼낸다.
trait ItemIdent {
    fn ident(&self) -> Option<&syn::Ident>;
}

impl ItemIdent for syn::Item {
    fn ident(&self) -> Option<&syn::Ident> {
        Some(match self {
            syn::Item::Fn(x) => &x.sig.ident,
            syn::Item::Struct(x) => &x.ident,
            syn::Item::Enum(x) => &x.ident,
            syn::Item::Trait(x) => &x.ident,
            syn::Item::Union(x) => &x.ident,
            syn::Item::Type(x) => &x.ident,
            syn::Item::Const(x) => &x.ident,
            syn::Item::Static(x) => &x.ident,
            syn::Item::Mod(x) => &x.ident,
            _ => return None,
        })
    }
}

/// 파일 헤더에 생성 코드 마커가 있는지 본다 — 표시하되 숨기지 않는다.
fn file_has_generated_marker(file: &Path) -> bool {
    let Ok(src) = std::fs::read_to_string(file) else {
        return false;
    };
    src.lines().take(10).any(|l| {
        let l = l.to_lowercase();
        l.contains("generated") && (l.contains("do not edit") || l.contains("@generated"))
            || l.contains("auto-generated")
            || l.contains("autogenerated")
    })
}

/// 표현식 목록 안에 `unsafe {}` 블록이 있는지 본다 — 재귀 방문.
fn exprs_have_unsafe(exprs: &[syn::Expr]) -> bool {
    struct Finder(bool);
    impl Visit<'_> for Finder {
        fn visit_expr_unsafe(&mut self, _e: &syn::ExprUnsafe) {
            self.0 = true;
        }
    }
    let mut f = Finder(false);
    for e in exprs {
        if f.0 {
            break;
        }
        f.visit_expr(e);
    }
    f.0
}

/// 포맷 문자열의 `{ident}`·`{ident:spec}` 캡처 이름을 뽑는다.
/// `{{`/`}}`는 이스케이프라 건너뛰고, 숫자 위치 인자(`{0}`)는 제외한다.
fn format_captures(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
            }
            '{' => {
                let name: String = chars
                    .by_ref()
                    .take_while(|&c| c != '}' && c != ':' && c != '?')
                    .collect();
                if name
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                {
                    out.push(name);
                }
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
            }
            _ => {}
        }
    }
    out
}

/// 테스트·외부 노출 속성으로 보존 루트 후보인지 본다.
pub fn is_test_entry(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path()
            .segments
            .last()
            .is_some_and(|s| s.ident == "test" || s.ident == "bench")
    })
}

/// no_mangle/export_name/used/proc_macro 계열 — 링커·매크로가 부르는 진입점.
pub fn is_extern_entry(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().segments.last().is_some_and(|s| {
            matches!(
                s.ident.to_string().as_str(),
                "no_mangle"
                    | "export_name"
                    | "used"
                    | "proc_macro"
                    | "proc_macro_derive"
                    | "proc_macro_attribute"
            )
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// cfg_attr 술어의 문자열 리터럴은 이스케이프 원문 그대로 보존된다 —
    /// `feature = "a\x62c"`를 Meta로 재파싱해 value()를 다시 감싸면
    /// "abc"로 바뀌어 거짓 조건이 참으로 평가될 수 있다.
    #[test]
    fn split_cfg_attr_preserves_predicate_escapes() {
        let attr: syn::Attribute =
            syn::parse_quote!(#[cfg_attr(feature = "a\x62c", derive(Debug))]);
        let syn::Meta::List(l) = attr.meta else {
            panic!("cfg_attr is a list meta")
        };
        let (pred, metas) = split_cfg_attr(&l.tokens).expect("split");
        let metas = metas.expect("derive(Debug) is a valid meta list");
        // 원문 이스케이프가 남고 디코드된 값이 섞이지 않아야 한다.
        assert!(pred.contains("\\x62"), "predicate lost escape: {pred}");
        assert!(!pred.contains("\"abc\""), "predicate decoded: {pred}");
        assert_eq!(metas.len(), 1);
        // 쉼표 없는 cfg_attr는 술어/속성 경계가 없다 — None.
        let attr2: syn::Attribute = syn::parse_quote!(#[cfg_attr(test)]);
        let syn::Meta::List(l2) = attr2.meta else {
            panic!("cfg_attr is a list meta")
        };
        assert!(split_cfg_attr(&l2.tokens).is_none());
    }

    /// 속성 목록이 Meta 문법이 아닌 cfg_attr는 안쪽 경로를 읽을 수 없다 —
    /// 조용히 버리지 않고 unparsed_attrs로 센다. 중첩 cfg_attr도 같다.
    /// 정상 목록은 세지 않고 참조를 그대로 모은다.
    #[test]
    fn unparsable_cfg_attr_list_is_counted() {
        let file: syn::File = syn::parse_str(
            "#[cfg_attr(test, 1 + 2)] fn a() {}
             #[cfg_attr(unix, cfg_attr(test, 1 + 2))] fn b() {}
             #[cfg_attr(test, dep::keep)] fn c() {}",
        )
        .expect("fixture parses as a file");
        let groups = [(PathBuf::from("lib.rs"), file.items.as_slice())];
        let mut h = Harvest::default();
        let d = decls("k", "k", &groups, &mut h);
        assert_eq!(h.unparsed_attrs, 2);
        assert_eq!(d.attr_refs.len(), 1, "valid cfg_attr still harvested");
        assert_eq!(d.attr_refs[0].path, ["dep", "keep"]);
    }
}
