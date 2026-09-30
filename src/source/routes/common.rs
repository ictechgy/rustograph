//! 프레임워크 추출기가 공유하는 모델과 syn 도우미.
//!
//! 추출기(axum·actix)는 선언을 [`Decl`]로, 서버 측 공백을 [`Gap`]으로 낸다. 문서
//! 조립(템플릿 렌더링·usr·위치·스코프 검증)은 상위 모듈이 한 곳에서 한다 — 두
//! 프레임워크가 계약 규칙을 따로 구현하면 갈라진다.

use super::template::{render, Seg};
use crate::source::schema::BridgeLocation;
use crate::source::Parts;
use proc_macro2::Span;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use syn::spanned::Spanned;

/// 소스 위치 — 파일과 syn 스팬. 줄·열은 문서를 조립할 때 계산한다.
#[derive(Clone, Debug)]
pub(super) struct Loc {
    pub file: PathBuf,
    pub span: Span,
}

/// 선언의 핸들러다.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Handler {
    /// 그래프 정점 ID로 해석된 함수·메서드.
    Usr(String),
    /// 클로저 핸들러 — usr는 감싸는 정점으로 근사하고 한계로 센다.
    Closure(Loc),
    /// 서비스·팩토리 호출 등 정점으로 해석하지 못한 핸들러.
    Unknown,
}

impl PartialEq for Loc {
    fn eq(&self, other: &Self) -> bool {
        self.file == other.file && self.span.byte_range() == other.span.byte_range()
    }
}
impl Eq for Loc {}

/// 끝 슬래시 판정이다.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Trailing {
    /// 끝 슬래시가 다르면 이 핸들러에 닿지 않는다.
    Strict,
    /// 끝 슬래시가 있든 없든 닿는다(경로 정규화 미들웨어).
    Optional,
    /// 알 수 없음 — `trailingSlash`를 싣지 않는다.
    Unknown,
}

/// 템플릿이 서버 루트부터 확정됐는지(`root`) 알 수 없는 접두사 뒤인지(`base`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Anchor {
    Root,
    Base,
}

/// 해석한 경로 — 세그먼트 또는 템플릿으로 확정하지 못한 원문.
#[derive(Clone, Debug)]
pub(super) enum DeclPath {
    Template {
        segs: Vec<Seg>,
        /// 끝 catch-all이 빈 나머지도 받으면 마지막 세그먼트를 비운 변형을 그 끝
        /// 슬래시 판정으로 함께 낸다. None이면 변형이 없다(받지 않거나, 경로
        /// 정규화가 그 요청을 다른 경로로 바꿔 닿지 않는다).
        empty_tail: Option<Trailing>,
    },
    Dynamic(String),
}

/// 추출기가 낸 선언 하나 — method마다 사실 하나가 된다.
#[derive(Clone, Debug)]
pub(super) struct Decl {
    /// 대문자 동사 또는 `ANY`.
    pub methods: Vec<String>,
    pub path: DeclPath,
    pub anchor: Anchor,
    pub trailing: Trailing,
    pub handler: Handler,
    pub loc: Loc,
    pub order: Option<(String, u64)>,
    pub narrowed: bool,
}

/// 한계 스코프 — 그 한계가 가릴 수 있는 요청의 상한이다.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct ScopeSpec {
    pub templates: Vec<String>,
    pub prefixes: Vec<String>,
    pub suffixes: Vec<String>,
    pub methods: Vec<String>,
}

/// 서버 측 공백 하나 — 접두사는 계약의 닫힌 목록에서 고른다.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct Gap {
    pub prefix: &'static str,
    pub text: String,
    pub scope: Option<ScopeSpec>,
}

/// 추출 결과.
#[derive(Default)]
pub(super) struct Output {
    pub decls: Vec<Decl>,
    pub gaps: Vec<Gap>,
}

impl Output {
    /// 스코프 없는 공백을 더한다.
    pub fn gap(&mut self, prefix: &'static str, text: String) {
        self.gaps.push(Gap {
            prefix,
            text,
            scope: None,
        });
    }

    /// 스코프 있는 공백을 더한다.
    pub fn scoped_gap(&mut self, prefix: &'static str, text: String, scope: ScopeSpec) {
        self.gaps.push(Gap {
            prefix,
            text,
            scope: Some(scope),
        });
    }
}

/// 크레이트의 모듈 수준 함수 하나.
pub(super) struct FnSite {
    pub id: String,
    pub module: String,
    pub file: PathBuf,
    pub item: &'static syn::ItemFn,
}

/// 추출기 공용 문맥 — 수확 산출물, 정점 집합, 파일 원문 캐시.
pub(super) struct Ctx<'a> {
    pub parts: &'a Parts,
    pub ids: BTreeSet<&'a str>,
    /// 문서 경로의 기준(정규화된 워크스페이스 루트).
    pub root: PathBuf,
    sources: RefCell<BTreeMap<PathBuf, Option<String>>>,
    owners: BTreeMap<PathBuf, Vec<(std::ops::Range<usize>, String)>>,
    consts: BTreeMap<String, String>,
}

impl<'a> Ctx<'a> {
    /// 수확 산출물로 문맥을 만든다. 정점 범위는 그래프에 실제로 있는 ID만 담는다.
    pub fn new(parts: &'a Parts, root: PathBuf) -> Ctx<'a> {
        let ids = parts.doc.vertex_ids();
        let mut owners: BTreeMap<PathBuf, Vec<(std::ops::Range<usize>, String)>> = BTreeMap::new();
        for sp in &parts.spans {
            if ids.contains(sp.id.as_str()) && !sp.range.is_empty() {
                owners
                    .entry(sp.file.clone())
                    .or_default()
                    .push((sp.range.clone(), sp.id.clone()));
            }
        }
        let mut ctx = Ctx {
            parts,
            ids,
            root,
            sources: RefCell::new(BTreeMap::new()),
            owners,
            consts: BTreeMap::new(),
        };
        ctx.consts = collect_str_consts(parts);
        ctx
    }

    /// 크레이트(루트 모듈 이름)의 테스트가 아닌 모듈 경로들이다.
    pub fn crate_modules(&self, krate: &str) -> Vec<String> {
        let prefix = format!("{krate}::");
        self.parts
            .tree
            .modules
            .keys()
            .filter(|m| (*m == krate || m.starts_with(&prefix)) && !self.module_is_test(m))
            .cloned()
            .collect()
    }

    /// `#[cfg(test)]` 모듈(또는 그 자손)인가 — 테스트 소스는 선언에서 뺀다.
    fn module_is_test(&self, module: &str) -> bool {
        let mut cur = Some(module.to_string());
        while let Some(m) = cur {
            let cfg = self
                .parts
                .tree
                .modules
                .get(&m)
                .and_then(|x| x.cfg.as_deref());
            if cfg.is_some_and(cfg_mentions_test) {
                return true;
            }
            cur = crate::modtree::parent_of(&m);
        }
        false
    }

    /// 크레이트의 테스트가 아닌 모듈 수준 함수 — ID는 수확과 같은 `모듈::이름`이다.
    pub fn crate_fns(&self, krate: &str) -> BTreeMap<String, FnSite> {
        let mut out = BTreeMap::new();
        for module in self.crate_modules(krate) {
            for (file, items) in self.parts.module_items(&module) {
                for item in items {
                    let syn::Item::Fn(f) = item else { continue };
                    if is_test_item(&f.attrs) {
                        continue;
                    }
                    let id = format!("{module}::{}", f.sig.ident);
                    out.insert(
                        id.clone(),
                        FnSite {
                            id,
                            module: module.clone(),
                            file: file.clone(),
                            item: f,
                        },
                    );
                }
            }
        }
        out
    }

    /// 모듈 기준으로 경로를 해석해 그래프 정점 ID를 돌려준다. 연관 함수
    /// (`Type::f`, 트레이트 impl의 `Type::<Trait>::f`)도 정점 집합에서 찾는다.
    pub fn resolve_vertex(&self, module: &str, segs: &[String]) -> Option<String> {
        let dep = crate::modtree::DepCrates::new();
        if let Some(id) = self.parts.tree.resolve(module, segs, &dep) {
            if self.ids.contains(id.as_str()) {
                return Some(id);
            }
        }
        let (name, head) = segs.split_last()?;
        if head.is_empty() {
            return None;
        }
        let ty = self.parts.tree.resolve(module, head, &dep)?;
        let inherent = format!("{ty}::{name}");
        if self.ids.contains(inherent.as_str()) {
            return Some(inherent);
        }
        let prefix = format!("{ty}::<");
        let suffix = format!(">::{name}");
        let mut hits = self
            .ids
            .iter()
            .filter(|id| id.starts_with(&prefix) && id.ends_with(&suffix));
        match (hits.next(), hits.next()) {
            (Some(one), None) => Some(one.to_string()),
            _ => None,
        }
    }

    /// 경로가 가리키는 `&str` 상수의 리터럴 값이다.
    pub fn const_str(&self, module: &str, segs: &[String]) -> Option<String> {
        let dep = crate::modtree::DepCrates::new();
        let id = self.parts.tree.resolve(module, segs, &dep)?;
        self.consts.get(&id).cloned()
    }

    /// 바이트 오프셋을 감싸는 가장 안쪽 정점 — 클로저 핸들러의 usr 근사다.
    pub fn owner_of(&self, loc: &Loc) -> Option<String> {
        let byte = loc.span.byte_range().start;
        self.owners
            .get(&loc.file)?
            .iter()
            .filter(|(r, _)| r.start <= byte && byte < r.end)
            .min_by(|(ra, ia), (rb, ib)| (ra.len(), ia).cmp(&(rb.len(), ib)))
            .map(|(_, id)| id.clone())
    }

    /// 계약의 위치(루트 기준 경로, 1 기반 줄, UTF-8 바이트 열)다.
    pub fn locate(&self, loc: &Loc) -> Option<BridgeLocation> {
        self.locate_with(loc, |c| c.len_utf8() as u32)
    }

    /// 계약의 위치 — 열은 GRAPH-EXCHANGE가 정한 UTF-8 바이트 오프셋 + 1이다.
    /// 호출 측 사실(`wrapper.location`)이 이 열을 쓴다.
    pub fn locate_utf8(&self, loc: &Loc) -> Option<BridgeLocation> {
        self.locate_with(loc, |c| c.len_utf8() as u32)
    }

    /// 열 단위(`unit`: 문자 하나의 길이)를 골라 위치를 계산한다.
    fn locate_with(&self, loc: &Loc, unit: fn(char) -> u32) -> Option<BridgeLocation> {
        let start = loc.span.start();
        if start.line == 0 {
            return None;
        }
        let file = loc.file.canonicalize().unwrap_or_else(|_| loc.file.clone());
        let rel = file.strip_prefix(&self.root).ok()?;
        let mut cache = self.sources.borrow_mut();
        let text = cache
            .entry(loc.file.clone())
            .or_insert_with(|| std::fs::read_to_string(&loc.file).ok())
            .as_ref()?;
        let line_text = text.lines().nth(start.line - 1)?;
        // proc-macro2의 열은 문자 수다 — 요청한 단위로 바꾼다.
        let column = line_text.chars().take(start.column).map(unit).sum::<u32>() + 1;
        Some(BridgeLocation {
            path: rel.to_string_lossy().replace('\\', "/"),
            line: start.line as u32,
            column,
            byte: loc.span.byte_range().start,
        })
    }

    /// 식의 원문(최대 2,048자) — dynamic 사실의 channel이다.
    pub fn source_text(&self, file: &Path, span: Span) -> String {
        let mut cache = self.sources.borrow_mut();
        let text = cache
            .entry(file.to_path_buf())
            .or_insert_with(|| std::fs::read_to_string(file).ok());
        let raw = text
            .as_deref()
            .and_then(|t| t.get(span.byte_range()))
            .unwrap_or("<expression>");
        take_utf16(raw, super::template::MAX_TEMPLATE_LENGTH)
    }
}

/// cfg 토큰이 `test`를 조건으로 쓰는가(`test`, `all(test, ..)` 등).
fn cfg_mentions_test(cfg: &str) -> bool {
    cfg.split(|c: char| !c.is_alphanumeric() && c != '_')
        .any(|w| w == "test")
}

/// 테스트 항목(`#[test]`·`#[tokio::test]`·`#[actix_web::test]`·`#[cfg(test)]`)인가.
pub(super) fn is_test_item(attrs: &[syn::Attribute]) -> bool {
    if crate::harvest::is_test_entry(attrs) {
        return true;
    }
    attrs.iter().any(|a| {
        a.path().is_ident("cfg")
            && matches!(&a.meta, syn::Meta::List(l) if cfg_mentions_test(&l.tokens.to_string()))
    })
}

/// 워크스페이스의 `const X: &str = "..";` 값 — 라우트 경로 상수 해석용.
fn collect_str_consts(parts: &Parts) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for module in parts.tree.modules.keys() {
        for (_, items) in parts.module_items(module) {
            for item in items {
                if let syn::Item::Const(c) = item {
                    if let Some((value, _)) = str_lit(&c.expr) {
                        out.insert(format!("{module}::{}", c.ident), value);
                    }
                }
            }
        }
    }
    out
}

/// 문자열 리터럴 식이면 값과 스팬.
pub(super) fn str_lit(expr: &syn::Expr) -> Option<(String, Span)> {
    match expr {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(s),
            ..
        }) => Some((s.value(), s.span())),
        syn::Expr::Paren(p) => str_lit(&p.expr),
        syn::Expr::Group(g) => str_lit(&g.expr),
        _ => None,
    }
}

/// 경로 인자 — 리터럴(또는 `&str` 상수)이면 값, 아니면 원문 스팬.
#[derive(Clone, Debug)]
pub(super) enum PathArg {
    Lit(String, Loc),
    Dyn(Loc),
}

impl PathArg {
    /// 인자의 위치.
    pub fn loc(&self) -> &Loc {
        match self {
            PathArg::Lit(_, l) | PathArg::Dyn(l) => l,
        }
    }
}

/// 경로 식을 읽는다 — 리터럴, 크레이트의 `&str` 상수 경로, 그 밖은 dynamic.
pub(super) fn path_arg(ctx: &Ctx, module: &str, file: &Path, expr: &syn::Expr) -> PathArg {
    if let Some((value, span)) = str_lit(expr) {
        return PathArg::Lit(
            value,
            Loc {
                file: file.to_path_buf(),
                span,
            },
        );
    }
    let loc = Loc {
        file: file.to_path_buf(),
        span: expr.span(),
    };
    let inner = match expr {
        syn::Expr::Reference(r) => &*r.expr,
        other => other,
    };
    if let syn::Expr::Path(p) = inner {
        let segs = crate::harvest::path_segments(&p.path);
        if let Some(value) = ctx.const_str(module, &segs) {
            return PathArg::Lit(value, loc);
        }
    }
    PathArg::Dyn(loc)
}

/// 이름 해석 전에 `use`로 들여온 이름을 원래 경로로 펼친다.
///
/// 모듈 트리의 import 표는 워크스페이스 안만 해석하므로 외부 크레이트(axum·
/// actix_web) 경로는 모듈의 `use` 선언을 직접 읽는다. 글롭은 접두사로 남긴다.
#[derive(Default, Debug)]
pub(super) struct Imports {
    names: BTreeMap<String, Vec<String>>,
    globs: Vec<Vec<String>>,
}

impl Imports {
    /// 모듈 아이템(여러 파일일 수 있다)의 `use` 선언을 모은다.
    pub fn of(groups: &[(PathBuf, &'static [syn::Item])]) -> Imports {
        let mut out = Imports::default();
        for (_, items) in groups {
            for item in *items {
                if let syn::Item::Use(u) = item {
                    out.collect(&u.tree, &mut Vec::new());
                }
            }
        }
        out
    }

    fn collect(&mut self, tree: &syn::UseTree, prefix: &mut Vec<String>) {
        match tree {
            syn::UseTree::Path(p) => {
                prefix.push(p.ident.to_string());
                self.collect(&p.tree, prefix);
                prefix.pop();
            }
            syn::UseTree::Name(n) => {
                let name = n.ident.to_string();
                let mut full = prefix.clone();
                if name != "self" {
                    full.push(name.clone());
                }
                let local = if name == "self" {
                    prefix.last().cloned().unwrap_or_default()
                } else {
                    name
                };
                self.names.insert(local, full);
            }
            syn::UseTree::Rename(r) => {
                let mut full = prefix.clone();
                if r.ident != "self" {
                    full.push(r.ident.to_string());
                }
                self.names.insert(r.rename.to_string(), full);
            }
            syn::UseTree::Glob(_) => self.globs.push(prefix.clone()),
            syn::UseTree::Group(g) => {
                for t in &g.items {
                    self.collect(t, prefix);
                }
            }
        }
    }

    /// 경로의 첫 세그먼트를 `use` 원래 경로로 펼친다. `::a::b` 선행 콜론은 없다고 본다.
    pub fn expand(&self, segs: &[String]) -> Vec<String> {
        let Some((first, rest)) = segs.split_first() else {
            return Vec::new();
        };
        match self.names.get(first) {
            Some(full) => full.iter().chain(rest.iter()).cloned().collect(),
            None => segs.to_vec(),
        }
    }

    /// 단일 이름이 `krate`로 시작하는 글롭에서 왔을 수 있는가.
    pub fn globbed_from(&self, krate: &str) -> bool {
        self.globs
            .iter()
            .any(|g| g.first().is_some_and(|f| f == krate))
    }
}

/// 크레이트 루트 모듈 이름 `krate`의 외부 API 경로인가 — 펼친 경로가 그 크레이트로
/// 시작하고 `tail`로 끝나거나, 한 세그먼트 이름이 그 크레이트 글롭에서 왔다.
pub(super) fn is_api(imports: &Imports, segs: &[String], krate: &str, tail: &[&str]) -> bool {
    let full = imports.expand(segs);
    let ends = full.len() >= tail.len()
        && full[full.len() - tail.len()..]
            .iter()
            .zip(tail)
            .all(|(a, b)| a == b);
    if !ends {
        return false;
    }
    if full.first().is_some_and(|f| f == krate) {
        return true;
    }
    // 글롭으로 들여온 이름(`use axum::routing::*;` 뒤 `get`)은 펼쳐지지 않는다.
    full.len() == tail.len() && imports.globbed_from(krate)
}

/// 템플릿을 스코프 원소로 쓸 수 있게 렌더링한다(접두사는 끝 `/`를 뗀다).
pub(super) fn prefix_template(segs: &[Seg]) -> Option<String> {
    if segs.iter().any(|s| matches!(s, Seg::CatchAll)) {
        return None;
    }
    let t = render(segs);
    if t == "/" {
        return Some(t);
    }
    let trimmed = t.trim_end_matches('/');
    Some(if trimmed.is_empty() {
        "/".to_string()
    } else {
        trimmed.to_string()
    })
}

/// `x`·`mut x`·`x: T` 패턴의 이름.
pub(super) fn pat_ident(p: &syn::Pat) -> Option<String> {
    match p {
        syn::Pat::Ident(i) if i.subpat.is_none() => Some(i.ident.to_string()),
        syn::Pat::Type(t) => pat_ident(&t.pat),
        _ => None,
    }
}

/// 해석 전 핸들러 식.
#[derive(Clone, Debug)]
pub(super) enum HandlerRef {
    Path { module: String, segs: Vec<String> },
    Closure(Loc),
    Unknown,
}

impl HandlerRef {
    /// 정점 ID로 해석한다 — 못 하면 Unknown(usr 없음).
    pub fn resolve(&self, ctx: &Ctx) -> Handler {
        match self {
            HandlerRef::Path { module, segs } => match ctx.resolve_vertex(module, segs) {
                Some(id) => Handler::Usr(id),
                None => Handler::Unknown,
            },
            HandlerRef::Closure(l) => Handler::Closure(l.clone()),
            HandlerRef::Unknown => Handler::Unknown,
        }
    }
}

/// 핸들러 식 — 경로, `h.layer(..)`·`h.with_state(..)`의 수신자, 클로저.
pub(super) fn handler_ref(module: &str, file: &Path, e: &syn::Expr) -> HandlerRef {
    match e {
        syn::Expr::Path(p) => HandlerRef::Path {
            module: module.to_string(),
            segs: crate::harvest::path_segments(&p.path),
        },
        syn::Expr::MethodCall(m) => handler_ref(module, file, &m.receiver),
        syn::Expr::Paren(p) => handler_ref(module, file, &p.expr),
        syn::Expr::Reference(r) => handler_ref(module, file, &r.expr),
        syn::Expr::Closure(c) => HandlerRef::Closure(Loc {
            file: file.to_path_buf(),
            span: c.span(),
        }),
        _ => HandlerRef::Unknown,
    }
}

/// 결합된 경로 — 리터럴, 알 수 없는 앞부분 뒤의 리터럴(base), 템플릿 불가.
#[derive(Clone, Debug)]
pub(super) enum JPath {
    Lit(String),
    Base(String),
    Dyn(Loc),
}

impl JPath {
    /// 경로 인자에서 만든다.
    pub fn of(p: &PathArg) -> JPath {
        match p {
            PathArg::Lit(s, _) => JPath::Lit(s.clone()),
            PathArg::Dyn(l) => JPath::Dyn(l.clone()),
        }
    }

    /// 접두사(self)에 안쪽 경로를 `glue`로 잇는다. 알 수 없는 조각이 맨 앞이면
    /// 나머지를 base로, 가운데면 템플릿을 포기한다(dynamic, 위치는 `loc`).
    pub fn join(&self, inner: &JPath, glue: impl Fn(&str, &str) -> String, loc: &Loc) -> JPath {
        match (self, inner) {
            (JPath::Lit(p), JPath::Lit(q)) => JPath::Lit(glue(p, q)),
            (JPath::Base(p), JPath::Lit(q)) => JPath::Base(glue(p, q)),
            (JPath::Dyn(_), JPath::Lit(q)) => JPath::Base(q.clone()),
            (JPath::Dyn(l), _) | (_, JPath::Dyn(l)) => JPath::Dyn(l.clone()),
            (JPath::Lit(_) | JPath::Base(_), JPath::Base(_)) => JPath::Dyn(loc.clone()),
        }
    }
}

/// 추출기가 평가하지 않는 자리(impl·trait 메서드 본문)의 라우터 생성 호출 위치.
///
/// 추출기는 모듈 수준 함수만 평가한다. 메서드 안에서 `Router::new()`·`App::new()`로
/// 라우터를 만들면 그 선언을 놓치므로, 조용히 0건이 되지 않게 위치를 센다.
pub(super) fn unevaluated_constructors(
    ctx: &Ctx,
    krate: &str,
    api_crate: &str,
    tail: &[&str],
) -> Vec<Loc> {
    struct Finder<'b> {
        imports: &'b Imports,
        api_crate: &'b str,
        tail: &'b [&'b str],
        file: &'b Path,
        out: Vec<Loc>,
    }
    impl syn::visit::Visit<'_> for Finder<'_> {
        fn visit_expr_call(&mut self, c: &syn::ExprCall) {
            if let syn::Expr::Path(p) = &*c.func {
                let segs = crate::harvest::path_segments(&p.path);
                if is_api(self.imports, &segs, self.api_crate, self.tail) {
                    self.out.push(Loc {
                        file: self.file.to_path_buf(),
                        span: c.span(),
                    });
                }
            }
            syn::visit::visit_expr_call(self, c);
        }
    }
    let mut out = Vec::new();
    for module in ctx.crate_modules(krate) {
        let groups = ctx.parts.module_items(&module);
        let imports = Imports::of(&groups);
        for (file, items) in &groups {
            let mut f = Finder {
                imports: &imports,
                api_crate,
                tail,
                file,
                out: Vec::new(),
            };
            for item in *items {
                match item {
                    syn::Item::Impl(i) if !is_test_item(&i.attrs) => {
                        syn::visit::Visit::visit_item_impl(&mut f, i)
                    }
                    syn::Item::Trait(t) if !is_test_item(&t.attrs) => {
                        syn::visit::Visit::visit_item_trait(&mut f, t)
                    }
                    _ => {}
                }
            }
            out.extend(f.out);
        }
    }
    out
}

/// order group 문자열 — 계약의 256자 상한을 넘으면 앞부분과 지문으로 줄인다.
pub(super) fn order_group(prefix: &str, id: &str) -> String {
    let full = format!("{prefix}{id}");
    if full.encode_utf16().count() <= 256 {
        return full;
    }
    let mut h = 0xcbf29ce484222325u64;
    for b in full.bytes() {
        h = (h ^ u64::from(b)).wrapping_mul(0x100000001b3);
    }
    // `#` + 16자리 지문을 뺀 나머지를 UTF-16 단위로 채운다(BMP 밖 글자는 2단위).
    let head = take_utf16(&full, 256 - 17);
    format!("{}#{h:016x}", head.trim_end())
}

/// 앞에서부터 UTF-16 코드 단위 `budget` 이하가 되도록 자른다 — 소비자의 길이 상한은
/// UTF-16 기준이라 문자 수로 자르면 BMP 밖 글자에서 넘친다.
pub(super) fn take_utf16(text: &str, budget: usize) -> String {
    let mut used = 0;
    text.chars()
        .take_while(|c| {
            used += c.len_utf16();
            used <= budget
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_groups_and_texts_fit_utf16_budgets() {
        let id = "\u{10300}".repeat(300);
        let g = order_group("actix:", &id);
        assert!(
            g.encode_utf16().count() <= 256,
            "{}",
            g.encode_utf16().count()
        );
        assert_eq!(order_group("actix:", "a::b"), "actix:a::b");
        assert_eq!(take_utf16("a\u{10300}b", 2), "a");
    }
}
