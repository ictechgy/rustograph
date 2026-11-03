//! syn AST에서 정점과 간선을 수확한다.
//!
//! 구문 수준 수확이라 할 수 있는 것과 없는 것을 구분한다: 경로 호출·use·impl은
//! 정확히 해석되고, 메서드 호출(`x.m()`)은 타입이 없어 이름 팬아웃으로
//! 과대 근사한다 — 오탐은 "살아 있다" 쪽으로만 기울게 하는 계약이다. 후보 규칙과
//! 그 예외(폭 상한을 넘는 호출은 간선 없이 센다)는 [`crate::fanout`]에 있다.

use crate::fanout::{self, Fanned};
use crate::graph::{Edge, EdgeKind, Kind, Vertex};
use crate::modtree::{cfg_of, crate_of, DepCrates, ModTree};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use syn::parse::Parser;
use syn::spanned::Spanned;
use syn::visit::Visit;

pub(crate) mod identity;
mod locals;

/// 수확 중간 산출물 — 문서 조립 전의 실측 카운터.
#[derive(Default)]
pub struct Harvest {
    /// 해석에 실패한 경로 수 — 외부 참조 limitation의 실측값.
    pub unresolved_paths: usize,
    /// 이름 팬아웃으로 해석한 메서드 호출 수.
    pub fanned_method_calls: usize,
    /// 같은 이름 메서드가 전부 호출 크레이트의 의존 폐포 밖 크레이트의 고유·트레이트 선언 메서드라
    /// 팬아웃 간선을 달지 않은 메서드 호출 수 — 그런 메서드는 정적으로 호출할 수 없다.
    pub out_of_reach_method_calls: usize,
    /// 좁힌 후보가 팬아웃 폭 상한을 넘어 간선을 달지 않은 메서드 호출 수 — 그 이름의 메서드를
    /// 살려 두지 못하므로 limitation이 dead·도달성에 닿는 영향을 알린다.
    pub capped_method_calls: usize,
    /// 그 호출들의 메서드 이름(중복 없음).
    pub capped_method_names: BTreeSet<String>,
    /// 외부/std 매크로 호출 수(유령 정점 없이 생략).
    pub external_macros: usize,
    /// cfg 조건부로 포함한 아이템 수.
    pub cfg_items: usize,
    /// 속성 목록을 Meta로 읽지 못한 cfg_attr 수 — 안쪽 경로가 참조로
    /// 잡히지 않았으니 속성으로만 쓰는 dep이 미사용으로 보일 수 있다.
    pub unparsed_attrs: usize,
    /// self 타입이 워크스페이스 정점이 아닌 impl 블록 수(`impl Tr for String`, blanket
    /// `impl<T> Tr for T`, `impl Tr for &Local`) — 메서드 정점이 없어 본문을 대신 소유시킨다.
    pub foreign_self_impls: usize,
}

/// 한 모듈의 아이템 목록을 정점으로 만든다(1패스 — 선언만).
/// 반환값: (이 모듈이 만든 정점들, 이 모듈의 impl 블록들).
pub struct ModuleDecls<'a> {
    pub vertices: Vec<Vertex>,
    pub impls: Vec<ImplBlock<'a>>,
    /// fn/method/const/static/macro 본문을 담은 항목(2패스용).
    pub bodies: Vec<BodyItem<'a>>,
    /// 아이템 속성의 경로 참조 — `#[dep::attr]`, `#[derive(dep::X)]`.
    /// 해석은 임포트가 채워진 뒤 2패스에서 한다.
    pub attr_refs: Vec<AttrRef>,
    /// #[no_mangle]·proc_macro 같은 외부 호출 진입점 — 항상 보존 루트.
    pub entry_roots: Vec<String>,
    /// #[test]/#[bench] 진입점 — --tests일 때만 보존 루트가 된다.
    pub test_roots: Vec<String>,
    /// 본문이 없는 소유 아이템(struct·enum·union·trait·macro_rules)의 소스
    /// 범위 — persistence 사실을 감싸는 그래프 정점으로 귀속할 때 쓴다.
    /// 본문 아이템의 범위는 BodyItem이 이미 들고 있다.
    pub owner_spans: Vec<OwnerSpan>,
    /// 본문 밖 선언이 담은 경로 참조 — 필드 타입과 `macro_rules!` 정의 본문. 해석은 임포트가
    /// 채워진 뒤 2패스에서 [`decl_edges`]가 한다.
    pub decl_refs: Vec<DeclRef>,
}

/// 본문 밖 선언이 담은 경로 참조 하나.
///
/// struct·enum·union 필드의 타입은 그 타입이 쓰는 타입이다 — 모으지 않으면 DTO 필드로만 쓰는
/// 타입이 unreachable로 보이고, 필드가 다른 모듈을 가리키는 의존이 규칙·순환에서 사라진다.
/// `macro_rules!` 정의 본문의 경로(`$crate::m::f`)는 매크로를 펼친 곳에서 쓰이는 이름이라 확정
/// 사용이 아니다 — 추정(tentative) 간선으로 dead·도달성에만 쓰인다.
#[derive(Debug, Clone)]
pub struct DeclRef {
    /// 참조를 소유한 정점 ID — 필드면 그 타입, 매크로 본문이면 그 매크로.
    pub owner: String,
    /// 경로를 해석할 모듈 — 선언이 사는 모듈이다.
    pub module: String,
    /// 경로 세그먼트(`$crate`는 `crate`로 바꿔 둔다).
    pub path: Vec<String>,
    /// 소유 아이템과 필드·배리언트의 `#[cfg]`를 합친 조건.
    pub cfg: Option<String>,
    /// 쓰임이 확정되지 않은 참조인가 — 매크로 본문이면 true다.
    pub tentative: bool,
    /// 소유 아이템의 선언 파일 — 같은 경로의 선언이 여럿이면 소스 인스턴스 ID로 옮길 때 쓴다.
    pub file: PathBuf,
    /// 소유 아이템의 파일 안 바이트 시작 위치(syn span, 속성 포함).
    pub start: usize,
}

/// 그래프 정점 하나가 소스에서 차지하는 범위다.
/// schema 사실(위치만 있는)을 impact와 같은 정점 ID로 귀속하는 재료다 —
/// 정점 ID 규칙(impl self 타입 해석 등)을 schema 쪽에서 다시 구현하면
/// 두 규칙이 어긋나 사실의 usr가 그래프에 없는 문자열이 된다.
#[derive(Debug, Clone)]
pub struct OwnerSpan {
    /// 정점 ID — 수확이 만든 그대로다.
    pub id: String,
    /// 선언 파일(수확이 읽은 경로 그대로 — 비교 전에 정규화한다).
    pub file: PathBuf,
    /// 파일 안 바이트 범위(syn span 기준, 속성 포함).
    pub range: std::ops::Range<usize>,
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
pub struct ImplBlock<'a> {
    /// 메서드 본문에서도 유효한 impl의 타입·const 매개변수 스코프다.
    pub generics: &'a syn::Generics,
    pub self_ty: Vec<String>,
    pub trait_path: Option<Vec<String>>,
    /// impl 안의 메서드 — 파싱한 AST 아레나의 노드를 빌린다. 예전에는 메서드를 통째로 복제해
    /// [`impls`]가 그 본문을 또 복제했다(impl 메서드 본문이 수확 내내 세 벌이었다).
    pub methods: Vec<&'a syn::ImplItemFn>,
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
    /// impl 블록의 바이트 범위 — 메서드 밖 사실(연관 상수 등)을 self 타입에 귀속한다.
    pub range: std::ops::Range<usize>,
}

/// 본문 아이템이 방문할 AST — 파싱한 아레나의 노드를 그대로 빌린다. 아레나는 수확 내내 살아
/// 있으므로 복제할 까닭이 없다: 예전처럼 본문을 표현식 목록으로 깊은 복제하면 수확 피크가 본문
/// AST만큼(impl 메서드는 그 두 배) 커졌다.
#[derive(Clone, Copy)]
pub enum Body<'a> {
    /// fn·impl 메서드·트레이트 기본 메서드 본문 블록의 문장들. 최상위 문장은 중첩 블록과 방문
    /// 규칙이 다르다 — [`body_edges`]가 따르는 규칙은 그쪽 문서에 있다.
    Stmts(&'a [syn::Stmt]),
    /// const·static 초기식.
    Expr(&'a syn::Expr),
}

/// 본문을 나중에 방문할 항목.
pub struct BodyItem<'a> {
    /// 본문을 감싸는 impl·trait·함수의 제네릭 스코프다. 전역 아이템과 구분한다.
    pub generic_scopes: Vec<&'a syn::Generics>,
    pub id: String,
    pub module: String,
    /// Self가 가리키는 타입(impl 안이면 Some).
    pub self_ty: Option<Vec<String>>,
    /// 방문할 본문 — 아레나의 문장·초기식을 빌린다.
    pub body: Body<'a>,
    /// 매개변수가 묶는 이름 — 본문 전체에서 같은 이름의 아이템을 가린다.
    pub params: Vec<String>,
    pub signature_surface: Vec<&'a syn::Type>,
    /// 소유 아이템의 `#[cfg]` — 이 본문이 만드는 간선 전부가 그 조건 아래 있다.
    pub cfg: Option<String>,
    /// 소유 아이템이 선언된 파일 — semantic 엔진이 소스 정체를 맞출 때 쓴다.
    pub file: PathBuf,
    /// 소유 아이템의 바이트 범위 — cfg 변형·블록 지역 정의 같은
    /// 정규 ID 충돌을 소스 위치로 걸러내는 데 쓴다.
    pub range: std::ops::Range<usize>,
}

/// 모듈의 파일 AST를 1패스로 돌려 선언 정점을 수확한다.
/// `groups`는 (선언 파일, 그 파일의 아이템 목록) — lib/bin 합본 루트처럼
/// 한 모듈의 아이템이 여러 파일에 걸칠 수 있어 위치·본문 정체는 아이템의
/// 실제 파일을 따라야 한다. 생성 코드 마커는 선언 파일을 읽어 판정한다.
pub fn decls<'a>(
    module_path: &str,
    krate: &str,
    groups: &[(PathBuf, &'a [syn::Item])],
    harvest: &mut Harvest,
) -> ModuleDecls<'a> {
    decls_with_markers(
        module_path,
        krate,
        groups,
        &file_has_generated_marker,
        harvest,
        &mut identity::Registry::default(),
    )
}

/// [`decls`]와 같되 선언 파일의 생성 코드 마커([`text_has_generated_marker`] — 모듈 정점과
/// 같은 규칙)를 호출자가 준다. 수확은 파싱할 때 읽은 원문으로 파일마다 한 번 판정해 둔다:
/// 그룹(모듈)마다 파일을 다시 통째로 읽으면 인라인 모듈이 수천 개인 생성 파일(prost 출력)에서
/// (모듈 수 × 파일 크기)를 읽는다.
pub(crate) fn decls_with_markers<'a>(
    module_path: &str,
    krate: &str,
    groups: &[(PathBuf, &'a [syn::Item])],
    is_generated_file: &dyn Fn(&Path) -> bool,
    harvest: &mut Harvest,
    identities: &mut identity::Registry,
) -> ModuleDecls<'a> {
    let mut out = ModuleDecls {
        vertices: Vec::new(),
        impls: Vec::new(),
        bodies: Vec::new(),
        attr_refs: Vec::new(),
        entry_roots: Vec::new(),
        test_roots: Vec::new(),
        owner_spans: Vec::new(),
        decl_refs: Vec::new(),
    };
    for (file, items) in groups {
        let generated = is_generated_file(file);
        for item in items.iter() {
            let first_vertex = out.vertices.len();
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
                    let body = Body::Stmts(&f.block.stmts);
                    // unsafe fn이거나 본문에 unsafe 블록이 있으면 경계의 안쪽이다.
                    let unsafe_ = f.sig.unsafety.is_some() || body_has_unsafe(body);
                    out.vertices
                        .push(v(id.clone(), Kind::Fn, is_pub(&f.vis), unsafe_));
                    if is_extern_entry(&f.attrs) {
                        out.entry_roots.push(id.clone());
                        identities.note_root(
                            &id,
                            module_path,
                            false,
                            file,
                            f.span().byte_range().start,
                        );
                    } else if is_test_entry(&f.attrs) {
                        out.test_roots.push(id.clone());
                        identities.note_root(
                            &id,
                            module_path,
                            true,
                            file,
                            f.span().byte_range().start,
                        );
                    }
                    out.bodies.push(BodyItem {
                        generic_scopes: generic_scopes([&f.sig.generics]),
                        id,
                        module: module_path.to_string(),
                        self_ty: None,
                        body,
                        params: locals::param_bindings(&f.sig),
                        signature_surface: fn_signature_types(&f.sig),
                        cfg: cfg.clone(),
                        file: file.to_path_buf(),
                        range: f.span().byte_range(),
                    });
                }
                syn::Item::Struct(s) => {
                    let id = format!("{module_path}::{}", s.ident);
                    out.vertices
                        .push(v(id.clone(), Kind::Struct, is_pub(&s.vis), false));
                    let owner = DeclOwner::new(&id, module_path, &s.ident, &s.generics, file, item);
                    owner.push_fields(fields_with_cfg(&s.fields, &cfg), &mut out.decl_refs);
                }
                syn::Item::Enum(e) => {
                    let id = format!("{module_path}::{}", e.ident);
                    out.vertices
                        .push(v(id.clone(), Kind::Enum, is_pub(&e.vis), false));
                    let owner = DeclOwner::new(&id, module_path, &e.ident, &e.generics, file, item);
                    for variant in &e.variants {
                        let variant_cfg = match cfg_of(&variant.attrs) {
                            Some(condition) => conjoin(&cfg, &condition),
                            None => cfg.clone(),
                        };
                        owner.push_fields(
                            fields_with_cfg(&variant.fields, &variant_cfg),
                            &mut out.decl_refs,
                        );
                    }
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
                            let own_cfg = cfg_of(&m.attrs);
                            harvest.cfg_items += usize::from(own_cfg.is_some());
                            let mcfg = match own_cfg {
                                Some(ref condition) => conjoin(&cfg, condition),
                                None => cfg.clone(),
                            };
                            let mid = format!("{module_path}::{}::{}", t.ident, m.sig.ident);
                            // 기본 구현이 없는 선언은 본문이 없다 — 정점만 남는다.
                            let body = m.default.as_ref().map(|b| Body::Stmts(&b.stmts));
                            let unsafe_ =
                                m.sig.unsafety.is_some() || body.is_some_and(body_has_unsafe);
                            let mut vertex = v(mid.clone(), Kind::Method, is_pub(&t.vis), unsafe_);
                            vertex.cfg = mcfg.clone();
                            identities.note(&vertex, file, m.span().byte_range().start);
                            identities.note_member(
                                &format!("{module_path}::{}", t.ident),
                                t.span().byte_range().start,
                                &vertex,
                                file,
                                m.span().byte_range().start,
                                t.unsafety.is_some(),
                            );
                            out.vertices.push(vertex);
                            if let Some(body) = body {
                                out.bodies.push(BodyItem {
                                    generic_scopes: generic_scopes([&t.generics, &m.sig.generics]),
                                    id: mid,
                                    module: module_path.to_string(),
                                    self_ty: None,
                                    body,
                                    params: locals::param_bindings(&m.sig),
                                    signature_surface: fn_signature_types(&m.sig),
                                    cfg: mcfg,
                                    file: file.to_path_buf(),
                                    range: m.span().byte_range(),
                                });
                            }
                        }
                    }
                }
                syn::Item::Union(u) => {
                    let id = format!("{module_path}::{}", u.ident);
                    out.vertices
                        .push(v(id.clone(), Kind::Union, is_pub(&u.vis), false));
                    let owner = DeclOwner::new(&id, module_path, &u.ident, &u.generics, file, item);
                    let fields = u.fields.named.iter().map(|f| (&f.ty, field_cfg(f, &cfg)));
                    owner.push_fields(fields.collect(), &mut out.decl_refs);
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
                    let body = Body::Expr(&c.expr);
                    let unsafe_ = body_has_unsafe(body);
                    out.vertices
                        .push(v(id.clone(), Kind::Const, is_pub(&c.vis), unsafe_));
                    out.bodies.push(BodyItem {
                        generic_scopes: Vec::new(),
                        id,
                        module: module_path.to_string(),
                        self_ty: None,
                        body,
                        params: Vec::new(),
                        signature_surface: vec![&c.ty],
                        cfg: cfg.clone(),
                        file: file.to_path_buf(),
                        range: c.span().byte_range(),
                    });
                }
                syn::Item::Static(s) => {
                    let id = format!("{module_path}::{}", s.ident);
                    let body = Body::Expr(&s.expr);
                    // static mut 초기화의 unsafe 블록도 경계다.
                    let unsafe_ = body_has_unsafe(body);
                    out.vertices
                        .push(v(id.clone(), Kind::Static, is_pub(&s.vis), unsafe_));
                    // `#[used]`·`#[no_mangle]`·`#[global_allocator]` static은 링커·런타임이 쓴다 —
                    // 초기식이 가리키는 함수·타입도 그 루트에서 닿는다.
                    if is_extern_entry(&s.attrs) {
                        out.entry_roots.push(id.clone());
                        identities.note_root(
                            &id,
                            module_path,
                            false,
                            file,
                            s.span().byte_range().start,
                        );
                    }
                    out.bodies.push(BodyItem {
                        generic_scopes: Vec::new(),
                        id,
                        module: module_path.to_string(),
                        self_ty: None,
                        body,
                        params: Vec::new(),
                        signature_surface: vec![&s.ty],
                        cfg: cfg.clone(),
                        file: file.to_path_buf(),
                        range: s.span().byte_range(),
                    });
                }
                syn::Item::Macro(m) => {
                    if let Some(id) = &m.ident {
                        if m.mac.path.is_ident("macro_rules") {
                            let mid = format!("{module_path}::{id}");
                            out.vertices.push(v(mid.clone(), Kind::Macro, true, false));
                            for path in macro_body_paths(m.mac.tokens.clone()) {
                                out.decl_refs.push(DeclRef {
                                    owner: mid.clone(),
                                    module: module_path.to_string(),
                                    path,
                                    cfg: cfg.clone(),
                                    tentative: true,
                                    file: file.clone(),
                                    start: item.span().byte_range().start,
                                });
                            }
                        }
                    }
                }
                syn::Item::Impl(i) => {
                    out.impls.push(ImplBlock {
                        generics: &i.generics,
                        self_ty: type_path(&i.self_ty),
                        trait_path: i.trait_.as_ref().map(|(_, p, _)| path_segments(p)),
                        methods: i
                            .items
                            .iter()
                            .filter_map(|x| match x {
                                syn::ImplItem::Fn(f) => Some(f),
                                _ => None,
                            })
                            .collect(),
                        items_module: module_path.to_string(),
                        cfg: cfg.clone(),
                        unsafe_: i.unsafety.is_some(),
                        file: file.clone(),
                        generated,
                        range: i.span().byte_range(),
                    });
                }
                _ => {}
            }
            for vertex in &out.vertices[first_vertex..] {
                // 기본 구현 없는 같은 이름의 cfg 메서드도 각자의 실제 범위를 쓴다.
                // 위에서 등록한 메서드를 이름으로 다시 찾으면 첫 선언으로 합쳐진다.
                if !(matches!(item, syn::Item::Trait(_)) && vertex.kind == Kind::Method) {
                    identities.note(vertex, file, item.span().byte_range().start);
                }
            }
            // 본문 없는 소유 아이템의 범위 — 필드 어트리뷰트 같은 선언 수준
            // 사실은 그 타입 정점이 감싼다.
            let owner_kind = match item {
                syn::Item::Struct(_)
                | syn::Item::Enum(_)
                | syn::Item::Union(_)
                | syn::Item::Trait(_) => true,
                syn::Item::Macro(m) => m.ident.is_some() && m.mac.path.is_ident("macro_rules"),
                _ => false,
            };
            if owner_kind {
                if let Some(ident) = item.ident().or(match item {
                    syn::Item::Macro(m) => m.ident.as_ref(),
                    _ => None,
                }) {
                    out.owner_spans.push(OwnerSpan {
                        id: format!("{module_path}::{ident}"),
                        file: file.clone(),
                        range: item.span().byte_range(),
                    });
                }
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
            let first_attr = out.attr_refs.len();
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
            identities.note_attrs(
                &out.attr_refs[first_attr..],
                file,
                item.span().byte_range().start,
            );
        }
    }
    out
}

/// impl 블록들을 정점·간선으로 변환한다(1패스 후속).
/// 반환: (정점, 간선, 속성 경로 참조, 2패스용 본문 목록). 본문은 블록 목록이 아니라 블록이
/// 빌린 AST 아레나를 빌린다 — 블록 목록을 먼저 버려도 된다.
pub fn impls<'a>(
    blocks: &[ImplBlock<'a>],
    krate: &str,
    tree: &ModTree,
    harvest: &mut Harvest,
) -> (Vec<Vertex>, Vec<Edge>, Vec<AttrRef>, Vec<BodyItem<'a>>) {
    let mut parts = impls_with_identities(
        blocks,
        krate,
        tree,
        &BTreeSet::new(),
        harvest,
        &mut identity::Registry::default(),
    );
    parts.bodies.append(&mut parts.foreign_bodies);
    (parts.vertices, parts.edges, parts.attr_refs, parts.bodies)
}

/// [`impls_with_identities`]의 산출물 — 공개 [`impls`]의 네 묶음에 impl 메서드 진입점 루트를 더한다.
pub(crate) struct ImplParts<'a> {
    pub vertices: Vec<Vertex>,
    pub edges: Vec<Edge>,
    pub attr_refs: Vec<AttrRef>,
    pub bodies: Vec<BodyItem<'a>>,
    /// `#[no_mangle]` 같은 진입점 속성이 붙은 impl 메서드 — 항상 보존 루트다.
    pub entry_roots: Vec<String>,
    /// self 타입이 워크스페이스 정점이 아닌 impl의 메서드 본문 — 소유자는 구현한 워크스페이스
    /// 트레이트의 선언 메서드나 impl이 선언된 모듈이다([`foreign_impl_bodies`]). 호출자는 소유자가
    /// 정점으로 남았는지 확인한 뒤 쓴다.
    pub foreign_bodies: Vec<BodyItem<'a>>,
}

/// 소스 수확의 전역 충돌 판정에 선언 위치를 넘긴다. 공개 수확 API는 그대로 둔다.
/// `methods`는 이미 수확한 메서드 정점 ID다 — self 타입이 워크스페이스 밖인 impl의 본문을 구현한
/// 트레이트의 선언 메서드가 소유하게 할 때 그 정점이 있는지 본다.
pub(crate) fn impls_with_identities<'a>(
    blocks: &[ImplBlock<'a>],
    krate: &str,
    tree: &ModTree,
    methods: &BTreeSet<String>,
    harvest: &mut Harvest,
    identities: &mut identity::Registry,
) -> ImplParts<'a> {
    let mut vertices = Vec::new();
    let mut edges = Vec::new();
    let mut attr_refs = Vec::new();
    let mut bodies = Vec::new();
    let mut entry_roots = Vec::new();
    let mut foreign_bodies = Vec::new();
    for b in blocks {
        // self 타입을 해석한다 — 모듈 로컬이면 정점이 있다.
        let Some(self_id) = tree.resolve(&b.items_module, &b.self_ty, &DepCrates::new()) else {
            harvest.unresolved_paths += 1;
            harvest.foreign_self_impls += 1;
            foreign_bodies.extend(foreign_impl_bodies(
                b,
                tree,
                methods,
                harvest,
                &mut attr_refs,
            ));
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
        for &m in &b.methods {
            let own_cfg = cfg_of(&m.attrs);
            harvest.cfg_items += usize::from(own_cfg.is_some());
            let mcfg = match own_cfg {
                Some(ref condition) => conjoin(&b.cfg, condition),
                None => b.cfg.clone(),
            };
            // 트레이트 impl 메서드는 `Type::<Trait>::m` — 고유 선언과 충돌 안 함.
            let mid = match &b.trait_path {
                Some(tp) => {
                    let tname = tp.last().cloned().unwrap_or_default();
                    format!("{self_id}::<{tname}>::{}", m.sig.ident)
                }
                None => format!("{self_id}::{}", m.sig.ident),
            };
            let body = Body::Stmts(&m.block.stmts);
            // unsafe impl의 메서드, unsafe fn 메서드, unsafe 본문 모두 경계다.
            let unsafe_ = b.unsafe_ || m.sig.unsafety.is_some() || body_has_unsafe(body);
            vertices.push(Vertex {
                id: mid.clone(),
                kind: Kind::Method,
                krate: krate.to_string(),
                module: b.items_module.clone(),
                position: Some(format!("{}:{}", b.file.display(), line_of(&m.sig.ident))),
                exported: matches!(m.vis, syn::Visibility::Public(_)),
                generated: b.generated,
                cfg: mcfg.clone(),
                unsafe_,
            });
            identities.note(
                vertices.last().expect("method vertex was just inserted"),
                &b.file,
                m.span().byte_range().start,
            );
            if is_extern_entry(&m.attrs) {
                entry_roots.push(mid.clone());
                identities.note_root(
                    &mid,
                    &b.items_module,
                    false,
                    &b.file,
                    m.span().byte_range().start,
                );
            }
            let mut contains = Edge::new(self_id.clone(), mid.clone(), EdgeKind::Contains);
            contains.cfg = mcfg.clone();
            contains.unsafe_ = b.unsafe_;
            edges.push(contains);
            // 메서드 속성의 경로 참조 — #[dep::attr] fn m()도 사용 증거다.
            // impl과 메서드 자신의 cfg를 둘 다 물린다 — 둘 다 성립해야
            // 이 참조가 존재한다.
            let first_attr = attr_refs.len();
            harvest.unparsed_attrs += collect_attr_refs(
                &m.attrs,
                &mid,
                &b.items_module,
                false,
                &mcfg,
                &mut attr_refs,
            );
            identities.note_attrs(
                &attr_refs[first_attr..],
                &b.file,
                m.span().byte_range().start,
            );
            bodies.push(BodyItem {
                generic_scopes: generic_scopes([b.generics, &m.sig.generics]),
                id: mid,
                module: b.items_module.clone(),
                self_ty: Some(b.self_ty.clone()),
                body,
                params: locals::param_bindings(&m.sig),
                signature_surface: fn_signature_types(&m.sig),
                cfg: mcfg,
                file: b.file.clone(),
                range: m.span().byte_range(),
            });
        }
    }
    ImplParts {
        vertices,
        edges,
        attr_refs,
        bodies,
        entry_roots,
        foreign_bodies,
    }
}

/// self 타입이 워크스페이스 정점이 아닌 impl 블록의 메서드 본문을 대신 소유시킨다.
///
/// 그런 impl(`impl Render for String`, blanket `impl<T: Tr> Greet for T`, `impl Tr for &Local`)의
/// 메서드에는 정점이 없다 — 예전에는 본문을 통째로 버려, 그 안에서만 부르는 함수가 unreachable로
/// 보였고 본문이 다른 모듈을 부르는 의존이 레이어 규칙에서 사라졌다. 구현한 트레이트가 워크스페이스
/// 트레이트이고 그 메서드를 선언했으면 선언 메서드(`Trait::m`)가 본문을 소유한다 — 그 메서드를 부르는
/// 곳(이름 팬아웃 포함)에서 본문의 호출이 닿는다. 아니면 impl이 선언된 모듈이 소유한다 — 의존은
/// 규칙·순환에 보이지만 모듈은 보존 루트가 아니어서 dead 판정은 바뀌지 않는다. impl 메서드의 `Self`는
/// 워크스페이스 밖 타입이라 바꾸지 않는다.
fn foreign_impl_bodies<'a>(
    b: &ImplBlock<'a>,
    tree: &ModTree,
    methods: &BTreeSet<String>,
    harvest: &mut Harvest,
    attr_refs: &mut Vec<AttrRef>,
) -> Vec<BodyItem<'a>> {
    let trait_id = b
        .trait_path
        .as_ref()
        .and_then(|tp| tree.resolve(&b.items_module, tp, &DepCrates::new()));
    let mut bodies = Vec::new();
    for &m in &b.methods {
        let owner = trait_id
            .as_ref()
            .map(|t| format!("{t}::{}", m.sig.ident))
            .filter(|declared| methods.contains(declared))
            .unwrap_or_else(|| b.items_module.clone());
        let own_cfg = cfg_of(&m.attrs);
        harvest.cfg_items += usize::from(own_cfg.is_some());
        let mcfg = match own_cfg {
            Some(ref condition) => conjoin(&b.cfg, condition),
            None => b.cfg.clone(),
        };
        harvest.unparsed_attrs +=
            collect_attr_refs(&m.attrs, &owner, &b.items_module, false, &mcfg, attr_refs);
        bodies.push(BodyItem {
            generic_scopes: generic_scopes([b.generics, &m.sig.generics]),
            id: owner,
            module: b.items_module.clone(),
            self_ty: None,
            body: Body::Stmts(&m.block.stmts),
            params: locals::param_bindings(&m.sig),
            signature_surface: fn_signature_types(&m.sig),
            cfg: mcfg,
            file: b.file.clone(),
            range: m.span().byte_range(),
        });
    }
    bodies
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

/// 모은 선언 참조([`DeclRef`])를 references 간선으로 해석한다(2패스).
///
/// 본문 방문의 타입 경로처럼 해석 실패를 미해석으로 세지 않는다 — 필드 타입은 대부분 std 타입이고
/// 매크로 본문의 경로는 메타변수·외부 경로가 섞여 있어, 세면 외부 참조 카운터가 의미 없이 부푼다.
/// 같은 (소유자, 목적지, 조건, 추정 여부) 간선은 한 번만 낸다. 소유자 자신을 가리키는 참조
/// (`Option<Box<Self>>`)는 버린다.
pub fn decl_edges(refs: &[DeclRef], tree: &ModTree, dep_crates: &DepCrates) -> Vec<Edge> {
    let mut seen = BTreeSet::new();
    let mut edges = Vec::new();
    for r in refs {
        let Some(to) = tree.resolve(&r.module, &r.path, dep_crates) else {
            continue;
        };
        if to == r.owner || !seen.insert((r.owner.as_str(), to.clone(), &r.cfg, r.tentative)) {
            continue;
        }
        let mut e = if r.tentative {
            Edge::maybe(r.owner.clone(), to, EdgeKind::References)
        } else {
            Edge::new(r.owner.clone(), to, EdgeKind::References)
        };
        e.cfg = r.cfg.clone();
        edges.push(e);
    }
    edges
}

/// 필드 타입 참조를 모으는 소유 타입 — 정점 ID·모듈·제네릭·선언 위치를 한데 묶는다.
struct DeclOwner<'a> {
    id: &'a str,
    module: &'a str,
    /// 필드 타입의 `Self`가 가리키는 타입 이름.
    name: String,
    /// 타입 이름공간을 가리는 제네릭 매개변수 — 필드의 `T`는 경로가 아니다.
    type_params: BTreeSet<String>,
    file: &'a PathBuf,
    start: usize,
}

impl<'a> DeclOwner<'a> {
    /// 타입 선언 하나의 소유자.
    fn new(
        id: &'a str,
        module: &'a str,
        ident: &syn::Ident,
        generics: &syn::Generics,
        file: &'a PathBuf,
        item: &syn::Item,
    ) -> DeclOwner<'a> {
        DeclOwner {
            id,
            module,
            name: ident.to_string(),
            type_params: generics
                .type_params()
                .map(|param| param.ident.to_string())
                .collect(),
            file,
            start: item.span().byte_range().start,
        }
    }

    /// 필드 (타입, 조건) 목록의 경로를 선언 참조로 넣는다.
    fn push_fields(&self, fields: Vec<(&syn::Type, Option<String>)>, out: &mut Vec<DeclRef>) {
        for (ty, cfg) in fields {
            let mut paths = TypeVisitor { paths: Vec::new() };
            paths.visit_type(ty);
            for path in paths.paths {
                if path
                    .first()
                    .is_some_and(|name| self.type_params.contains(name))
                {
                    continue;
                }
                let path = substitute_self(&path, std::slice::from_ref(&self.name)).unwrap_or(path);
                out.push(DeclRef {
                    owner: self.id.to_string(),
                    module: self.module.to_string(),
                    path,
                    cfg: cfg.clone(),
                    tentative: false,
                    file: self.file.clone(),
                    start: self.start,
                });
            }
        }
    }
}

/// struct·배리언트 필드마다 (타입, 아이템 조건과 필드 `#[cfg]`를 합친 조건).
fn fields_with_cfg<'f>(
    fields: &'f syn::Fields,
    cfg: &Option<String>,
) -> Vec<(&'f syn::Type, Option<String>)> {
    fields.iter().map(|f| (&f.ty, field_cfg(f, cfg))).collect()
}

/// 필드 하나의 조건 — 필드 자신의 `#[cfg]`가 있으면 아이템 조건과 합친다.
fn field_cfg(field: &syn::Field, cfg: &Option<String>) -> Option<String> {
    match cfg_of(&field.attrs) {
        Some(condition) => conjoin(cfg, &condition),
        None => cfg.clone(),
    }
}

/// `macro_rules!` 정의 본문의 토큰에서 두 세그먼트 이상의 경로(`a::b`, `crate::a`, `$crate::a`,
/// `self::a`, `super::a`)를 모은다. `$crate`는 `crate`로 바꾼다. 메타변수(`$x`)로 시작하는 경로는
/// 펼친 곳의 토큰이라 건너뛴다. 한 세그먼트 이름은 지역 변수·키워드와 구별할 수 없어 모으지 않는다.
fn macro_body_paths(tokens: proc_macro2::TokenStream) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    collect_token_paths(tokens, &mut out);
    out
}

/// [`macro_body_paths`]의 재귀 — 괄호 묶음 안까지 내려간다.
fn collect_token_paths(tokens: proc_macro2::TokenStream, out: &mut Vec<Vec<String>>) {
    use proc_macro2::TokenTree;
    let trees: Vec<TokenTree> = tokens.into_iter().collect();
    let mut i = 0;
    while i < trees.len() {
        if let TokenTree::Group(g) = &trees[i] {
            collect_token_paths(g.stream(), out);
            i += 1;
            continue;
        }
        if is_metavariable(&trees, i) {
            i = skip_path_tail(&trees, i + 2);
            continue;
        }
        let (path, next) = token_path_at(&trees, i);
        if path.len() >= 2 {
            out.push(path);
        }
        i = next.max(i + 1);
    }
}

/// `i`가 `$crate`가 아닌 메타변수(`$x`)의 시작인가.
fn is_metavariable(trees: &[proc_macro2::TokenTree], i: usize) -> bool {
    use proc_macro2::TokenTree;
    matches!(
        (trees.get(i), trees.get(i + 1)),
        (Some(TokenTree::Punct(p)), Some(TokenTree::Ident(id))) if p.as_char() == '$' && id != "crate"
    )
}

/// 메타변수 뒤에 이어진 `::이름` 꼬리를 건너뛴 위치.
fn skip_path_tail(trees: &[proc_macro2::TokenTree], mut i: usize) -> usize {
    while is_path_separator(trees, i)
        && matches!(trees.get(i + 2), Some(proc_macro2::TokenTree::Ident(_)))
    {
        i += 3;
    }
    i
}

/// `i`에서 시작하는 경로의 세그먼트와 경로 다음 위치 — 경로가 아니면 빈 목록과 `i`.
fn token_path_at(trees: &[proc_macro2::TokenTree], mut i: usize) -> (Vec<String>, usize) {
    use proc_macro2::TokenTree;
    let mut segs = Vec::new();
    loop {
        match (trees.get(i), trees.get(i + 1)) {
            (Some(TokenTree::Punct(p)), Some(TokenTree::Ident(id)))
                if p.as_char() == '$' && id == "crate" =>
            {
                segs.push("crate".to_string());
                i += 2;
            }
            (Some(TokenTree::Ident(id)), _) => {
                segs.push(id.to_string());
                i += 1;
            }
            _ => break,
        }
        if !is_path_separator(trees, i) {
            break;
        }
        i += 2;
    }
    (segs, i)
}

/// `i`와 `i + 1`이 경로 구분자 `::`인가.
fn is_path_separator(trees: &[proc_macro2::TokenTree], i: usize) -> bool {
    use proc_macro2::{Spacing, TokenTree};
    matches!(
        (trees.get(i), trees.get(i + 1)),
        (Some(TokenTree::Punct(a)), Some(TokenTree::Punct(b)))
            if a.as_char() == ':' && a.spacing() == Spacing::Joint && b.as_char() == ':'
    )
}

/// 본문을 방문해 call/references/매크로 간선을 만든다(2패스).
/// 이름 팬아웃은 `method_index`의 같은 이름 메서드 전부를 후보로 삼는다(크레이트로 좁히지 않는다 —
/// 의존 그래프를 모른다). 후보가 팬아웃 폭 상한을 넘는 호출은 간선 없이 [`Harvest`]에 센다.
/// 워크스페이스 수확은 의존 폐포로 좁히는 변형을 쓴다.
pub fn bodies(
    items: &[BodyItem],
    tree: &ModTree,
    dep_crates: &DepCrates,
    method_index: &BTreeMap<String, Vec<String>>,
    harvest: &mut Harvest,
) -> Vec<Edge> {
    bodies_within(
        items,
        tree,
        dep_crates,
        method_index,
        &fanout::Reach::new(),
        harvest,
    )
}

/// [`bodies`]와 같되 이름 팬아웃의 고유·트레이트 선언 메서드 후보를 본문 크레이트의 의존 폐포
/// (`reach`)로 좁힌다 — 의존 관계가 없는 크레이트의 같은 이름 메서드로 간선이 퍼지지 않게 한다.
pub(crate) fn bodies_within(
    items: &[BodyItem],
    tree: &ModTree,
    dep_crates: &DepCrates,
    method_index: &BTreeMap<String, Vec<String>>,
    reach: &fanout::Reach,
    harvest: &mut Harvest,
) -> Vec<Edge> {
    let mut edges = Vec::new();
    for b in items {
        let visible = reach.get(&crate_of(&b.module));
        edges.extend(body_edges_within(
            b,
            tree,
            dep_crates,
            method_index,
            visible,
            harvest,
        ));
        edges.extend(signature_edges(b, tree, dep_crates));
    }
    edges
}

/// 본문 하나의 syn 방문 — 표현식 안의 호출·참조·매크로 간선.
/// semantic 엔진이 못 보는 본문(cfg 비활성·매크로 생성 정의)의 폴백이기도 하다.
/// 이름 팬아웃은 [`bodies`]와 같이 좁히지 않는다.
///
/// 본문 블록의 최상위 문장은 중첩 블록의 문장과 규칙이 다르다. 최상위에서는 지역 겹을 새로 열지
/// 않고(`let` 묶음은 매개변수 겹에 들어간다), `let .. else`의 else 블록을 초기식보다 먼저 보며,
/// 문장 매크로(`;`로 끝나거나 중괄호로 감싼 문장 위치 매크로 호출)는 표현식 매크로처럼 인자를
/// 파싱한다 — 중첩 블록의 문장 매크로도 같다. 함수 안 아이템(`fn` 안의 `fn`, 지역 `impl`)은
/// 최상위든 중첩 블록이든 본다 — 정점이 없는 그 아이템의 간선은 바깥 본문이 소유한다.
pub fn body_edges(
    b: &BodyItem,
    tree: &ModTree,
    dep_crates: &DepCrates,
    method_index: &BTreeMap<String, Vec<String>>,
    harvest: &mut Harvest,
) -> Vec<Edge> {
    body_edges_within(b, tree, dep_crates, method_index, None, harvest)
}

/// [`body_edges`]의 본체 — `visible`(본문 크레이트의 의존 폐포, None이면 좁히지 않음)로 이름
/// 팬아웃의 고유·트레이트 선언 메서드 후보를 좁힌다.
fn body_edges_within(
    b: &BodyItem,
    tree: &ModTree,
    dep_crates: &DepCrates,
    method_index: &BTreeMap<String, Vec<String>>,
    visible: Option<&BTreeSet<String>>,
    harvest: &mut Harvest,
) -> Vec<Edge> {
    let mut params = b.params.clone();
    params.extend(
        b.generic_scopes
            .iter()
            .flat_map(|scope| scope.const_params())
            .map(|param| param.ident.to_string()),
    );
    let mut vis = BodyVisitor {
        type_params: generic_type_parameters(b),
        owner: &b.id,
        module: &b.module,
        self_ty: b.self_ty.as_deref(),
        tree,
        dep_crates,
        method_index,
        visible,
        edge_cfg: &b.cfg,
        in_unsafe: 0,
        scopes: locals::Scopes::with_params(&params),
        edges: Vec::new(),
        unresolved: 0,
        fanned: 0,
        out_of_reach: 0,
        capped: 0,
        capped_names: BTreeSet::new(),
        ext_macros: 0,
    };
    vis.visit_body(b.body);
    harvest.unresolved_paths += vis.unresolved;
    harvest.fanned_method_calls += vis.fanned;
    harvest.out_of_reach_method_calls += vis.out_of_reach;
    harvest.capped_method_calls += vis.capped;
    harvest.capped_method_names.append(&mut vis.capped_names);
    harvest.external_macros += vis.ext_macros;
    vis.edges
}

/// 시그니처 표면(파라미터·반환)의 타입 참조를 signature 간선으로 만든다.
/// semantic 엔진이 본문을 맡을 때도 이 부분은 syn이 권위다 — 두 경로가
/// 같은 간선을 내므로 엔진 선택과 무관하게 일관된다.
pub fn signature_edges(b: &BodyItem, tree: &ModTree, dep_crates: &DepCrates) -> Vec<Edge> {
    let mut edges = Vec::new();
    let type_params = generic_type_parameters(b);
    // 소유 아이템이 cfg면 시그니처 자체가 그 조건 아래 있으니 간선도 물려받는다.
    for t in &b.signature_surface {
        let mut sv = TypeVisitor { paths: Vec::new() };
        sv.visit_type(t);
        for p in sv.paths {
            if p.first().is_some_and(|name| type_params.contains(name)) {
                continue;
            }
            // `-> Self`·`other: &Self`는 impl self 타입이다.
            let p = b
                .self_ty
                .as_deref()
                .and_then(|ty| substitute_self(&p, ty))
                .unwrap_or(p);
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
    type_params: BTreeSet<String>,
    owner: &'a str,
    module: &'a str,
    self_ty: Option<&'a [String]>,
    tree: &'a ModTree,
    dep_crates: &'a DepCrates,
    method_index: &'a BTreeMap<String, Vec<String>>,
    /// 본문 크레이트의 의존 폐포 — 이름 팬아웃의 고유·트레이트 선언 메서드 후보를 좁힌다(None이면
    /// 좁히지 않는다).
    visible: Option<&'a BTreeSet<String>>,
    /// 소유 아이템의 cfg — 이 본문의 간선은 전부 그 조건 아래 있다.
    edge_cfg: &'a Option<String>,
    /// 현재 unsafe 블록 깊이 — 0보다 크면 간선에 unsafe를 찍는다.
    in_unsafe: usize,
    /// 지역 묶음 스택 — 단일 식별자 경로는 아이템보다 지역을 먼저 본다.
    scopes: locals::Scopes,
    edges: Vec<Edge>,
    unresolved: usize,
    fanned: usize,
    out_of_reach: usize,
    capped: usize,
    capped_names: BTreeSet<String>,
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

    /// 수신자를 모르는 `x.m()` — 같은 이름 메서드 중 이 본문 크레이트가 닿을 수 있는 후보에 추정
    /// 간선을 단다([`fanout::select`]). 같은 이름 메서드가 워크스페이스에 없으면 세지 않는다.
    fn fan_out(&mut self, name: String) {
        let Some(ids) = self.method_index.get(&name) else {
            return;
        };
        match fanout::select(ids, self.visible) {
            Fanned::Candidates(picked) => {
                for mid in picked {
                    self.push_maybe(mid.to_string(), EdgeKind::Call);
                }
                self.fanned += 1;
            }
            Fanned::OutOfReach => self.out_of_reach += 1,
            Fanned::Capped => {
                self.capped += 1;
                self.capped_names.insert(name);
            }
        }
    }

    /// 경로를 이 본문의 모듈에서 해석한다. impl 본문의 `Self`는 impl self 타입 경로로 바꿔 해석한다
    /// ([`substitute_self`]) — `Self::f()`·`Self(..)`·`Self { .. }`·`Self::CONST`가 모두 그 타입을 가리킨다.
    fn resolve(&mut self, segs: &[String]) -> Option<String> {
        match self.self_ty.and_then(|ty| substitute_self(segs, ty)) {
            Some(path) => self.tree.resolve(self.module, &path, self.dep_crates),
            None => self.tree.resolve(self.module, segs, self.dep_crates),
        }
    }

    /// 값 자리(표현식 경로·호출 대상·포맷 캡처)의 경로를 해석한다.
    ///
    /// rustc의 값 이름공간 규칙 두 가지를 지킨다. 단일 식별자가 지역
    /// 묶음이거나 `self` 수신자면 아이템을 찾지 않는다(지역이 먼저다).
    /// 해석 결과가 워크스페이스 모듈이면 버린다 — 모듈은 타입 이름공간에만
    /// 있어 값이 될 수 없으므로, 그런 해석은 지역 변수를 같은 이름의
    /// 모듈로 오독한 것이다. 외부 크레이트로 붕괴한 정점은 모듈 표에 없어
    /// 그대로 남는다(`dep::f()`는 크레이트 경계 증거다).
    fn resolve_value(&mut self, segs: &[String]) -> ValueRes {
        if segs.len() > 1
            && segs
                .first()
                .is_some_and(|name| self.type_params.contains(name))
        {
            return ValueRes::Local;
        }
        if let [only] = segs {
            if only == "self" || self.scopes.is_local(only) {
                return ValueRes::Local;
            }
        }
        match self.resolve(segs) {
            Some(id) if self.tree.modules.contains_key(&id) && !self.value_item(&id) => {
                ValueRes::Local
            }
            Some(id) => ValueRes::Item(id),
            None => ValueRes::Unresolved,
        }
    }

    /// 모듈 경로가 같은 이름의 값 아이템(fn·const·static)과도 겹치는가 — 그때 값
    /// 자리의 해석은 그 아이템이다(모듈과 값은 이름공간이 다르다).
    fn value_item(&self, id: &str) -> bool {
        let Some((parent, name)) = id.rsplit_once("::") else {
            return false;
        };
        self.tree
            .modules
            .get(parent)
            .is_some_and(|m| m.value_items.contains(name))
    }

    /// 타입 `owner` 범위에서 이름이 `name`인 메서드(고유·트레이트 impl)로 references 간선을 단다 —
    /// 연관 함수를 값으로 넘긴 경로(`Type::new`)다. 같은 이름 메서드가 없으면(배리언트·연관 상수)
    /// 아무것도 하지 않는다.
    fn push_associated(&mut self, owner: &str, name: &str) {
        let index = self.method_index;
        let Some(ids) = index.get(name) else {
            return;
        };
        let prefix = owner_prefix(owner);
        for mid in ids.iter().filter(|mid| mid.starts_with(&prefix)) {
            self.push(mid.clone(), EdgeKind::References);
        }
    }

    /// `<T as Tr>::m(..)`·`<T>::m(..)` — self 타입이 적힌 정적 호출이다. `T`가 워크스페이스 타입이면
    /// 그 타입의 메서드로 call 간선을 단다: 트레이트를 적었으면 그 트레이트 impl의 `T::<Tr>::m`(정확히
    /// 없으면 같은 이름 impl 메서드 후보를 추정 간선으로), 적지 않았으면 `T` 범위의 같은 이름 메서드다.
    /// 예전에는 트레이트 선언(`Tr::m`)에만 닿아 impl 메서드와 그 안에서만 부르는 함수가 unreachable이었다.
    ///
    /// 반환: 경로 해석을 이것으로 대신했는가 — `<T>::m`은 타입 상대 경로라 값 경로로 해석하면 같은
    /// 이름의 자유 함수로 오독한다. `<T as Tr>::m`은 트레이트 선언으로 가는 기존 해석도 그대로 둔다.
    fn qualified_self_call(&mut self, qself: &syn::QSelf, segs: &[String]) -> bool {
        let type_relative = qself.position == 0;
        let (Some(method), syn::Type::Path(ty)) = (segs.last(), &*qself.ty) else {
            return type_relative;
        };
        let index = self.method_index;
        let (Some(owner), Some(ids)) = (self.resolve(&path_segments(&ty.path)), index.get(method))
        else {
            self.unresolved += usize::from(type_relative);
            return type_relative;
        };
        if type_relative {
            let prefix = owner_prefix(&owner);
            let mut hit = false;
            for mid in ids.iter().filter(|mid| mid.starts_with(&prefix)) {
                self.push(mid.clone(), EdgeKind::Call);
                hit = true;
            }
            self.unresolved += usize::from(!hit);
            return true;
        }
        let trait_name = &segs[qself.position - 1];
        let exact = format!("{owner}::<{trait_name}>::{method}");
        if ids.contains(&exact) {
            self.push(exact, EdgeKind::Call);
        } else {
            // 별칭·재노출 이름으로 적은 트레이트 — 같은 타입의 같은 이름 impl 메서드 중 하나다.
            let (prefix, suffix) = (format!("{owner}::<"), format!(">::{method}"));
            for mid in ids
                .iter()
                .filter(|mid| mid.starts_with(&prefix) && mid.ends_with(&suffix))
            {
                self.push_maybe(mid.clone(), EdgeKind::Call);
            }
        }
        false
    }

    /// 본문을 아레나에서 바로 방문한다 — 최상위 문장 규칙은 [`body_edges`] 문서에 있다.
    /// 최상위를 [`Visit::visit_block`]으로 돌리면 겹이 하나 더 열리고 `let .. else`의 방문 순서가
    /// 달라져 간선이 달라진다.
    fn visit_body(&mut self, body: Body<'_>) {
        let stmts = match body {
            Body::Expr(e) => return self.visit_expr(e),
            Body::Stmts(stmts) => stmts,
        };
        for stmt in stmts {
            match stmt {
                syn::Stmt::Expr(e, _) => self.visit_expr(e),
                syn::Stmt::Local(l) => self.visit_top_local(l),
                // `println!(..);`·`name! { .. }` 같은 문장 매크로 — 표현식 매크로와 같은 처리다.
                syn::Stmt::Macro(m) => {
                    self.visit_macro_call(&m.mac);
                    syn::visit::visit_stmt_macro(self, m);
                }
                // 함수 안 아이템(`fn inner`, 지역 `impl`, 지역 `macro_rules!`)은 바깥 함수가 소유한
                // 것으로 본다 — 그 아이템에는 정점이 없어, 보지 않으면 그 본문에서만 부르는
                // 함수가 unreachable로 보였다. 규칙은 중첩 블록의 아이템과 같다([`Visit::visit_item`]).
                syn::Stmt::Item(i) => self.visit_item(i),
            }
        }
    }

    /// 최상위 `let` — else 블록, 초기식, 패턴 순으로 보고 이름을 매개변수 겹에 묶는다. else
    /// 블록은 새 묶음이 보이지 않는 곳이라 묶기 전에 보고, 초기식도 새 묶음을 못 본다
    /// (`let x = x + 1`의 오른쪽은 바깥 x). 속성은 보지 않는다.
    fn visit_top_local(&mut self, l: &syn::Local) {
        if let Some(init) = &l.init {
            if let Some((_, diverge)) = &init.diverge {
                self.visit_expr(diverge);
            }
            self.visit_expr(&init.expr);
        }
        syn::visit::visit_pat(self, &l.pat);
        self.scopes.bind(&l.pat);
    }

    /// 매크로 호출 하나 — 크레이트 안 매크로면 call 간선, std/외부면 실측 생략으로 센다.
    /// 매크로 인자는 토큰 스트림이라 방문자가 못 내린다 — write!·format!·assert_eq! 같은 흔한
    /// 형태는 쉼표로 나뉜 표현식 목록이라 직접 파싱을 시도한다. 파싱이 안 되면 매크로 문법이
    /// 독자적이라는 뜻이고, 그 안의 호출은 구문 수준에서 영원히 보이지 않는다(ext_macros가 그
    /// 수를 잰다).
    fn visit_macro_call(&mut self, mac: &syn::Macro) {
        let segs = path_segments(&mac.path);
        match self.resolve(&segs) {
            Some(id) => self.push(id, EdgeKind::Call),
            None => self.ext_macros += 1,
        }
        if let Ok(exprs) =
            syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated
                .parse2(mac.tokens.clone())
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
                        if let ValueRes::Item(id) = self.resolve_value(&[name]) {
                            self.push(id, EdgeKind::References);
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
thread_local! {
    /// 타입 범위 메서드 후보 접두사를 만든 횟수 — 호출마다 한 번인지(후보마다가 아닌지)
    /// 단위 테스트가 센다(테스트 스레드마다 따로).
    static OWNER_PREFIXES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// 테스트용 — 이 스레드에서 지금까지 만든 타입 범위 메서드 후보 접두사 수.
#[cfg(test)]
pub(crate) fn owner_prefixes() -> usize {
    OWNER_PREFIXES.with(std::cell::Cell::get)
}

/// 타입 범위 메서드 후보를 고르는 접두사 `{owner}::`다. 고유 메서드 `T::m`과 트레이트
/// impl 메서드 `T::<Tr>::m`이 모두 이 아래 있어, `Type::assoc()`·`self.m()`이 두 쪽을
/// 함께 잡는다. `::`까지 붙여야 이름이 접두사인 다른 타입(`T1`과 `T10`)을 거른다.
fn owner_prefix(owner: &str) -> String {
    #[cfg(test)]
    OWNER_PREFIXES.with(|c| c.set(c.get() + 1));
    format!("{owner}::")
}

/// 값 자리 경로의 해석 결과다.
enum ValueRes {
    /// 크레이트 아이템(또는 외부 크레이트 경계 정점).
    Item(String),
    /// 지역 묶음·`self`·값이 될 수 없는 모듈 — 간선도 미해석 계수도 없다.
    Local,
    /// 아이템 표에서 찾지 못함.
    Unresolved,
}

impl Visit<'_> for BodyVisitor<'_> {
    fn visit_type_path(&mut self, ty: &syn::TypePath) {
        // 타입 경로만 추가하고 기본 방문을 유지해야 배열 길이·const 인자도 사라지지 않는다.
        let path = path_segments(&ty.path);
        if !path
            .first()
            .is_some_and(|name| self.type_params.contains(name))
        {
            if let Some(id) = self.resolve(&path) {
                self.push(id, EdgeKind::References);
            }
        }
        syn::visit::visit_type_path(self, ty);
    }

    fn visit_type_macro(&mut self, ty: &syn::TypeMacro) {
        self.visit_macro_call(&ty.mac);
    }

    fn visit_expr_call(&mut self, e: &syn::ExprCall) {
        // foo::bar() — 경로 호출은 정확히 해석된다.
        if let syn::Expr::Path(p) = &*e.func {
            let segs = path_segments(&p.path);
            let type_relative = p
                .qself
                .as_ref()
                .is_some_and(|qself| self.qualified_self_call(qself, &segs));
            match if type_relative {
                ValueRes::Local
            } else {
                self.resolve_value(&segs)
            } {
                ValueRes::Item(id) => self.push(id, EdgeKind::Call),
                // 지역 클로저·함수 값 호출 — 아이템 호출이 아니다.
                ValueRes::Local => {}
                ValueRes::Unresolved => {
                    // Type::assoc_fn() — 마지막 세그먼트가 연관 함수다.
                    // 앞부분이 타입 정점이면 그 타입의 메서드로 좁힌다.
                    let n = segs.len();
                    let mut hit = false;
                    if n >= 2 {
                        if let Some(ty) = self.resolve(&segs[..n - 1]) {
                            if let Some(ids) = self.method_index.get(&segs[n - 1]) {
                                // 접두사는 호출마다 한 번 — 후보마다 만들면 같은 이름 정의가
                                // 수천 개일 때 할당이 (호출 수 × 정의 수)다.
                                let prefix = owner_prefix(&ty);
                                for mid in ids {
                                    if mid.starts_with(&prefix) {
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
            // 호출 경로 자체는 중복 계수하지 않되, 그 안의 타입 인자와 qself는 참조다.
            if let Some(qself) = &p.qself {
                self.visit_type(&qself.ty);
                let path = path_segments(&p.path);
                if qself.position > 0 {
                    if let Some(id) = self.resolve(&path[..qself.position]) {
                        self.push(id, EdgeKind::References);
                    }
                }
            }
            for segment in &p.path.segments {
                syn::visit::visit_path_arguments(self, &segment.arguments);
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
                        let prefix = owner_prefix(&sid);
                        for mid in ids {
                            if mid.starts_with(&prefix) {
                                self.push_maybe(mid.clone(), EdgeKind::Call);
                            }
                        }
                    }
                    scoped = true;
                }
            }
        }
        if !scoped {
            self.fan_out(name);
        }
        syn::visit::visit_expr_method_call(self, e);
    }

    fn visit_expr_macro(&mut self, e: &syn::ExprMacro) {
        // name!() — 표현식 위치 매크로(최상위 문장 매크로도 같은 처리를 탄다).
        self.visit_macro_call(&e.mac);
        syn::visit::visit_expr_macro(self, e);
    }

    fn visit_stmt_macro(&mut self, m: &syn::StmtMacro) {
        // 중첩 블록(`for`·`if`·`match` 팔 본문)의 문장 매크로 — 최상위 문장 매크로와 같이 인자를
        // 파싱한다. 예전에는 syn 기본 순회만 따라 `for x in xs { println!("{}", f(x)); }`의 `f`가
        // 간선 없이 사라졌다. 최상위 문장은 [`BodyVisitor::visit_body`]가 따로 처리한다.
        self.visit_macro_call(&m.mac);
        syn::visit::visit_stmt_macro(self, m);
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

    // ── 지역 스코프 관리 — 묶음을 만드는 구문마다 겹을 열고 닫는다. ──

    fn visit_expr_let(&mut self, e: &syn::ExprLet) {
        // 초기식은 새 묶음을 못 본다(`let x = x + 1`의 오른쪽은 바깥 x).
        self.visit_expr(&e.expr);
        syn::visit::visit_pat(self, &e.pat);
        self.scopes.bind(&e.pat);
    }

    fn visit_block(&mut self, b: &syn::Block) {
        self.scopes.push();
        for stmt in &b.stmts {
            match stmt {
                syn::Stmt::Local(l) => {
                    if let Some(init) = &l.init {
                        self.visit_expr(&init.expr);
                        if let Some((_, diverge)) = &init.diverge {
                            self.visit_expr(diverge);
                        }
                    }
                    syn::visit::visit_pat(self, &l.pat);
                    self.scopes.bind(&l.pat);
                }
                other => self.visit_stmt(other),
            }
        }
        self.scopes.pop();
    }

    fn visit_item(&mut self, i: &syn::Item) {
        // 블록 안 아이템은 바깥 지역을 캡처하지 않는다. 블록 안 fn은 자기
        // 매개변수를 첫 겹으로 가진다.
        let saved = self.scopes.take();
        let saved_types = self.type_params.clone();
        // 안쪽 아이템은 바깥 impl의 `Self`를 쓸 수 없다(rustc E0401) — 안쪽 `impl`의 `Self`는
        // 그 impl의 타입이라, 바깥 self 타입으로 바꾸면 거짓 간선이 된다. 안쪽에서는 바꾸지 않는다.
        let saved_self = self.self_ty.take();
        let generics = match i {
            syn::Item::Fn(item) => Some(&item.sig.generics),
            syn::Item::Struct(item) => Some(&item.generics),
            syn::Item::Enum(item) => Some(&item.generics),
            syn::Item::Union(item) => Some(&item.generics),
            syn::Item::Trait(item) => Some(&item.generics),
            syn::Item::TraitAlias(item) => Some(&item.generics),
            syn::Item::Impl(item) => Some(&item.generics),
            syn::Item::Type(item) => Some(&item.generics),
            _ => None,
        };
        let mut params = Vec::new();
        if let syn::Item::Fn(f) = i {
            params = locals::param_bindings(&f.sig);
        }
        if let Some(generics) = generics {
            self.type_params
                .extend(generics.type_params().map(|param| param.ident.to_string()));
            params.extend(generics.const_params().map(|param| param.ident.to_string()));
        }
        self.scopes = locals::Scopes::with_params(&params);
        if let syn::Item::Macro(m) = i {
            if m.ident.is_some() && m.mac.path.is_ident("macro_rules") {
                // 지역 `macro_rules!` 정의 본문의 경로 — 펼친 곳에서 쓰이는 이름이라 추정 간선이다.
                for path in macro_body_paths(m.mac.tokens.clone()) {
                    if let Some(id) = self.resolve(&path) {
                        self.push_maybe(id, EdgeKind::References);
                    }
                }
            }
        }
        syn::visit::visit_item(self, i);
        self.scopes.restore(saved);
        self.type_params = saved_types;
        self.self_ty = saved_self;
    }

    fn visit_expr_if(&mut self, e: &syn::ExprIf) {
        // `if let` 묶음은 조건과 then 블록에서만 보이고 else에서는 안 보인다.
        self.scopes.push();
        self.visit_expr(&e.cond);
        self.visit_block(&e.then_branch);
        self.scopes.pop();
        if let Some((_, els)) = &e.else_branch {
            self.visit_expr(els);
        }
    }

    fn visit_expr_while(&mut self, e: &syn::ExprWhile) {
        self.scopes.push();
        self.visit_expr(&e.cond);
        self.visit_block(&e.body);
        self.scopes.pop();
    }

    fn visit_expr_for_loop(&mut self, e: &syn::ExprForLoop) {
        self.visit_expr(&e.expr);
        self.scopes.push();
        syn::visit::visit_pat(self, &e.pat);
        self.scopes.bind(&e.pat);
        self.visit_block(&e.body);
        self.scopes.pop();
    }

    fn visit_expr_match(&mut self, e: &syn::ExprMatch) {
        self.visit_expr(&e.expr);
        for arm in &e.arms {
            self.scopes.push();
            syn::visit::visit_pat(self, &arm.pat);
            self.scopes.bind(&arm.pat);
            if let Some((_, guard)) = &arm.guard {
                self.visit_expr(guard);
            }
            self.visit_expr(&arm.body);
            self.scopes.pop();
        }
    }

    fn visit_expr_closure(&mut self, e: &syn::ExprClosure) {
        self.scopes.push();
        for input in &e.inputs {
            syn::visit::visit_pat(self, input);
            self.scopes.bind(input);
        }
        if let syn::ReturnType::Type(_, ty) = &e.output {
            self.visit_type(ty);
        }
        self.visit_expr(&e.body);
        self.scopes.pop();
    }

    fn visit_expr_path(&mut self, e: &syn::ExprPath) {
        // 호출이 아닌 경로 참조 — filter_map(f) 같은 함수 값 포함.
        // 단일 식별자는 모듈 아이템이면 잡고 지역 변수면 조용히 넘긴다.
        let segs = path_segments(&e.path);
        match self.resolve_value(&segs) {
            ValueRes::Item(id) => self.push(id, EdgeKind::References),
            ValueRes::Local => {}
            ValueRes::Unresolved => {
                // E::V 형태 — 마지막 세그먼트가 열거형 배리언트/상수면
                // 앞부분 타입에의 참조다. 그것도 안 되면 진짜 미해석.
                if segs.len() >= 2 {
                    match self.resolve(&segs[..segs.len() - 1]) {
                        Some(ty) => {
                            // `.map(Type::new)`·`.ok_or_else(Type::err)`처럼 연관 함수를 값으로 넘기면
                            // 그 메서드의 참조다 — 타입만 잇던 때는 그렇게만 쓰는 연관 함수가
                            // unreachable이었다. 호출(`Type::assoc()`)과 같은 타입 범위 후보를 쓴다.
                            self.push_associated(&ty, &segs[segs.len() - 1]);
                            self.push(ty, EdgeKind::References);
                        }
                        None => self.unresolved += 1,
                    }
                }
            }
        }
        syn::visit::visit_expr_path(self, e);
    }
}

/// 빈 제네릭 스코프는 저장하지 않아 일반 함수의 본문 메타데이터를 늘리지 않는다.
fn generic_scopes<'a>(
    scopes: impl IntoIterator<Item = &'a syn::Generics>,
) -> Vec<&'a syn::Generics> {
    scopes
        .into_iter()
        .filter(|scope| !scope.params.is_empty())
        .collect()
}

/// 타입 이름 공간을 가리는 제네릭 매개변수다. const는 값 묶음에서 따로 다룬다.
fn generic_type_parameters(body: &BodyItem<'_>) -> BTreeSet<String> {
    body.generic_scopes
        .iter()
        .flat_map(|scope| scope.type_params())
        .map(|param| param.ident.to_string())
        .collect()
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

/// `Self`로 시작하는 경로를 impl self 타입 경로로 바꾼다 — `Self`로 시작하지 않거나 self 타입
/// 경로가 비었으면(참조·튜플 같은 이름 없는 타입) None이다. `Self`는 본문이 속한 impl 모듈에서
/// self 타입과 같은 이름이므로, 바꾼 경로도 같은 모듈에서 해석한다.
fn substitute_self(segs: &[String], self_ty: &[String]) -> Option<Vec<String>> {
    let (first, rest) = segs.split_first()?;
    if first != "Self" || self_ty.is_empty() {
        return None;
    }
    Some(self_ty.iter().chain(rest).cloned().collect())
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
    text_has_generated_marker(&src)
}

#[cfg(test)]
thread_local! {
    /// 생성 코드 마커를 판정한 횟수 — 모듈마다가 아니라 파싱한 파일마다 한 번인지 단위
    /// 테스트가 센다(테스트 스레드마다 따로).
    static GENERATED_SCANS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// 테스트용 — 이 스레드에서 지금까지 생성 코드 마커를 판정한 횟수.
#[cfg(test)]
pub(crate) fn generated_marker_scans() -> usize {
    GENERATED_SCANS.with(std::cell::Cell::get)
}

/// 원문 앞 10줄에 생성 코드 마커가 있는지 본다 — 모든 정점(모듈·크레이트, 아이템, impl 메서드)의
/// generated 규칙 하나다. 한 줄에 'generated'와 함께 'do not edit'·'@generated'가 있거나,
/// 'auto-generated'·'autogenerated'가 있어야 참이다(대소문자 무시). 'do not edit'만으로는 참이
/// 아니다 — "DO NOT EDIT: keep in sync by hand"처럼 손으로 관리하는 파일도 그렇게 쓴다. 예전에는
/// 모듈 정점이 다른 규칙('do not edit'만 있어도 참, 'autogenerated'는 거짓)을 써서 같은 파일의
/// 모듈과 아이템이 서로 반대로 나왔다.
pub(crate) fn text_has_generated_marker(src: &str) -> bool {
    #[cfg(test)]
    GENERATED_SCANS.with(|c| c.set(c.get() + 1));
    src.lines().take(10).any(|l| {
        let l = l.to_lowercase();
        l.contains("generated") && (l.contains("do not edit") || l.contains("@generated"))
            || l.contains("auto-generated")
            || l.contains("autogenerated")
    })
}

/// 본문 안에 `unsafe {}` 블록이 있는지 본다 — 재귀 방문. 본문 방문([`body_edges`])처럼 최상위
/// 아이템은 보지 않는다. 매크로 토큰 안도 보지 않는다(syn 기본 순회는 토큰을 내리지 않는다).
fn body_has_unsafe(body: Body<'_>) -> bool {
    struct Finder(bool);
    impl Visit<'_> for Finder {
        fn visit_expr_unsafe(&mut self, _e: &syn::ExprUnsafe) {
            self.0 = true;
        }
    }
    let mut f = Finder(false);
    match body {
        Body::Expr(e) => f.visit_expr(e),
        Body::Stmts(stmts) => {
            for stmt in stmts {
                if f.0 {
                    break;
                }
                match stmt {
                    syn::Stmt::Item(_) => {}
                    // let은 속성·패턴·초기식·else 블록, 문장 매크로는 속성·경로를 본다.
                    other => f.visit_stmt(other),
                }
            }
        }
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

/// 링커·런타임·매크로 확장기가 부르는 진입점 속성 — 이 속성이 붙은 fn·static·impl 메서드는 그래프
/// 밖에서 불리므로 보존 루트다.
const ENTRY_ATTRIBUTES: &[&str] = &[
    "no_mangle",
    "export_name",
    "used",
    "proc_macro",
    "proc_macro_derive",
    "proc_macro_attribute",
    "global_allocator",
    "panic_handler",
];

/// no_mangle/export_name/used/proc_macro/global_allocator 계열 — 링커·런타임·매크로가 부르는 진입점.
///
/// Rust 2024가 요구하는 `#[unsafe(no_mangle)]`·`#[unsafe(export_name = "..")]`와
/// `#[cfg_attr(pred, no_mangle)]`의 안쪽 속성도 본다. 속성 경로의 마지막 세그먼트만 보면 앞의 둘은
/// `unsafe`·`cfg_attr`로 읽혀 2024 판 FFI export 전부가 unreachable로 보였다. cfg_attr 술어는
/// 평가하지 않는다 — 거짓일 수 있지만 보존하는 쪽으로 기운다.
pub fn is_extern_entry(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| meta_is_entry(&a.meta))
}

/// 속성 메타 하나가 진입점 속성인가 — `unsafe(..)`·`cfg_attr(pred, ..)` 안쪽까지 내려간다.
fn meta_is_entry(meta: &syn::Meta) -> bool {
    let Some(name) = meta.path().segments.last().map(|s| s.ident.to_string()) else {
        return false;
    };
    if ENTRY_ATTRIBUTES.contains(&name.as_str()) {
        return true;
    }
    let syn::Meta::List(list) = meta else {
        return false;
    };
    match name.as_str() {
        "unsafe" => syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated
            .parse2(list.tokens.clone())
            .is_ok_and(|inner| inner.iter().any(meta_is_entry)),
        "cfg_attr" => matches!(
            split_cfg_attr(&list.tokens),
            Some((_, Ok(inner))) if inner.iter().any(meta_is_entry)
        ),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `macro_rules!` 본문의 경로 — `$crate`는 `crate`로 바꾸고, 메타변수로 시작하는 경로와 한
    /// 세그먼트 이름은 버린다. 괄호 묶음 안까지 내려간다.
    #[test]
    fn macro_body_paths_keep_crate_paths_and_skip_metavariables() {
        let tokens: proc_macro2::TokenStream = syn::parse_str(
            "($t:ty, $e:expr) => { $crate::m::helper($e); <$t>::new(); $t::make(); \
             crate::a::b(); local(); { super::up::f() } }",
        )
        .unwrap();
        let paths = macro_body_paths(tokens);
        let paths: Vec<String> = paths.iter().map(|p| p.join("::")).collect();
        assert_eq!(paths, ["crate::m::helper", "crate::a::b", "super::up::f"]);
    }

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
