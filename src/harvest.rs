//! syn AST에서 정점과 간선을 수확한다.
//!
//! 구문 수준 수확이라 할 수 있는 것과 없는 것을 구분한다: 경로 호출·use·impl은
//! 정확히 해석되고, 메서드 호출(`x.m()`)은 타입이 없어 이름 팬아웃으로
//! 과대 근사한다 — 오탐은 "살아 있다" 쪽으로만 기울게 하는 계약이다.

use crate::graph::{Edge, EdgeKind, Kind, Vertex};
use crate::modtree::{has_cfg, ModTree};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use syn::parse::Parser;
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
}

/// 한 모듈의 아이템 목록을 정점으로 만든다(1패스 — 선언만).
/// 반환값: (이 모듈이 만든 정점들, 이 모듈의 impl 블록들).
pub struct ModuleDecls<'a> {
    pub vertices: Vec<Vertex>,
    pub impls: Vec<ImplBlock>,
    /// fn/method/const/static/macro 본문을 담은 항목(2패스용).
    pub bodies: Vec<BodyItem<'a>>,
    /// #[no_mangle]·proc_macro 같은 외부 호출 진입점 — 항상 보존 루트.
    pub entry_roots: Vec<String>,
    /// #[test]/#[bench] 진입점 — --tests일 때만 보존 루트가 된다.
    pub test_roots: Vec<String>,
}

/// impl 블록 — self 타입·트레이트·메서드 목록.
#[derive(Clone)]
pub struct ImplBlock {
    pub self_ty: Vec<String>,
    pub trait_path: Option<Vec<String>>,
    pub methods: Vec<syn::ImplItemFn>,
    pub items_module: String,
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

/// 모듈의 파일 AST를 1패스로 돌려 선언 정점을 수확한다.
pub fn decls<'a>(
    module_path: &str,
    krate: &str,
    items: &[&'a syn::Item],
    file: &Path,
    harvest: &mut Harvest,
) -> ModuleDecls<'a> {
    let mut out = ModuleDecls {
        vertices: Vec::new(),
        impls: Vec::new(),
        bodies: Vec::new(),
        entry_roots: Vec::new(),
        test_roots: Vec::new(),
    };
    let generated = file_has_generated_marker(file);
    for &item in items {
        let conditional = has_cfg(attrs_of(item));
        if conditional {
            harvest.cfg_items += 1;
        }
        let pos = position_of(file, item);
        let v = |id: String, kind: Kind, exported: bool| Vertex {
            id,
            kind,
            krate: krate.to_string(),
            module: module_path.to_string(),
            position: pos.clone(),
            exported,
            generated,
        };
        match item {
            syn::Item::Fn(f) => {
                let id = format!("{module_path}::{}", f.sig.ident);
                out.vertices.push(v(id.clone(), Kind::Fn, is_pub(&f.vis)));
                if is_extern_entry(&f.attrs) {
                    out.entry_roots.push(id.clone());
                } else if is_test_entry(&f.attrs) {
                    out.test_roots.push(id.clone());
                }
                out.bodies.push(BodyItem {
                    id,
                    module: module_path.to_string(),
                    self_ty: None,
                    exprs: block_exprs(&f.block),
                    signature_surface: fn_signature_types(&f.sig),
                });
            }
            syn::Item::Struct(s) => {
                out.vertices.push(v(
                    format!("{module_path}::{}", s.ident),
                    Kind::Struct,
                    is_pub(&s.vis),
                ));
            }
            syn::Item::Enum(e) => {
                out.vertices.push(v(
                    format!("{module_path}::{}", e.ident),
                    Kind::Enum,
                    is_pub(&e.vis),
                ));
            }
            syn::Item::Trait(t) => {
                out.vertices.push(v(
                    format!("{module_path}::{}", t.ident),
                    Kind::Trait,
                    is_pub(&t.vis),
                ));
                // 트레이트 기본 메서드는 트레이트 소유 메서드 정점이다.
                for ti in &t.items {
                    if let syn::TraitItem::Fn(m) = ti {
                        let mid = format!("{module_path}::{}::{}", t.ident, m.sig.ident);
                        out.vertices
                            .push(v(mid.clone(), Kind::Method, is_pub(&t.vis)));
                        if let Some(default) = &m.default {
                            out.bodies.push(BodyItem {
                                id: mid,
                                module: module_path.to_string(),
                                self_ty: None,
                                exprs: block_exprs(default),
                                signature_surface: fn_signature_types(&m.sig),
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
                ));
            }
            syn::Item::Type(t) => {
                out.vertices.push(v(
                    format!("{module_path}::{}", t.ident),
                    Kind::TypeAlias,
                    is_pub(&t.vis),
                ));
            }
            syn::Item::Const(c) => {
                let id = format!("{module_path}::{}", c.ident);
                out.vertices
                    .push(v(id.clone(), Kind::Const, is_pub(&c.vis)));
                out.bodies.push(BodyItem {
                    id,
                    module: module_path.to_string(),
                    self_ty: None,
                    exprs: vec![(*c.expr).clone()],
                    signature_surface: vec![&c.ty],
                });
            }
            syn::Item::Static(s) => {
                let id = format!("{module_path}::{}", s.ident);
                out.vertices
                    .push(v(id.clone(), Kind::Static, is_pub(&s.vis)));
                out.bodies.push(BodyItem {
                    id,
                    module: module_path.to_string(),
                    self_ty: None,
                    exprs: vec![(*s.expr).clone()],
                    signature_surface: vec![&s.ty],
                });
            }
            syn::Item::Macro(m) => {
                if let Some(id) = &m.ident {
                    if m.mac.path.is_ident("macro_rules") {
                        out.vertices
                            .push(v(format!("{module_path}::{id}"), Kind::Macro, true));
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
                });
            }
            _ => {}
        }
    }
    out
}

/// impl 블록들을 정점·간선으로 변환한다(1패스 후속).
/// 반환: (정점, 간선, 2패스용 본문 목록).
pub fn impls<'a>(
    blocks: &'a [ImplBlock],
    krate: &str,
    tree: &ModTree,
    file: &Path,
    generated: bool,
    harvest: &mut Harvest,
) -> (Vec<Vertex>, Vec<Edge>, Vec<BodyItem<'a>>) {
    let mut vertices = Vec::new();
    let mut edges = Vec::new();
    let mut bodies = Vec::new();
    for b in blocks {
        // self 타입을 해석한다 — 모듈 로컬이면 정점이 있다.
        let Some(self_id) = tree.resolve(&b.items_module, &b.self_ty, &BTreeSet::new()) else {
            harvest.unresolved_paths += 1;
            continue;
        };
        if let Some(tp) = &b.trait_path {
            if let Some(trait_id) = tree.resolve(&b.items_module, tp, &BTreeSet::new()) {
                edges.push(Edge::new(
                    self_id.clone(),
                    trait_id.clone(),
                    EdgeKind::Implements,
                ));
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
            vertices.push(Vertex {
                id: mid.clone(),
                kind: Kind::Method,
                krate: krate.to_string(),
                module: b.items_module.clone(),
                position: Some(format!("{}:{}", file.display(), line_of(&m.sig.ident))),
                exported: matches!(m.vis, syn::Visibility::Public(_)),
                generated,
            });
            edges.push(Edge::new(self_id.clone(), mid.clone(), EdgeKind::Contains));
            bodies.push(BodyItem {
                id: mid,
                module: b.items_module.clone(),
                self_ty: Some(b.self_ty.clone()),
                exprs: block_exprs(&m.block),
                signature_surface: fn_signature_types(&m.sig),
            });
        }
    }
    (vertices, edges, bodies)
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

/// 본문을 방문해 call/references/매크로 간선을 만든다(2패스).
pub fn bodies(
    items: &[BodyItem],
    tree: &ModTree,
    dep_crates: &BTreeSet<String>,
    method_index: &BTreeMap<String, Vec<String>>,
    harvest: &mut Harvest,
) -> Vec<Edge> {
    let mut edges = Vec::new();
    for b in items {
        let mut vis = BodyVisitor {
            owner: &b.id,
            module: &b.module,
            self_ty: b.self_ty.as_deref(),
            tree,
            dep_crates,
            method_index,
            edges: Vec::new(),
            unresolved: 0,
            fanned: 0,
            ext_macros: 0,
        };
        for e in &b.exprs {
            vis.visit_expr(e);
        }
        // 시그니처 표면의 타입 참조 — signature 간선.
        for t in &b.signature_surface {
            let mut sv = TypeVisitor { paths: Vec::new() };
            sv.visit_type(t);
            for p in sv.paths {
                if let Some(id) = tree.resolve(&b.module, &p, dep_crates) {
                    if id != b.id {
                        edges.push(Edge::new(b.id.clone(), id, EdgeKind::Signature));
                    }
                }
            }
        }
        harvest.unresolved_paths += vis.unresolved;
        harvest.fanned_method_calls += vis.fanned;
        harvest.external_macros += vis.ext_macros;
        edges.extend(vis.edges);
    }
    edges
}

/// 본문 방문자 — 호출·경로 참조·매크로 호출을 간선으로 옮긴다.
struct BodyVisitor<'a> {
    owner: &'a str,
    module: &'a str,
    self_ty: Option<&'a [String]>,
    tree: &'a ModTree,
    dep_crates: &'a BTreeSet<String>,
    method_index: &'a BTreeMap<String, Vec<String>>,
    edges: Vec<Edge>,
    unresolved: usize,
    fanned: usize,
    ext_macros: usize,
}

impl BodyVisitor<'_> {
    fn push(&mut self, to: String, kind: EdgeKind) {
        if to != self.owner {
            self.edges.push(Edge::new(self.owner.to_string(), to, kind));
        }
    }

    /// 추정 간선 — 팬아웃의 "이 중 하나일 수 있다"는 확정이 아니다.
    fn push_maybe(&mut self, to: String, kind: EdgeKind) {
        if to != self.owner {
            self.edges
                .push(Edge::maybe(self.owner.to_string(), to, kind));
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
