//! axum 0.7·0.8 라우터 선언 추출.
//!
//! 규칙과 근거(axum·matchit 소스 줄 번호)는 docs/HTTP-ROUTES.md에 있다. 요약:
//!
//! - 경로 문법은 버전마다 다르다. 0.7(matchit 0.7)은 `:name`·`*name`, 0.8(matchit
//!   0.8)은 `{name}`·`{*name}`과 `{{`·`}}` 이스케이프다. 0.8은 `:`·`*`로 시작하는
//!   세그먼트를 기동 시 panic으로 거부한다.
//! - matchit은 정적 > 파라미터 > catch-all 우선순위와 되돌아가기로 고른다 →
//!   `dispatch: "specificity"`. 끝 슬래시는 엄격하다(`/a`와 `/a/`는 다른 경로).
//! - `nest`는 `path_for_nested_route` 규칙으로 문자열을 잇는다.
//! - 라우터 값은 한 함수 안의 `let`·재대입, 크레이트 함수 호출(반환식)을 따라
//!   정적으로 평가한다. 조건·반복 등록, 외부 함수가 만든 라우터는 한계다.

use super::common::{
    handler_ref, is_api, pat_ident, path_arg, prefix_template, unevaluated_constructors, Anchor,
    Ctx, Decl, DeclPath, FnSite,
    Handler, HandlerRef, Imports, JPath, Loc, Output, PathArg, ScopeSpec, Trailing,
};
use super::template::{normalize_uri_path, render, template_problem, Seg};
use crate::harvest::path_segments;
use std::collections::{BTreeMap, BTreeSet};
use syn::spanned::Spanned;
use syn::visit::Visit;

/// 경로 문법 버전이다.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Version {
    /// matchit 0.7 문법(`:name`·`*name`) — axum 0.7.x(0.6.x도 같은 문법).
    V07,
    /// matchit 0.8 문법(`{name}`·`{*name}`) — axum 0.8.x.
    V08,
}

/// 메서드 라우터 생성자·체인 메서드 이름과 동사다.
const METHOD_FNS: &[(&str, &str)] = &[
    ("get", "GET"),
    ("post", "POST"),
    ("put", "PUT"),
    ("delete", "DELETE"),
    ("patch", "PATCH"),
    ("head", "HEAD"),
    ("options", "OPTIONS"),
    ("trace", "TRACE"),
    ("connect", "CONNECT"),
];

/// 한 크레이트의 axum 라우트를 추출한다.
pub(super) fn extract(ctx: &Ctx, krate: &str, version: Version, out: &mut Output) {
    let fns = ctx.crate_fns(krate);
    let mut ev = Eval {
        ctx,
        fns: &fns,
        memo: BTreeMap::new(),
        stack: Vec::new(),
        consumed: BTreeSet::new(),
        imports: BTreeMap::new(),
        layer_normalize: 0,
    };
    // 1) 서빙되는 라우터(루트) — `axum::serve(_, X)`·`X.into_make_service()`.
    let mut roots: Vec<RouterVal> = Vec::new();
    for site in fns.values() {
        for expr in served_exprs(&ev, site) {
            let mut env = Env::default();
            ev.fill_env(site, &site.item.block, &mut env);
            if let Some(Val::Router(r)) = ev.eval(site, &env, &expr) {
                roots.push(r);
            }
        }
    }
    // 2) 라우터를 만드는 함수 중 루트에서 닿지 않은 것 — 어디에 붙는지 모른다.
    let builders: Vec<String> = fns
        .values()
        .filter(|s| builds_router(&ev, s))
        .map(|s| s.id.clone())
        .collect();
    let mut flat = Vec::new();
    for r in &roots {
        ev.flatten(r, &mut flat);
    }
    let rooted = flat.len();
    // 먼저 전부 평가해 다른 라우터에 쓰인 함수를 표시한 뒤 남은 것만 낸다 — 한 번에
    // 하면 이름 순서에 따라 안쪽 라우터가 따로 한 번, 바깥 라우터 안에서 또 한 번 나온다.
    for id in &builders {
        let _ = ev.eval_fn(id);
    }
    for id in &builders {
        if ev.consumed.contains(id) {
            continue;
        }
        if let Some(Val::Router(r)) = ev.eval_fn(id) {
            if !r.entries.is_empty() {
                let mut own = Vec::new();
                ev.flatten(&r, &mut own);
                for f in &mut own {
                    f.path = rebase(std::mem::replace(&mut f.path, JPath::Lit(String::new())));
                }
                flat.extend(own);
            }
        }
    }
    for loc in unevaluated_constructors(ctx, krate, "axum", &["Router", "new"]) {
        let at = ctx
            .locate(&loc)
            .map(|l| format!("{}:{}", l.path, l.line))
            .unwrap_or_default();
        out.gap(
            "route-coverage:",
            format!("a Router built inside a method at {at} is not evaluated; its routes are not extracted"),
        );
    }
    let normalize = normalize_effect(&ev, &fns);
    let mut emit = Emit {
        ctx,
        version,
        normalize,
        out,
        bases: Vec::new(),
    };
    for (i, f) in flat.iter().enumerate() {
        emit.flat(f, i >= rooted);
    }
    emit.finish_bases();
}

/// 경로 정규화 미들웨어의 효과다(Router 바깥을 감쌀 때만 라우팅 전에 동작한다).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Normalize {
    None,
    Trim,
    Append,
    Unknown,
}

/// 라우터 값 — 등록 순서대로의 항목.
#[derive(Clone, Debug, Default)]
struct RouterVal {
    entries: Vec<Entry>,
}

/// 라우터 항목 하나.
#[derive(Clone, Debug)]
enum Entry {
    Route {
        path: PathArg,
        method: MethodVal,
    },
    Nest {
        prefix: PathArg,
        inner: RouterVal,
    },
    NestService {
        prefix: PathArg,
    },
    RouteService {
        path: PathArg,
    },
    Merge(RouterVal),
    Fallback {
        loc: Loc,
    },
    /// 평가하지 못한 라우터 조각(외부 함수·조건 등) — 그 자리의 접두사 아래 한계다.
    Unknown {
        loc: Loc,
        what: String,
    },
}

/// 메서드 라우터 값.
#[derive(Clone, Debug, Default)]
struct MethodVal {
    routes: Vec<MRoute>,
    /// 평가하지 못한 메서드 라우터 식이 섞였다.
    unknown: bool,
}

/// 메서드 라우터 항목 — `methods`가 None이면 모든 method(ANY)다.
#[derive(Clone, Debug)]
struct MRoute {
    methods: Option<Vec<String>>,
    handler: HandlerRef,
}

/// 평가 값.
#[derive(Clone, Debug)]
enum Val {
    Router(RouterVal),
    Method(MethodVal),
}

/// 함수 본문의 지역 값(이름 → 마지막으로 대입된 값). 흐름 비민감 근사다.
#[derive(Default)]
struct Env {
    vars: BTreeMap<String, Val>,
}

/// 평가기 — 함수 반환 라우터를 기억하고 순환 호출을 막는다.
struct Eval<'a> {
    ctx: &'a Ctx<'a>,
    fns: &'a BTreeMap<String, FnSite>,
    memo: BTreeMap<String, Option<Val>>,
    stack: Vec<String>,
    /// 다른 라우터·서빙 식에 쓰인 라우터 함수.
    consumed: BTreeSet<String>,
    imports: BTreeMap<String, Imports>,
    /// `Router::layer` 인자로 쓰인 경로 정규화 레이어 수(라우팅 뒤라 효과 없음).
    layer_normalize: usize,
}

impl Eval<'_> {
    /// 모듈의 `use` 표(캐시).
    fn imports(&mut self, module: &str) -> &Imports {
        if !self.imports.contains_key(module) {
            let groups = self.ctx.parts.module_items(module);
            self.imports
                .insert(module.to_string(), Imports::of(&groups));
        }
        &self.imports[module]
    }

    /// 경로가 axum API(`tail`로 끝나는 axum 경로)인가.
    fn is_axum(&mut self, module: &str, segs: &[String], tail: &[&str]) -> bool {
        is_api(self.imports(module), segs, "axum", tail)
    }

    /// 크레이트 함수의 반환 값을 평가한다(기억·순환 방지).
    fn eval_fn(&mut self, id: &str) -> Option<Val> {
        if let Some(v) = self.memo.get(id) {
            return v.clone();
        }
        if self.stack.iter().any(|s| s == id) {
            return None;
        }
        let site = self.fns.get(id)?;
        self.stack.push(id.to_string());
        let mut env = Env::default();
        let result = self.eval_block(site, &mut env, &site.item.block);
        self.stack.pop();
        self.memo.insert(id.to_string(), result.clone());
        result
    }

    /// 블록을 순서대로 평가해 지역을 채우고 꼬리식(또는 `return`) 값을 돌려준다.
    fn eval_block(&mut self, site: &FnSite, env: &mut Env, block: &syn::Block) -> Option<Val> {
        let mut result = None;
        for stmt in &block.stmts {
            match stmt {
                syn::Stmt::Local(l) => self.bind_local(site, env, l),
                syn::Stmt::Expr(syn::Expr::Assign(a), _) => self.assign(site, env, a),
                syn::Stmt::Expr(syn::Expr::Return(r), _) => {
                    if let Some(e) = &r.expr {
                        return self.eval(site, env, e);
                    }
                }
                syn::Stmt::Expr(e, None) => result = self.eval(site, env, e),
                _ => {}
            }
        }
        result
    }

    /// 함수 본문 전체의 지역을 채운다(서빙 식 평가용 — 꼬리식은 버린다).
    fn fill_env(&mut self, site: &FnSite, block: &syn::Block, env: &mut Env) {
        for stmt in &block.stmts {
            match stmt {
                syn::Stmt::Local(l) => self.bind_local(site, env, l),
                syn::Stmt::Expr(syn::Expr::Assign(a), _) => self.assign(site, env, a),
                _ => {}
            }
        }
    }

    /// `let x = <라우터 식>;`을 지역에 묶는다.
    fn bind_local(&mut self, site: &FnSite, env: &mut Env, l: &syn::Local) {
        let (Some(name), Some(init)) = (pat_ident(&l.pat), &l.init) else {
            return;
        };
        match self.eval(site, env, &init.expr) {
            Some(v) => {
                env.vars.insert(name, v);
            }
            None => {
                env.vars.remove(&name);
            }
        }
    }

    /// `x = <라우터 식>;` 재대입.
    fn assign(&mut self, site: &FnSite, env: &mut Env, a: &syn::ExprAssign) {
        let syn::Expr::Path(p) = &*a.left else { return };
        let Some(name) = p.path.get_ident().map(|i| i.to_string()) else {
            return;
        };
        match self.eval(site, env, &a.right) {
            Some(v) => {
                env.vars.insert(name, v);
            }
            None => {
                env.vars.remove(&name);
            }
        }
    }

    /// 식을 라우터·메서드 라우터 값으로 평가한다. 모르면 None.
    fn eval(&mut self, site: &FnSite, env: &Env, e: &syn::Expr) -> Option<Val> {
        match e {
            syn::Expr::Paren(p) => self.eval(site, env, &p.expr),
            syn::Expr::Group(g) => self.eval(site, env, &g.expr),
            syn::Expr::Reference(r) => self.eval(site, env, &r.expr),
            syn::Expr::Await(a) => self.eval(site, env, &a.base),
            syn::Expr::Try(t) => self.eval(site, env, &t.expr),
            syn::Expr::Block(b) => {
                let mut inner = Env {
                    vars: env.vars.clone(),
                };
                self.eval_block(site, &mut inner, &b.block)
            }
            syn::Expr::Path(p) => {
                let name = p.path.get_ident()?.to_string();
                env.vars.get(&name).cloned()
            }
            syn::Expr::Call(c) => self.eval_call(site, env, c),
            syn::Expr::MethodCall(m) => self.eval_method(site, env, m),
            _ => None,
        }
    }

    /// 함수 호출 — `Router::new()`, 메서드 라우터 생성자, 크레이트 함수.
    fn eval_call(&mut self, site: &FnSite, env: &Env, c: &syn::ExprCall) -> Option<Val> {
        let syn::Expr::Path(p) = &*c.func else {
            return None;
        };
        let segs = path_segments(&p.path);
        let module = site.module.clone();
        // 크레이트 함수가 먼저다 — 사용자가 `get`이라는 함수를 정의했을 수 있다.
        if let Some(id) = self.ctx.resolve_vertex(&module, &segs) {
            if self.fns.contains_key(&id) {
                // 라우터를 돌려주지 않는 크레이트 함수면 아래 axum API 판정으로 넘긴다.
                if let Some(v) = self.eval_fn(&id) {
                    self.consumed.insert(id);
                    return Some(v);
                }
            }
        }
        if self.is_axum(&module, &segs, &["Router", "new"]) {
            return Some(Val::Router(RouterVal::default()));
        }
        if self.is_axum(&module, &segs, &["MethodRouter", "new"]) {
            return Some(Val::Method(MethodVal::default()));
        }
        let name = segs.last()?.clone();
        if !self.is_axum(&module, &segs, &[name.as_str()]) {
            return None;
        }
        let mut mv = MethodVal::default();
        self.apply_method(
            site,
            env,
            &mut mv,
            &name,
            &c.args.iter().collect::<Vec<_>>(),
        )
        .then_some(Val::Method(mv))
    }

    /// 체인 메서드 — 수신자 값에 등록을 더한다.
    fn eval_method(&mut self, site: &FnSite, env: &Env, m: &syn::ExprMethodCall) -> Option<Val> {
        let recv = self.eval(site, env, &m.receiver)?;
        let name = m.method.to_string();
        let args: Vec<&syn::Expr> = m.args.iter().collect();
        match recv {
            Val::Router(mut r) => {
                self.apply_router(site, env, &mut r, &name, &args);
                Some(Val::Router(r))
            }
            Val::Method(mut mv) => {
                if !self.apply_method(site, env, &mut mv, &name, &args) {
                    // 레이어·상태 등 등록을 바꾸지 않는 메서드는 투명하다.
                }
                Some(Val::Method(mv))
            }
        }
    }

    /// Router 체인 메서드 하나를 적용한다. 모르는 메서드(`layer`·`with_state` 등)는 투명.
    fn apply_router(
        &mut self,
        site: &FnSite,
        env: &Env,
        r: &mut RouterVal,
        name: &str,
        args: &[&syn::Expr],
    ) {
        let file = site.file.clone();
        let loc_of = |e: &syn::Expr| Loc {
            file: file.clone(),
            span: e.span(),
        };
        match (name, args) {
            ("route", [p, mr]) => {
                let path = path_arg(self.ctx, &site.module, &site.file, p);
                let method = match self.eval(site, env, mr) {
                    Some(Val::Method(mv)) => mv,
                    _ => MethodVal {
                        routes: Vec::new(),
                        unknown: true,
                    },
                };
                r.entries.push(Entry::Route { path, method });
            }
            ("nest", [p, inner]) => {
                let prefix = path_arg(self.ctx, &site.module, &site.file, p);
                match self.eval(site, env, inner) {
                    Some(Val::Router(ir)) => r.entries.push(Entry::Nest { prefix, inner: ir }),
                    _ => r.entries.push(Entry::Nest {
                        prefix,
                        inner: RouterVal {
                            entries: vec![Entry::Unknown {
                                loc: loc_of(inner),
                                what: "a nested router the analyzer could not evaluate".to_string(),
                            }],
                        },
                    }),
                }
            }
            ("merge", [inner]) => match self.eval(site, env, inner) {
                Some(Val::Router(ir)) => r.entries.push(Entry::Merge(ir)),
                _ => r.entries.push(Entry::Unknown {
                    loc: loc_of(inner),
                    what: "a merged router the analyzer could not evaluate".to_string(),
                }),
            },
            ("nest_service", [p, _]) => r.entries.push(Entry::NestService {
                prefix: path_arg(self.ctx, &site.module, &site.file, p),
            }),
            ("route_service", [p, _]) => r.entries.push(Entry::RouteService {
                path: path_arg(self.ctx, &site.module, &site.file, p),
            }),
            ("fallback" | "fallback_service", [h]) => {
                r.entries.push(Entry::Fallback { loc: loc_of(h) })
            }
            ("layer" | "route_layer", [layer]) if mentions(layer, "NormalizePathLayer") => {
                self.layer_normalize += 1;
            }
            _ => {}
        }
    }

    /// 메서드 라우터 생성자·체인 메서드를 적용한다. 등록을 바꿨으면 true.
    fn apply_method(
        &mut self,
        site: &FnSite,
        env: &Env,
        mv: &mut MethodVal,
        name: &str,
        args: &[&syn::Expr],
    ) -> bool {
        let verb = |n: &str| {
            METHOD_FNS
                .iter()
                .find(|(f, _)| *f == n)
                .map(|(_, v)| v.to_string())
        };
        let service = name.strip_suffix("_service");
        match (name, args) {
            ("any", [h]) | ("fallback", [h]) => mv.routes.push(MRoute {
                methods: None,
                handler: handler_ref(&site.module, &site.file, h),
            }),
            ("on", [filter, h]) => mv.routes.push(MRoute {
                methods: method_filter(filter),
                handler: handler_ref(&site.module, &site.file, h),
            }),
            ("on_service", [filter, _]) => mv.routes.push(MRoute {
                methods: method_filter(filter),
                handler: HandlerRef::Unknown,
            }),
            ("any_service" | "fallback_service", [_]) => mv.routes.push(MRoute {
                methods: None,
                handler: HandlerRef::Unknown,
            }),
            ("merge", [other]) => match self.eval(site, env, other) {
                Some(Val::Method(o)) => {
                    mv.routes.extend(o.routes);
                    mv.unknown |= o.unknown;
                }
                _ => mv.unknown = true,
            },
            (n, [h]) if verb(n).is_some() => mv.routes.push(MRoute {
                methods: verb(n).map(|v| vec![v]),
                handler: handler_ref(&site.module, &site.file, h),
            }),
            (_, [_]) if service.and_then(verb).is_some() => mv.routes.push(MRoute {
                methods: service.and_then(verb).map(|v| vec![v]),
                handler: HandlerRef::Unknown,
            }),
            _ => return false,
        }
        true
    }

    /// 라우터 값을 (결합된 경로, 항목) 목록으로 편다. nest 접두사는 axum과 같은
    /// 규칙(`path_for_nested_route`)으로 안쪽 경로에 잇는다.
    fn flatten(&mut self, r: &RouterVal, out: &mut Vec<Flat>) {
        for entry in &r.entries {
            match entry {
                Entry::Route { path, method } => out.push(Flat {
                    path: JPath::of(path),
                    kind: FlatKind::Route(method.clone(), path.loc().clone()),
                }),
                Entry::RouteService { path } => out.push(Flat {
                    path: JPath::of(path),
                    kind: FlatKind::RouteService(path.loc().clone()),
                }),
                Entry::NestService { prefix } => out.push(Flat {
                    path: JPath::of(prefix),
                    kind: FlatKind::NestService(prefix.loc().clone()),
                }),
                Entry::Fallback { loc } => out.push(Flat {
                    path: JPath::Lit("/".to_string()),
                    kind: FlatKind::Fallback(loc.clone(), false),
                }),
                Entry::Unknown { loc, what } => out.push(Flat {
                    path: JPath::Lit("/".to_string()),
                    kind: FlatKind::Unknown(loc.clone(), what.clone()),
                }),
                Entry::Merge(inner) => self.flatten(inner, out),
                Entry::Nest { prefix, inner } => {
                    let mut own = Vec::new();
                    self.flatten(inner, &mut own);
                    let p = JPath::of(prefix);
                    for mut f in own {
                        f.kind = f.kind.nested();
                        let prefix_item = f.kind.is_prefix_item();
                        let glue = |a: &str, b: &str| {
                            if prefix_item {
                                nest_prefix_join(a, b)
                            } else {
                                path_for_nested_route(a, b)
                            }
                        };
                        f.path = p.join(&f.path, glue, f.kind.loc());
                        out.push(f);
                    }
                }
            }
        }
    }
}

/// 식이 이름 `name`을 담는가(경로 세그먼트 기준) — 레이어 판별용.
fn mentions(e: &syn::Expr, name: &str) -> bool {
    struct Finder<'n>(&'n str, bool);
    impl Visit<'_> for Finder<'_> {
        fn visit_path(&mut self, p: &syn::Path) {
            if p.segments.iter().any(|s| s.ident == self.0) {
                self.1 = true;
            }
            syn::visit::visit_path(self, p);
        }
    }
    let mut f = Finder(name, false);
    f.visit_expr(e);
    f.1
}

/// `MethodFilter::GET.or(MethodFilter::POST)` 같은 필터의 동사들. 모르면 None(ANY 근사).
fn method_filter(e: &syn::Expr) -> Option<Vec<String>> {
    match e {
        syn::Expr::Path(p) => {
            let last = p.path.segments.last()?.ident.to_string();
            METHOD_FNS
                .iter()
                .any(|(_, v)| *v == last)
                .then(|| vec![last])
        }
        syn::Expr::MethodCall(m) if m.method == "or" && m.args.len() == 1 => {
            let mut a = method_filter(&m.receiver)?;
            a.extend(method_filter(&m.args[0])?);
            a.sort();
            a.dedup();
            Some(a)
        }
        syn::Expr::Paren(p) => method_filter(&p.expr),
        _ => None,
    }
}

/// axum `path_for_nested_route`(0.7.9 path_router.rs:496, 0.8.9 path_router.rs:535).
fn path_for_nested_route(prefix: &str, path: &str) -> String {
    if prefix.ends_with('/') {
        format!("{prefix}{}", path.trim_start_matches('/'))
    } else if path == "/" {
        prefix.to_string()
    } else {
        format!("{prefix}{path}")
    }
}

/// 접두사 자리 항목(중첩 fallback·nest_service)의 접두사 결합 — 안쪽 접두사가
/// `/`면 바깥 접두사 그대로다.
fn nest_prefix_join(prefix: &str, inner: &str) -> String {
    if inner == "/" {
        prefix.to_string()
    } else {
        path_for_nested_route(prefix, inner)
    }
}

/// 루트에서 닿지 않은 라우터의 경로는 알 수 없는 접두사 뒤다.
fn rebase(p: JPath) -> JPath {
    match p {
        JPath::Lit(s) => JPath::Base(s),
        other => other,
    }
}

/// 편 항목 하나.
#[derive(Clone, Debug)]
struct Flat {
    path: JPath,
    kind: FlatKind,
}

/// 편 항목의 종류 — bool은 nest 아래(접두사 자리)인지다.
#[derive(Clone, Debug)]
enum FlatKind {
    Route(MethodVal, Loc),
    RouteService(Loc),
    NestService(Loc),
    Fallback(Loc, bool),
    Unknown(Loc, String),
}

impl FlatKind {
    /// nest 안으로 들어간 항목 표시.
    fn nested(self) -> FlatKind {
        match self {
            FlatKind::Fallback(l, _) => FlatKind::Fallback(l, true),
            other => other,
        }
    }

    /// 항목의 소스 위치.
    fn loc(&self) -> &Loc {
        match self {
            FlatKind::Route(_, l)
            | FlatKind::RouteService(l)
            | FlatKind::NestService(l)
            | FlatKind::Fallback(l, _)
            | FlatKind::Unknown(l, _) => l,
        }
    }

    /// 경로가 아니라 접두사를 뜻하는 항목인가.
    fn is_prefix_item(&self) -> bool {
        matches!(
            self,
            FlatKind::NestService(_) | FlatKind::Fallback(..) | FlatKind::Unknown(..)
        )
    }
}

/// 함수가 `Router::new()`를 부르는가(라우터를 만드는 함수 후보).
fn builds_router(ev: &Eval, site: &FnSite) -> bool {
    struct Finder<'a, 'b> {
        imports: &'b Imports,
        found: bool,
        _p: std::marker::PhantomData<&'a ()>,
    }
    impl Visit<'_> for Finder<'_, '_> {
        fn visit_expr_call(&mut self, c: &syn::ExprCall) {
            if let syn::Expr::Path(p) = &*c.func {
                if is_api(
                    self.imports,
                    &path_segments(&p.path),
                    "axum",
                    &["Router", "new"],
                ) {
                    self.found = true;
                }
            }
            syn::visit::visit_expr_call(self, c);
        }
    }
    let groups = ev.ctx.parts.module_items(&site.module);
    let imports = Imports::of(&groups);
    let mut f = Finder {
        imports: &imports,
        found: false,
        _p: std::marker::PhantomData,
    };
    f.visit_block(&site.item.block);
    f.found
}

/// 함수 본문에서 서빙되는 라우터 식 — `serve(_, X)`, `X.into_make_service()`.
fn served_exprs(ev: &Eval, site: &FnSite) -> Vec<syn::Expr> {
    struct Finder<'b> {
        imports: &'b Imports,
        out: Vec<syn::Expr>,
    }
    impl Visit<'_> for Finder<'_> {
        fn visit_expr_call(&mut self, c: &syn::ExprCall) {
            if let syn::Expr::Path(p) = &*c.func {
                if is_api(self.imports, &path_segments(&p.path), "axum", &["serve"])
                    && c.args.len() == 2
                {
                    self.out.push(strip_make_service(&c.args[1]));
                    // 인자의 into_make_service를 다시 세지 않는다.
                    return;
                }
            }
            syn::visit::visit_expr_call(self, c);
        }
        fn visit_expr_method_call(&mut self, m: &syn::ExprMethodCall) {
            if m.method == "into_make_service" || m.method == "into_make_service_with_connect_info"
            {
                self.out.push((*m.receiver).clone());
                // 수신자 안의 serve 호출은 없다 — 다시 내려가지 않는다.
                return;
            }
            syn::visit::visit_expr_method_call(self, m);
        }
    }
    let groups = ev.ctx.parts.module_items(&site.module);
    let imports = Imports::of(&groups);
    let mut f = Finder {
        imports: &imports,
        out: Vec::new(),
    };
    f.visit_block(&site.item.block);
    f.out
}

/// `app.into_make_service()` 인자는 수신자 라우터다.
fn strip_make_service(e: &syn::Expr) -> syn::Expr {
    if let syn::Expr::MethodCall(m) = e {
        if m.method == "into_make_service" || m.method == "into_make_service_with_connect_info" {
            return (*m.receiver).clone();
        }
    }
    e.clone()
}

/// 크레이트의 `NormalizePathLayer` 효과. Router::layer 인자로만 쓰였으면 효과가 없다
/// (axum 문서: 그 미들웨어는 라우팅 뒤에 돈다).
fn normalize_effect(ev: &Eval, fns: &BTreeMap<String, FnSite>) -> Normalize {
    struct Finder {
        trim: usize,
        append: usize,
        other: usize,
    }
    impl Visit<'_> for Finder {
        fn visit_path(&mut self, p: &syn::Path) {
            let segs: Vec<String> = p.segments.iter().map(|s| s.ident.to_string()).collect();
            if let Some(i) = segs
                .iter()
                .position(|s| s == "NormalizePathLayer" || s == "NormalizePath")
            {
                match segs.get(i + 1).map(String::as_str) {
                    Some("trim_trailing_slash") => self.trim += 1,
                    Some("append_trailing_slash") => self.append += 1,
                    _ => {}
                }
            }
            syn::visit::visit_path(self, p);
        }
        fn visit_expr_method_call(&mut self, m: &syn::ExprMethodCall) {
            if m.method == "trim_trailing_slash" || m.method == "append_trailing_slash" {
                self.other += 1;
            }
            syn::visit::visit_expr_method_call(self, m);
        }
    }
    let mut f = Finder {
        trim: 0,
        append: 0,
        other: 0,
    };
    for site in fns.values() {
        f.visit_block(&site.item.block);
    }
    let total = f.trim + f.append;
    if total == 0 && f.other == 0 {
        return Normalize::None;
    }
    if total <= ev.layer_normalize && f.other == 0 {
        return Normalize::None;
    }
    match (f.trim > 0, f.append > 0) {
        (true, false) => Normalize::Trim,
        (false, true) => Normalize::Append,
        _ => Normalize::Unknown,
    }
}

/// 편 항목을 선언·한계로 바꾼다.
struct Emit<'a, 'o> {
    ctx: &'a Ctx<'a>,
    version: Version,
    normalize: Normalize,
    out: &'o mut Output,
    /// base 선언의 (접미사 스코프 원소, 루트에서 닿지 않은 라우터인가) — 한계용.
    bases: Vec<(Option<String>, bool)>,
}

impl Emit<'_, '_> {
    fn flat(&mut self, f: &Flat, unrooted: bool) {
        match &f.kind {
            FlatKind::Route(mv, loc) => self.route(&f.path, mv, loc, unrooted),
            FlatKind::RouteService(loc) => {
                let mv = MethodVal {
                    routes: vec![MRoute {
                        methods: None,
                        handler: HandlerRef::Unknown,
                    }],
                    unknown: false,
                };
                self.route(&f.path, &mv, loc, unrooted);
            }
            FlatKind::NestService(loc) => self.prefix_gap(
                "framework-provided-routes:",
                format!(
                    "a nested tower service at {} may serve any path under its prefix",
                    self.at(loc)
                ),
                &f.path,
            ),
            FlatKind::Fallback(loc, nested) => {
                let text = format!(
                    "a router fallback at {} receives requests that no route matches",
                    self.at(loc)
                );
                if *nested {
                    self.prefix_gap("route-coverage:", text, &f.path);
                } else {
                    self.out.gap("route-coverage:", text);
                }
            }
            FlatKind::Unknown(loc, what) => {
                let text = format!("{} at {} may register routes", what, self.at(loc));
                self.prefix_gap("route-coverage:", text, &f.path);
            }
        }
    }

    /// `파일:줄` 표기.
    fn at(&self, loc: &Loc) -> String {
        match self.ctx.locate(loc) {
            Some(l) => format!("{}:{}", l.path, l.line),
            None => "<unknown location>".to_string(),
        }
    }

    /// 접두사 아래 전부를 덮는 한계 — 접두사가 리터럴 root일 때만 스코프를 단다.
    fn prefix_gap(&mut self, prefix: &'static str, text: String, path: &JPath) {
        if let JPath::Lit(raw) = path {
            if let Ok(segs) = parse(self.version, raw) {
                if let Some(t) = prefix_template(&segs) {
                    if t != "/" {
                        self.out.scoped_gap(
                            prefix,
                            text,
                            ScopeSpec {
                                prefixes: vec![t],
                                ..Default::default()
                            },
                        );
                        return;
                    }
                }
            }
        }
        self.out.gap(prefix, text);
    }

    /// 경로 등록 하나를 method별 선언으로 낸다.
    fn route(&mut self, path: &JPath, mv: &MethodVal, loc: &Loc, unrooted: bool) {
        if mv.unknown {
            self.out.gap(
                "route-coverage:",
                format!(
                    "a method router at {} could not be evaluated; its methods are unknown",
                    self.at(loc)
                ),
            );
        }
        let (paths, anchor) = match path {
            JPath::Lit(raw) | JPath::Base(raw) => {
                let anchor = if matches!(path, JPath::Base(_)) {
                    Anchor::Base
                } else {
                    Anchor::Root
                };
                let raw = if raw.is_empty() { "/" } else { raw.as_str() };
                match parse(self.version, raw) {
                    Ok(segs) => match self.empty_variants(segs, loc) {
                        Some(vs) => (vs, anchor),
                        None => (vec![DeclPath::Dynamic(raw.to_string())], Anchor::Root),
                    },
                    Err(e) => match self.parse_failure(raw, loc, e) {
                        Some(p) => (vec![p], Anchor::Root),
                        None => return,
                    },
                }
            }
            JPath::Dyn(dl) => {
                self.out.gap(
                    "route-coverage:",
                    format!(
                        "a route path at {} is not a literal; the declaration is dynamic",
                        self.at(dl)
                    ),
                );
                (
                    vec![DeclPath::Dynamic(self.ctx.source_text(&dl.file, dl.span))],
                    Anchor::Root,
                )
            }
        };
        if anchor == Anchor::Base {
            for p in &paths {
                let suffix = match p {
                    DeclPath::Template { segs, .. } => base_suffix(segs),
                    DeclPath::Dynamic(_) => None,
                };
                self.bases.push((suffix, unrooted));
            }
        }
        for r in &mv.routes {
            let methods = match &r.methods {
                None => vec!["ANY".to_string()],
                Some(ms) => {
                    let mut keep = Vec::new();
                    for m in ms {
                        if m == "CONNECT" {
                            self.out.gap(
                                "route-coverage:",
                                format!(
                                    "a CONNECT route at {} has no http-domain method",
                                    self.at(loc)
                                ),
                            );
                        } else {
                            keep.push(m.clone());
                        }
                    }
                    keep
                }
            };
            if methods.is_empty() {
                continue;
            }
            for p in &paths {
                self.out.decls.push(Decl {
                    methods: methods.clone(),
                    path: p.clone(),
                    anchor,
                    trailing: self.trailing(p),
                    handler: self.handler(&r.handler),
                    loc: loc.clone(),
                    order: None,
                    narrowed: false,
                });
            }
        }
    }

    /// 빈 값 변형 — matchit은 마지막이 아닌 세그먼트의 파라미터(세그먼트 전체·앞
    /// 글자 붙은 파라미터)가 빈 값과도 맞는다(오라클 실측: `/items//tags/x`,
    /// `/v/status`). 계약의 `{}`는 비어 있지 않으므로 빈 값 자리를 리터럴로 채운
    /// 변형을 함께 낸다. 16개를 넘으면 None(dynamic과 펼침 상한 한계).
    fn empty_variants(&mut self, segs: Vec<Seg>, loc: &Loc) -> Option<Vec<DeclPath>> {
        let last = segs.len().saturating_sub(1);
        let slots: Vec<usize> = segs
            .iter()
            .enumerate()
            .filter(|(i, s)| *i < last && matches!(s, Seg::Param { .. }))
            .map(|(i, _)| i)
            .collect();
        if slots.len() > 4 {
            self.out.gap(
                "route-template-expansion-capped:",
                format!(
                    "a route at {} has {} parameters that also match an empty value; more than 16 variants, so the declaration is dynamic",
                    self.at(loc),
                    slots.len()
                ),
            );
            return None;
        }
        let mut out = Vec::new();
        for mask in 0u32..(1 << slots.len()) {
            let mut v = segs.clone();
            for (bit, &i) in slots.iter().enumerate() {
                if mask & (1 << bit) != 0 {
                    if let Seg::Param { prefix, suffix, .. } = &v[i] {
                        v[i] = Seg::Lit(format!("{prefix}{suffix}"));
                    }
                }
            }
            out.push(DeclPath::Template {
                segs: v,
                empty_tail: None,
            });
        }
        Some(out)
    }

    /// 템플릿으로 못 바꾼 경로의 처리 — 기동 시 panic하는 경로는 선언이 아니고
    /// 서버 전체가 뜨지 않으므로 스코프 없는 한계, 원문 경로로만 닿는 경로는 그
    /// 정규 템플릿 스코프의 한계, 세그먼트를 넘는 catch-all은 dynamic 선언이다.
    fn parse_failure(&mut self, raw: &str, loc: &Loc, e: PathError) -> Option<DeclPath> {
        let at = self.at(loc);
        match e {
            PathError::Panic(why) => {
                self.out.gap(
                    "route-coverage:",
                    format!("route path {raw:?} at {at} {why}"),
                );
                None
            }
            PathError::Encoded(canonical) => {
                let text = format!(
                    "route path {raw:?} at {at} has characters that clients percent-encode; axum matches the raw request path, so encoded requests do not reach it"
                );
                match canonical {
                    Some(t) => self.out.scoped_gap(
                        "route-coverage:",
                        text,
                        ScopeSpec {
                            templates: vec![t],
                            ..Default::default()
                        },
                    ),
                    None => self.out.gap("route-coverage:", text),
                }
                None
            }
            PathError::NoTemplate(why) => {
                self.out.gap(
                    "route-coverage:",
                    format!("route path {raw:?} at {at} {why}; the declaration is dynamic"),
                );
                Some(DeclPath::Dynamic(raw.to_string()))
            }
        }
    }

    /// 핸들러 식을 정점 ID로 해석한다.
    fn handler(&self, h: &HandlerRef) -> Handler {
        h.resolve(self.ctx)
    }

    /// 끝 슬래시 판정 — axum은 엄격하다. 바깥 경로 정규화 레이어가 있으면 바뀐다.
    fn trailing(&self, p: &DeclPath) -> Trailing {
        let DeclPath::Template { segs, .. } = p else {
            return Trailing::Unknown;
        };
        if matches!(segs.last(), Some(Seg::CatchAll)) {
            return Trailing::Unknown;
        }
        let rendered = render(segs);
        let ends_slash = rendered.len() > 1 && rendered.ends_with('/');
        match self.normalize {
            Normalize::None => Trailing::Strict,
            Normalize::Trim if rendered == "/" => Trailing::Strict,
            Normalize::Trim if !ends_slash => Trailing::Optional,
            Normalize::Append if ends_slash => Trailing::Optional,
            Normalize::Trim | Normalize::Append => Trailing::Strict,
            Normalize::Unknown => Trailing::Unknown,
        }
    }

    /// base 선언의 공백을 사유별로 한 번씩 낸다 — 스코프는 모든 base 템플릿을
    /// 접미사로 쓸 수 있을 때만 단다.
    fn finish_bases(&mut self) {
        for unrooted in [true, false] {
            let group: Vec<&Option<String>> = self
                .bases
                .iter()
                .filter(|(_, u)| *u == unrooted)
                .map(|(s, _)| s)
                .collect();
            if group.is_empty() {
                continue;
            }
            let n = group.len();
            let text = if unrooted {
                format!("{n} route declaration(s) come from routers that are not reachable from a served router (axum::serve or into_make_service); they are emitted with pathAnchor base")
            } else {
                format!("{n} route declaration(s) sit under a nest prefix that is not a literal; they are emitted with pathAnchor base")
            };
            if group.iter().all(|s| s.is_some()) {
                let mut suffixes: Vec<String> = group.iter().filter_map(|s| (*s).clone()).collect();
                suffixes.sort();
                suffixes.dedup();
                self.out.scoped_gap(
                    "unresolved-route-prefix:",
                    text,
                    ScopeSpec {
                        suffixes,
                        ..Default::default()
                    },
                );
            } else {
                self.out.gap("unresolved-route-prefix:", text);
            }
        }
    }
}

/// base 선언을 덮는 접미사 스코프 원소 — `{**}`·루트는 접미사로 쓸 수 없다.
fn base_suffix(segs: &[Seg]) -> Option<String> {
    if segs.iter().any(|s| matches!(s, Seg::CatchAll)) {
        return None;
    }
    let t = render(segs);
    (t != "/").then_some(t)
}

/// 템플릿으로 못 바꾼 axum 경로의 사유.
#[derive(Debug)]
pub(super) enum PathError {
    /// axum이 기동 시 panic으로 거부한다.
    Panic(String),
    /// 리터럴에 클라이언트가 인코딩하는 글자가 있다 — 정규 템플릿(있으면).
    Encoded(Option<String>),
    /// 라우트는 있지만 정규 템플릿으로 쓸 수 없다.
    NoTemplate(String),
}

/// 버전별 axum 경로 문법을 세그먼트로 바꾼다.
///
/// matchit은 요청의 원문(퍼센트 인코딩된) 경로를 바이트로 비교한다. 그래서 리터럴이
/// 정규형(`normalize_uri_path`)과 다르면(중괄호·공백·ASCII 밖 글자·인코딩된
/// unreserved) 정규 템플릿으로 보낸 요청은 닿지 않는다 — `Encoded`로 돌려준다.
pub(super) fn parse(version: Version, raw: &str) -> Result<Vec<Seg>, PathError> {
    if !raw.starts_with('/') {
        return Err(PathError::Panic(
            "does not start with `/` (axum panics at startup)".to_string(),
        ));
    }
    let body = &raw[1..];
    let parts: Vec<&str> = body.split('/').collect();
    let mut segs = Vec::with_capacity(parts.len());
    for (i, part) in parts.iter().enumerate() {
        let last = i == parts.len() - 1;
        let seg = match version {
            Version::V07 => segment_v07(part, last),
            Version::V08 => segment_v08(part, last),
        };
        segs.push(seg?);
    }
    let raw_literal = |t: &str| normalize_uri_path(t) == t;
    let encoded = segs.iter().any(|s| match s {
        Seg::Lit(t) => !raw_literal(t),
        Seg::Param { prefix, suffix, .. } => !raw_literal(prefix) || !raw_literal(suffix),
        Seg::CatchAll => false,
    });
    if encoded {
        let t = render(&segs);
        return Err(PathError::Encoded(
            template_problem(&t).is_none().then_some(t),
        ));
    }
    Ok(segs)
}

/// matchit 0.7 세그먼트: `:`·`*`는 세그먼트 어디서든 와일드카드를 시작해 다음
/// `/`까지 이어진다(tree.rs:651-670). `:name` 뒤 글자는 이름에 흡수되므로 파라미터는
/// 항상 세그먼트 끝까지다. `*name`은 `/` 바로 뒤, 경로 끝에만 온다.
fn segment_v07(part: &str, last: bool) -> Result<Seg, PathError> {
    let Some(at) = part.find([':', '*']) else {
        return Ok(Seg::Lit(part.to_string()));
    };
    let name = &part[at + 1..];
    if name.is_empty() {
        return Err(PathError::Panic(
            "has an unnamed parameter (axum panics at startup)".to_string(),
        ));
    }
    if name.contains([':', '*']) {
        return Err(PathError::Panic(
            "has two parameters in one segment (axum panics at startup)".to_string(),
        ));
    }
    if part.as_bytes()[at] == b'*' {
        if at != 0 || !last {
            return Err(PathError::Panic(
                "has a catch-all that is not a whole last segment (axum panics at startup)"
                    .to_string(),
            ));
        }
        return Ok(Seg::CatchAll);
    }
    Ok(Seg::Param {
        prefix: part[..at].to_string(),
        suffix: String::new(),
        constraint: None,
    })
}

/// matchit 0.8 세그먼트: `{name}`은 앞에 정적 글자를 둘 수 있지만 뒤에는 못 둔다
/// (tree.rs:783-788 InvalidParamSegment). `{*name}`은 경로 끝에만 온다(372-374).
/// `{{`·`}}`는 리터럴 중괄호다(escape.rs). `:`·`*`로 시작하는 세그먼트는 axum이
/// 기동 시 panic한다(path_router.rs:53-73 validate_v07_paths).
fn segment_v08(part: &str, last: bool) -> Result<Seg, PathError> {
    if part.starts_with(':') || part.starts_with('*') {
        return Err(PathError::Panic(
            "starts a segment with `:` or `*`, which axum 0.8 rejects at startup".to_string(),
        ));
    }
    let mut lit = String::new();
    let mut chars = part.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '{' if chars.peek().map(|(_, n)| *n) == Some('{') => {
                chars.next();
                lit.push('{');
            }
            '}' if chars.peek().map(|(_, n)| *n) == Some('}') => {
                chars.next();
                lit.push('}');
            }
            '{' => {
                let rest = &part[i + 1..];
                let Some(close) = rest.find('}') else {
                    return Err(PathError::Panic(
                        "has an unclosed `{` (axum panics at startup)".to_string(),
                    ));
                };
                let name = &rest[..close];
                let after = &rest[close + 1..];
                if name.is_empty() || name == "*" {
                    return Err(PathError::Panic(
                        "has an unnamed parameter (axum panics at startup)".to_string(),
                    ));
                }
                if !after.is_empty() {
                    return Err(PathError::Panic(
                        "has text after a parameter in one segment (axum panics at startup)"
                            .to_string(),
                    ));
                }
                if name.starts_with('*') {
                    if !last {
                        return Err(PathError::Panic(
                            "has a catch-all before the end (axum panics at startup)".to_string(),
                        ));
                    }
                    if !lit.is_empty() {
                        return Err(PathError::NoTemplate(
                            "has a catch-all after static text in one segment; the capture can span segments, so it has no canonical template"
                                .to_string(),
                        ));
                    }
                    return Ok(Seg::CatchAll);
                }
                return Ok(Seg::Param {
                    prefix: lit,
                    suffix: String::new(),
                    constraint: None,
                });
            }
            '}' => {
                return Err(PathError::Panic(
                    "has an unmatched `}` (axum panics at startup)".to_string(),
                ))
            }
            other => lit.push(other),
        }
    }
    Ok(Seg::Lit(lit))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(v: Version, raw: &str) -> String {
        match parse(v, raw) {
            Ok(segs) => render(&segs),
            Err(e) => format!("ERR {e:?}"),
        }
    }

    #[test]
    fn v07_syntax() {
        assert_eq!(t(Version::V07, "/items/:id"), "/items/{}");
        assert_eq!(t(Version::V07, "/user_:id"), "/user_{}");
        assert_eq!(t(Version::V07, "/f/:id.json"), "/f/{}");
        assert_eq!(t(Version::V07, "/files/*rest"), "/files/{**}");
        assert!(t(Version::V07, "/{id}").starts_with("ERR Encoded"));
        assert!(t(Version::V07, "/a/:x:y").starts_with("ERR"));
        assert!(t(Version::V07, "/a*x").starts_with("ERR"));
    }

    #[test]
    fn v08_syntax() {
        assert_eq!(t(Version::V08, "/items/{id}"), "/items/{}");
        assert_eq!(t(Version::V08, "/v{version}/x"), "/v{}/x");
        assert_eq!(t(Version::V08, "/files/{*rest}"), "/files/{**}");
        assert!(t(Version::V08, "/{{x}}").starts_with("ERR Encoded"));
        assert_eq!(t(Version::V08, "/a:b"), "/a:b");
        assert!(t(Version::V08, "/:id").starts_with("ERR"));
        assert!(t(Version::V08, "/{id}.json").starts_with("ERR"));
        assert!(t(Version::V08, "/pre{*rest}").starts_with("ERR"));
    }

    #[test]
    fn nest_join_matches_axum() {
        assert_eq!(path_for_nested_route("/api", "/"), "/api");
        assert_eq!(path_for_nested_route("/api", "/x"), "/api/x");
        assert_eq!(path_for_nested_route("/api/", "/x"), "/api/x");
        assert_eq!(path_for_nested_route("/api/", "/"), "/api/");
        assert_eq!(path_for_nested_route("/", "/x"), "/x");
    }
}
