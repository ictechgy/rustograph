//! actix-web 4 라우트 선언 추출.
//!
//! 규칙과 근거(actix-web 4.15.0·actix-router 0.5.4·actix-web-codegen 4.4.0 소스
//! 줄 번호)는 docs/HTTP-ROUTES.md에 있다. 요약:
//!
//! - App·Scope 라우터는 서비스를 등록 순서대로 시도하고 첫 매치가 이긴다(정렬
//!   없음, app_service.rs:302-311·router.rs:54-58) → `dispatch: "registration-order"`.
//!   리소스 하나가 등록 하나(`order.index`)이고 App마다 group 하나다.
//! - `#[get]` 매크로·`App::route`·`Scope::route`는 method 가드를 리소스 수준에
//!   붙여(route.rs:456-463, app.rs:227-233) method가 안 맞으면 다음 리소스로
//!   넘어간다. `web::resource().route(web::get())`은 라우트 수준이라 405다.
//! - 스코프는 접두사를 소비하고 안쪽은 나머지로 정확 매치한다 — 결합은 문자열
//!   연결과 같다(resource.rs:503-519 `join`, introspection.rs:576-594).
//! - 끝 슬래시는 엄격하고 App·Scope의 `NormalizePath` 미들웨어가 바꾼다.

use super::common::{
    handler_ref, is_api, order_group, pat_ident, path_arg, prefix_template, str_lit,
    unevaluated_constructors, Anchor, Ctx, Decl, DeclPath, FnSite, Handler, HandlerRef, Imports,
    JPath, Loc, Output, PathArg, ScopeSpec, Trailing,
};
use super::pattern::parse_actix;
use super::template::{render, Seg};
use crate::harvest::path_segments;
use std::collections::{BTreeMap, BTreeSet};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::visit::Visit;

/// method 매크로 이름과 동사(actix-web-codegen route.rs:94-104).
const METHOD_MACROS: &[(&str, &str)] = &[
    ("get", "GET"),
    ("post", "POST"),
    ("put", "PUT"),
    ("delete", "DELETE"),
    ("head", "HEAD"),
    ("connect", "CONNECT"),
    ("options", "OPTIONS"),
    ("trace", "TRACE"),
    ("patch", "PATCH"),
];

/// isthmus가 받는 동사(CONNECT·사용자 동사는 http 도메인에 없다).
const HTTP_METHODS: &[&str] = &[
    "GET", "HEAD", "POST", "PUT", "PATCH", "DELETE", "OPTIONS", "TRACE",
];

/// 한 크레이트의 actix-web 라우트를 추출한다.
pub(super) fn extract(ctx: &Ctx, krate: &str, out: &mut Output) {
    let fns = ctx.crate_fns(krate);
    let mut ev = Eval {
        ctx,
        fns: &fns,
        imports: BTreeMap::new(),
        macros: BTreeMap::new(),
        registered: BTreeSet::new(),
        memo: BTreeMap::new(),
        stack: Vec::new(),
        out_gaps: Vec::new(),
    };
    for site in fns.values() {
        let regs = ev.macro_regs(site);
        if !regs.is_empty() {
            ev.macros.insert(site.id.clone(), regs);
        }
    }
    let mut apps: Vec<(String, AppVal)> = Vec::new();
    for site in fns.values() {
        let found = ev.apps_in(site);
        let many = found.len() > 1;
        for (i, app) in found.into_iter().enumerate() {
            let id = if many && i > 0 {
                format!("{}#{}", site.id, i + 1)
            } else {
                site.id.clone()
            };
            let group = order_group("actix:", &id);
            apps.push((group, app));
        }
    }
    let gaps = std::mem::take(&mut ev.out_gaps);
    out.gaps.extend(gaps);
    for loc in unevaluated_constructors(ctx, krate, "actix_web", &["App", "new"]) {
        let at = ctx
            .locate(&loc)
            .map(|l| format!("{}:{}", l.path, l.line))
            .unwrap_or_default();
        out.gap(
            "route-coverage:",
            format!("an App built inside a method at {at} is not evaluated; its routes are not extracted"),
        );
    }
    let macros = std::mem::take(&mut ev.macros);
    let mut emit = Emit {
        ctx,
        out,
        macros: &macros,
        bases: Vec::new(),
    };
    for (group, app) in &apps {
        let mut index = 0u64;
        emit.services(
            &app.services,
            &JPath::Lit(String::new()),
            &Guard::default(),
            app.normalize,
            Some((group.as_str(), &mut index)),
        );
        for loc in &app.defaults {
            emit.out.gap(
                "route-coverage:",
                format!(
                    "an App default service at {} receives requests that no route matches",
                    emit.at(loc)
                ),
            );
        }
    }
    // 어느 App에도 등록되지 않은 매크로 핸들러 — 어디에 붙는지 모른다.
    let unregistered: Vec<(String, Vec<MacroReg>)> = macros
        .iter()
        .filter(|(id, _)| !ev.registered.contains(*id))
        .map(|(id, regs)| (id.clone(), regs.clone()))
        .collect();
    for (id, regs) in &unregistered {
        for reg in regs {
            emit.resource_decls(
                std::slice::from_ref(&reg.path),
                &reg.loc,
                &Guard {
                    methods: reg.methods.clone(),
                    narrowed: reg.narrowed,
                },
                &[RouteVal::any(HandlerRef::Unknown)],
                &JPath::Dyn(reg.loc.clone()),
                Normalize::None,
                None,
                Some(Handler::Usr(id.clone())),
            );
        }
    }
    emit.finish_bases();
}

/// `NormalizePath` 모드(normalize.rs:44-50, 174-228).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Normalize {
    #[default]
    None,
    Trim,
    Always,
    MergeOnly,
    Unknown,
}

/// 가드 요약 — method 제한(None = 모두)과 그 밖의 조건(narrowed).
#[derive(Clone, Debug, Default)]
struct Guard {
    methods: Option<BTreeSet<String>>,
    narrowed: bool,
}

impl Guard {
    /// 두 가드를 모두 통과해야 한다(교집합).
    fn and(&self, o: &Guard) -> Guard {
        let methods = match (&self.methods, &o.methods) {
            (None, m) | (m, None) => m.clone(),
            (Some(a), Some(b)) => Some(a.intersection(b).cloned().collect()),
        };
        Guard {
            methods,
            narrowed: self.narrowed || o.narrowed,
        }
    }
}

/// 매크로 등록 하나(`#[get("/x")]` 하나 = 리소스 하나).
#[derive(Clone, Debug)]
struct MacroReg {
    path: PathArg,
    loc: Loc,
    methods: Option<BTreeSet<String>>,
    narrowed: bool,
}

/// 라우트 하나 — 라우트 수준 가드와 핸들러.
#[derive(Clone, Debug)]
struct RouteVal {
    guard: Guard,
    handler: HandlerRef,
}

impl RouteVal {
    fn any(handler: HandlerRef) -> RouteVal {
        RouteVal {
            guard: Guard::default(),
            handler,
        }
    }
}

/// 리소스 — 경로(여러 개일 수 있다), 리소스 수준 가드, 라우트들.
#[derive(Clone, Debug)]
struct ResourceVal {
    paths: Vec<PathArg>,
    loc: Loc,
    guard: Guard,
    routes: Vec<RouteVal>,
}

/// 스코프 — 접두사, 가드, 안쪽 서비스, 정규화, 기본 서비스.
#[derive(Clone, Debug)]
struct ScopeVal {
    prefix: PathArg,
    guard: Guard,
    services: Vec<Svc>,
    normalize: Option<Normalize>,
    defaults: Vec<Loc>,
}

/// 등록된 서비스 하나.
#[derive(Clone, Debug)]
enum Svc {
    Resource(ResourceVal),
    Scope(ScopeVal),
    Macro(String),
    Files { prefix: PathArg },
    Unknown { loc: Loc, what: String },
}

/// App 값.
#[derive(Clone, Debug, Default)]
struct AppVal {
    services: Vec<Svc>,
    normalize: Normalize,
    defaults: Vec<Loc>,
}

/// 평가 값.
#[derive(Clone, Debug)]
enum Val {
    App(AppVal),
    Scope(ScopeVal),
    Resource(ResourceVal),
    Route(RouteVal),
    Files(PathArg),
}

/// 평가기.
struct Eval<'a> {
    ctx: &'a Ctx<'a>,
    fns: &'a BTreeMap<String, FnSite>,
    imports: BTreeMap<String, Imports>,
    macros: BTreeMap<String, Vec<MacroReg>>,
    registered: BTreeSet<String>,
    memo: BTreeMap<String, Option<Val>>,
    stack: Vec<String>,
    out_gaps: Vec<super::common::Gap>,
}

impl Eval<'_> {
    fn imports(&mut self, module: &str) -> &Imports {
        if !self.imports.contains_key(module) {
            let groups = self.ctx.parts.module_items(module);
            self.imports
                .insert(module.to_string(), Imports::of(&groups));
        }
        &self.imports[module]
    }

    fn is_actix(&mut self, module: &str, segs: &[String], tail: &[&str]) -> bool {
        is_api(self.imports(module), segs, "actix_web", tail)
    }

    /// 함수에 붙은 라우트 매크로 등록들(`#[routes]`면 속성 순서대로 여럿).
    fn macro_regs(&mut self, site: &FnSite) -> Vec<MacroReg> {
        let mut regs = Vec::new();
        // `#[routes]` 아래의 method 속성은 그 매크로가 읽는 도우미라 import가 없어도
        // 된다(codegen route.rs:509-550) — actix `routes`가 붙었으면 이름만으로 받는다.
        let under_routes = site.item.attrs.iter().any(|a| {
            let segs = path_segments(a.path());
            segs.last().is_some_and(|l| l == "routes")
                && self.is_actix(&site.module, &segs, &["routes"])
        });
        for attr in &site.item.attrs {
            let segs = path_segments(attr.path());
            let Some(name) = segs.last().cloned() else {
                continue;
            };
            let verb = METHOD_MACROS
                .iter()
                .find(|(m, _)| *m == name)
                .map(|(_, v)| v.to_string());
            if verb.is_none() && name != "route" {
                continue;
            }
            let helper = under_routes && verb.is_some() && segs.len() == 1;
            if !helper && !self.is_actix(&site.module, &segs, &[name.as_str()]) {
                continue;
            }
            match parse_macro(site, attr, verb) {
                Ok(reg) => regs.push(reg),
                Err(why) => self.out_gaps.push(super::common::Gap {
                    prefix: "route-coverage:",
                    text: format!("route macro on {} {why}", site.id),
                    scope: None,
                }),
            }
        }
        regs
    }

    /// 함수 본문의 App 체인을 찾아 평가한다(`App::new()`에서 시작하는 가장 바깥 체인).
    fn apps_in(&mut self, site: &FnSite) -> Vec<AppVal> {
        struct Finder<'b> {
            imports: &'b Imports,
            chains: Vec<syn::Expr>,
        }
        impl Visit<'_> for Finder<'_> {
            fn visit_expr(&mut self, e: &syn::Expr) {
                if chain_root_is(self.imports, e, &["App", "new"]) {
                    self.chains.push(e.clone());
                    // 인자 안의 App은 없다고 본다 — 체인 안으로 내려가지 않는다.
                    return;
                }
                syn::visit::visit_expr(self, e);
            }
        }
        let groups = self.ctx.parts.module_items(&site.module);
        let imports = Imports::of(&groups);
        let mut f = Finder {
            imports: &imports,
            chains: Vec::new(),
        };
        f.visit_block(&site.item.block);
        let env = BTreeMap::new();
        f.chains
            .iter()
            .filter_map(|e| match self.eval(site, &env, e) {
                Some(Val::App(a)) => Some(a),
                _ => None,
            })
            .collect()
    }

    /// 크레이트 함수의 반환 값(스코프·리소스 등)을 평가한다.
    fn eval_fn(&mut self, id: &str) -> Option<Val> {
        if let Some(v) = self.memo.get(id) {
            return v.clone();
        }
        if self.stack.iter().any(|s| s == id) {
            return None;
        }
        let site = self.fns.get(id)?;
        self.stack.push(id.to_string());
        let mut env = BTreeMap::new();
        let mut result = None;
        for stmt in &site.item.block.stmts {
            match stmt {
                syn::Stmt::Local(l) => {
                    if let (Some(name), Some(init)) = (pat_ident(&l.pat), &l.init) {
                        if let Some(v) = self.eval(site, &env, &init.expr) {
                            env.insert(name, v);
                        }
                    }
                }
                syn::Stmt::Expr(syn::Expr::Return(r), _) => {
                    result = r.expr.as_ref().and_then(|e| self.eval(site, &env, e));
                    break;
                }
                syn::Stmt::Expr(e, None) => result = self.eval(site, &env, e),
                _ => {}
            }
        }
        self.stack.pop();
        self.memo.insert(id.to_string(), result.clone());
        result
    }

    /// 식을 평가한다.
    fn eval(&mut self, site: &FnSite, env: &BTreeMap<String, Val>, e: &syn::Expr) -> Option<Val> {
        match e {
            syn::Expr::Paren(p) => self.eval(site, env, &p.expr),
            syn::Expr::Group(g) => self.eval(site, env, &g.expr),
            syn::Expr::Path(p) => env.get(&p.path.get_ident()?.to_string()).cloned(),
            syn::Expr::Call(c) => self.eval_call(site, env, c),
            syn::Expr::MethodCall(m) => {
                let recv = self.eval(site, env, &m.receiver)?;
                let args: Vec<&syn::Expr> = m.args.iter().collect();
                Some(self.apply(site, env, recv, &m.method.to_string(), &args, m.span()))
            }
            _ => None,
        }
    }

    /// 생성 호출 — App·Scope·Resource·Route 생성자와 크레이트 함수.
    fn eval_call(
        &mut self,
        site: &FnSite,
        env: &BTreeMap<String, Val>,
        c: &syn::ExprCall,
    ) -> Option<Val> {
        let syn::Expr::Path(p) = &*c.func else {
            return None;
        };
        let segs = path_segments(&p.path);
        let module = site.module.clone();
        if let Some(id) = self.ctx.resolve_vertex(&module, &segs) {
            if self.fns.contains_key(&id) {
                if let Some(v) = self.eval_fn(&id) {
                    return Some(v);
                }
            }
        }
        let args: Vec<&syn::Expr> = c.args.iter().collect();
        let loc = Loc {
            file: site.file.clone(),
            span: c.span(),
        };
        let pa = |ev: &mut Self, e: &syn::Expr| path_arg(ev.ctx, &site.module, &site.file, e);
        if self.is_actix(&module, &segs, &["App", "new"]) {
            return Some(Val::App(AppVal::default()));
        }
        if self.is_actix(&module, &segs, &["scope"])
            || self.is_actix(&module, &segs, &["Scope", "new"])
        {
            let [prefix] = args.as_slice() else {
                return None;
            };
            return Some(Val::Scope(ScopeVal {
                prefix: pa(self, prefix),
                guard: Guard::default(),
                services: Vec::new(),
                normalize: None,
                defaults: Vec::new(),
            }));
        }
        if self.is_actix(&module, &segs, &["resource"])
            || self.is_actix(&module, &segs, &["Resource", "new"])
            || self.is_actix(&module, &segs, &["service"])
        {
            let [paths] = args.as_slice() else {
                return None;
            };
            return Some(Val::Resource(ResourceVal {
                paths: self.resource_paths(site, paths),
                loc: Loc {
                    file: site.file.clone(),
                    span: paths.span(),
                },
                guard: Guard::default(),
                routes: Vec::new(),
            }));
        }
        if self.is_actix(&module, &segs, &["redirect"])
            || self.is_actix(&module, &segs, &["Redirect", "new"])
        {
            let [from, _] = args.as_slice() else {
                return None;
            };
            return Some(Val::Resource(ResourceVal {
                paths: vec![pa(self, from)],
                loc: Loc {
                    file: site.file.clone(),
                    span: from.span(),
                },
                guard: Guard::default(),
                routes: vec![RouteVal::any(HandlerRef::Unknown)],
            }));
        }
        if is_api(
            self.imports(&module),
            &segs,
            "actix_files",
            &["Files", "new"],
        ) {
            let [prefix, _] = args.as_slice() else {
                return None;
            };
            return Some(Val::Files(pa(self, prefix)));
        }
        let name = segs.last()?.clone();
        if let Some((_, verb)) = METHOD_MACROS.iter().find(|(m, _)| *m == name) {
            if self.is_actix(&module, &segs, &["web", name.as_str()]) && args.is_empty() {
                return Some(Val::Route(RouteVal {
                    guard: Guard {
                        methods: Some([verb.to_string()].into()),
                        narrowed: false,
                    },
                    handler: HandlerRef::Unknown,
                }));
            }
        }
        if self.is_actix(&module, &segs, &["web", "route"])
            || self.is_actix(&module, &segs, &["Route", "new"])
        {
            return Some(Val::Route(RouteVal::any(HandlerRef::Unknown)));
        }
        if self.is_actix(&module, &segs, &["web", "method"]) {
            let [m] = args.as_slice() else { return None };
            return Some(Val::Route(RouteVal {
                guard: method_guard(m),
                handler: HandlerRef::Unknown,
            }));
        }
        if self.is_actix(&module, &segs, &["web", "to"]) {
            let [h] = args.as_slice() else { return None };
            return Some(Val::Route(RouteVal::any(handler_ref(
                &site.module,
                &site.file,
                h,
            ))));
        }
        let _ = (env, loc);
        None
    }

    /// `web::resource`의 경로 — 리터럴 하나 또는 배열·`vec!` 원소들.
    fn resource_paths(&mut self, site: &FnSite, e: &syn::Expr) -> Vec<PathArg> {
        let elems: Vec<&syn::Expr> = match e {
            syn::Expr::Array(a) => a.elems.iter().collect(),
            syn::Expr::Reference(r) => return self.resource_paths(site, &r.expr),
            _ => vec![e],
        };
        elems
            .into_iter()
            .map(|x| path_arg(self.ctx, &site.module, &site.file, x))
            .collect()
    }

    /// 체인 메서드를 값에 적용한다. 모르는 메서드는 투명하다(`wrap`·`app_data` 등).
    fn apply(
        &mut self,
        site: &FnSite,
        env: &BTreeMap<String, Val>,
        recv: Val,
        name: &str,
        args: &[&syn::Expr],
        span: proc_macro2::Span,
    ) -> Val {
        let loc = Loc {
            file: site.file.clone(),
            span,
        };
        match recv {
            Val::App(mut a) => {
                match (name, args) {
                    ("service", [x]) => {
                        let svcs = self.services_of(site, env, x);
                        a.services.extend(svcs);
                    }
                    ("route", [p, r]) => a.services.push(self.route_resource(site, env, p, r)),
                    ("configure", [f]) => {
                        let (svcs, defaults) = self.configure(site, f);
                        a.services.extend(svcs);
                        a.defaults.extend(defaults);
                    }
                    ("default_service", [_]) => a.defaults.push(loc),
                    ("wrap", [mw]) => {
                        if let Some(n) = normalize_of(mw) {
                            a.normalize = n;
                        }
                    }
                    _ => {}
                }
                Val::App(a)
            }
            Val::Scope(mut s) => {
                match (name, args) {
                    ("service", [x]) => {
                        let svcs = self.services_of(site, env, x);
                        s.services.extend(svcs);
                    }
                    ("route", [p, r]) => s.services.push(self.route_resource(site, env, p, r)),
                    ("configure", [f]) => {
                        let (svcs, defaults) = self.configure(site, f);
                        s.services.extend(svcs);
                        s.defaults.extend(defaults);
                    }
                    ("default_service", [_]) => s.defaults.push(loc),
                    ("guard", [g]) => s.guard = s.guard.and(&guard_of(g)),
                    ("wrap", [mw]) => {
                        if let Some(n) = normalize_of(mw) {
                            s.normalize = Some(n);
                        }
                    }
                    _ => {}
                }
                Val::Scope(s)
            }
            Val::Resource(mut r) => {
                match (name, args) {
                    ("route", [x]) => match self.eval(site, env, x) {
                        Some(Val::Route(rv)) => r.routes.push(rv),
                        _ => r.routes.push(RouteVal::any(HandlerRef::Unknown)),
                    },
                    ("to" | "finish", [h]) => {
                        r.routes
                            .push(RouteVal::any(handler_ref(&site.module, &site.file, h)))
                    }
                    ("default_service", [x]) => r.routes.push(self.default_route(site, env, x)),
                    ("guard", [g]) => r.guard = r.guard.and(&guard_of(g)),
                    (m, [h]) => {
                        if let Some((_, verb)) = METHOD_MACROS
                            .iter()
                            .find(|(n, _)| *n == m && *n != "connect" && *n != "options")
                        {
                            r.routes.push(RouteVal {
                                guard: Guard {
                                    methods: Some([verb.to_string()].into()),
                                    narrowed: false,
                                },
                                handler: handler_ref(&site.module, &site.file, h),
                            });
                        }
                    }
                    _ => {}
                }
                Val::Resource(r)
            }
            Val::Route(mut rv) => {
                match (name, args) {
                    ("to", [h]) => rv.handler = handler_ref(&site.module, &site.file, h),
                    ("method", [m]) => rv.guard = rv.guard.and(&method_guard(m)),
                    ("guard", [g]) => rv.guard = rv.guard.and(&guard_of(g)),
                    _ => {}
                }
                Val::Route(rv)
            }
            other => other,
        }
    }

    /// `default_service(web::to(h))`처럼 핸들러가 보이면 그 핸들러의 ANY 라우트.
    fn default_route(
        &mut self,
        site: &FnSite,
        env: &BTreeMap<String, Val>,
        x: &syn::Expr,
    ) -> RouteVal {
        match self.eval(site, env, x) {
            Some(Val::Route(rv)) => RouteVal::any(rv.handler),
            _ => RouteVal::any(HandlerRef::Unknown),
        }
    }

    /// `App::route(path, route)` — 라우트 가드를 리소스 수준으로 옮긴다(app.rs:227-233).
    fn route_resource(
        &mut self,
        site: &FnSite,
        env: &BTreeMap<String, Val>,
        p: &syn::Expr,
        r: &syn::Expr,
    ) -> Svc {
        let route = match self.eval(site, env, r) {
            Some(Val::Route(rv)) => rv,
            _ => RouteVal::any(HandlerRef::Unknown),
        };
        Svc::Resource(ResourceVal {
            paths: vec![path_arg(self.ctx, &site.module, &site.file, p)],
            loc: Loc {
                file: site.file.clone(),
                span: p.span(),
            },
            guard: route.guard.clone(),
            routes: vec![RouteVal::any(route.handler)],
        })
    }

    /// `.service(x)`의 서비스들 — 매크로 핸들러, 리소스·스코프 식, 튜플.
    fn services_of(
        &mut self,
        site: &FnSite,
        env: &BTreeMap<String, Val>,
        x: &syn::Expr,
    ) -> Vec<Svc> {
        let loc = Loc {
            file: site.file.clone(),
            span: x.span(),
        };
        match x {
            syn::Expr::Tuple(t) => {
                return t
                    .elems
                    .iter()
                    .flat_map(|e| self.services_of(site, env, e))
                    .collect()
            }
            syn::Expr::Paren(p) => return self.services_of(site, env, &p.expr),
            syn::Expr::Path(p)
                if p.path
                    .get_ident()
                    .is_none_or(|i| !env.contains_key(&i.to_string())) =>
            {
                let segs = path_segments(&p.path);
                if let Some(id) = self.ctx.resolve_vertex(&site.module, &segs) {
                    if self.macros.contains_key(&id) {
                        self.registered.insert(id.clone());
                        return vec![Svc::Macro(id)];
                    }
                }
                return vec![Svc::Unknown {
                    loc,
                    what: "a service the analyzer could not resolve".to_string(),
                }];
            }
            _ => {}
        }
        match self.eval(site, env, x) {
            Some(Val::Resource(r)) => vec![Svc::Resource(r)],
            Some(Val::Scope(s)) => vec![Svc::Scope(s)],
            Some(Val::Files(p)) => vec![Svc::Files { prefix: p }],
            _ => vec![Svc::Unknown {
                loc,
                what: "a service the analyzer could not evaluate".to_string(),
            }],
        }
    }

    /// `.configure(f)` — `fn f(cfg: &mut ServiceConfig)` 또는 클로저 본문의 등록.
    fn configure(&mut self, site: &FnSite, f: &syn::Expr) -> (Vec<Svc>, Vec<Loc>) {
        let loc = Loc {
            file: site.file.clone(),
            span: f.span(),
        };
        let unknown = |what: &str| {
            (
                vec![Svc::Unknown {
                    loc: loc.clone(),
                    what: what.to_string(),
                }],
                Vec::new(),
            )
        };
        match f {
            syn::Expr::Closure(c) => {
                let Some(param) = c.inputs.first().and_then(pat_ident) else {
                    return unknown("a configure closure without a named parameter");
                };
                let mut acc = Config::default();
                self.config_expr(site, &param, &c.body, &mut acc);
                (acc.services, acc.defaults)
            }
            syn::Expr::Path(p) => {
                let segs = path_segments(&p.path);
                match self.ctx.resolve_vertex(&site.module, &segs) {
                    Some(id) if self.fns.contains_key(&id) => {
                        let mut acc = Config::default();
                        self.config_fn(&id, &mut acc);
                        (acc.services, acc.defaults)
                    }
                    _ => unknown("a configure function the analyzer could not resolve"),
                }
            }
            _ => unknown("a configure argument the analyzer could not evaluate"),
        }
    }

    /// `fn f(cfg: &mut ServiceConfig)`의 본문을 평가해 등록을 모은다.
    fn config_fn(&mut self, id: &str, acc: &mut Config) {
        if self.stack.iter().any(|s| s == id) {
            return;
        }
        let Some(site) = self.fns.get(id) else { return };
        let Some(param) = site.item.sig.inputs.iter().find_map(|a| match a {
            syn::FnArg::Typed(t) => pat_ident(&t.pat),
            syn::FnArg::Receiver(_) => None,
        }) else {
            return;
        };
        self.stack.push(id.to_string());
        for stmt in &site.item.block.stmts {
            if let syn::Stmt::Expr(e, _) = stmt {
                self.config_expr(site, &param, e, acc);
            }
        }
        self.stack.pop();
    }

    /// ServiceConfig 식 하나 — `cfg.service(..)` 체인, 블록, 다른 설정 함수 호출.
    fn config_expr(&mut self, site: &FnSite, param: &str, e: &syn::Expr, acc: &mut Config) {
        let env = BTreeMap::new();
        match e {
            syn::Expr::Block(b) => {
                for stmt in &b.block.stmts {
                    if let syn::Stmt::Expr(x, _) = stmt {
                        self.config_expr(site, param, x, acc);
                    }
                }
            }
            syn::Expr::MethodCall(m) => {
                // 체인을 뿌리부터 적용한다: cfg.service(a).service(b)
                let mut calls = Vec::new();
                let mut cur: &syn::Expr = e;
                while let syn::Expr::MethodCall(mc) = cur {
                    calls.push(mc);
                    cur = &mc.receiver;
                }
                let rooted = matches!(cur, syn::Expr::Path(p) if p.path.is_ident(param));
                if !rooted {
                    let _ = m;
                    return;
                }
                for mc in calls.into_iter().rev() {
                    let args: Vec<&syn::Expr> = mc.args.iter().collect();
                    match (mc.method.to_string().as_str(), args.as_slice()) {
                        ("service", [x]) => {
                            let svcs = self.services_of(site, &env, x);
                            acc.services.extend(svcs);
                        }
                        ("route", [p, r]) => {
                            let svc = self.route_resource(site, &env, p, r);
                            acc.services.push(svc);
                        }
                        ("configure", [f]) => {
                            let (svcs, defaults) = self.configure(site, f);
                            acc.services.extend(svcs);
                            acc.defaults.extend(defaults);
                        }
                        ("default_service", [_]) => acc.defaults.push(Loc {
                            file: site.file.clone(),
                            span: mc.span(),
                        }),
                        _ => {}
                    }
                }
            }
            syn::Expr::Call(c) => {
                // 설정 함수에 cfg를 넘기는 호출 — `items::config(cfg)`
                let passes = c.args.iter().any(|a| {
                    let a = match a {
                        syn::Expr::Reference(r) => &*r.expr,
                        other => other,
                    };
                    matches!(a, syn::Expr::Path(p) if p.path.is_ident(param))
                });
                if let (true, syn::Expr::Path(p)) = (passes, &*c.func) {
                    let segs = path_segments(&p.path);
                    if let Some(id) = self.ctx.resolve_vertex(&site.module, &segs) {
                        if self.fns.contains_key(&id) {
                            self.config_fn(&id, acc);
                            return;
                        }
                    }
                    acc.services.push(Svc::Unknown {
                        loc: Loc {
                            file: site.file.clone(),
                            span: c.span(),
                        },
                        what: "a configuration call the analyzer could not resolve".to_string(),
                    });
                }
            }
            syn::Expr::If(_)
            | syn::Expr::Match(_)
            | syn::Expr::ForLoop(_)
            | syn::Expr::While(_)
                if mentions_ident(e, param) =>
            {
                acc.services.push(Svc::Unknown {
                    loc: Loc {
                        file: site.file.clone(),
                        span: e.span(),
                    },
                    what: "a conditional or repeated registration".to_string(),
                });
            }
            _ => {}
        }
    }
}

/// ServiceConfig 누적.
#[derive(Default)]
struct Config {
    services: Vec<Svc>,
    defaults: Vec<Loc>,
}

/// 식이 식별자 `name`을 쓰는가.
fn mentions_ident(e: &syn::Expr, name: &str) -> bool {
    struct Finder<'n>(&'n str, bool);
    impl Visit<'_> for Finder<'_> {
        fn visit_path(&mut self, p: &syn::Path) {
            if p.is_ident(self.0) {
                self.1 = true;
            }
            syn::visit::visit_path(self, p);
        }
    }
    let mut f = Finder(name, false);
    f.visit_expr(e);
    f.1
}

/// 메서드 체인의 뿌리가 `tail` 호출(예: `App::new()`)인가.
fn chain_root_is(imports: &Imports, e: &syn::Expr, tail: &[&str]) -> bool {
    let mut cur = e;
    while let syn::Expr::MethodCall(m) = cur {
        cur = &m.receiver;
    }
    matches!(cur, syn::Expr::Call(c) if matches!(&*c.func, syn::Expr::Path(p)
        if is_api(imports, &path_segments(&p.path), "actix_web", tail)))
}

/// `Method::GET`·`http::Method::POST` 같은 method 식.
fn method_guard(e: &syn::Expr) -> Guard {
    match e {
        syn::Expr::Path(p) => {
            let last = p
                .path
                .segments
                .last()
                .map(|s| s.ident.to_string())
                .unwrap_or_default();
            if HTTP_METHODS.contains(&last.as_str()) || last == "CONNECT" {
                Guard {
                    methods: Some([last].into()),
                    narrowed: false,
                }
            } else {
                Guard {
                    methods: None,
                    narrowed: true,
                }
            }
        }
        _ => Guard {
            methods: None,
            narrowed: true,
        },
    }
}

/// 가드 식 — `guard::Get()`·`guard::Method(..)`·`guard::Any(a).or(b)`·`guard::All(a).and(b)`.
/// method가 아닌 가드(`Header`·`Host`·`fn_guard` 등)는 narrowed다.
fn guard_of(e: &syn::Expr) -> Guard {
    let narrowed = Guard {
        methods: None,
        narrowed: true,
    };
    match e {
        syn::Expr::Call(c) => {
            let syn::Expr::Path(p) = &*c.func else {
                return narrowed;
            };
            let last = p
                .path
                .segments
                .last()
                .map(|s| s.ident.to_string())
                .unwrap_or_default();
            let upper = last.to_ascii_uppercase();
            match last.as_str() {
                "Method" if c.args.len() == 1 => method_guard(&c.args[0]),
                "Any" | "All" if c.args.len() == 1 => guard_of(&c.args[0]),
                _ if c.args.is_empty()
                    && (HTTP_METHODS.contains(&upper.as_str()) || upper == "CONNECT") =>
                {
                    Guard {
                        methods: Some([upper].into()),
                        narrowed: false,
                    }
                }
                _ => narrowed,
            }
        }
        syn::Expr::MethodCall(m)
            if m.args.len() == 1 && (m.method == "or" || m.method == "and") =>
        {
            let a = guard_of(&m.receiver);
            let b = guard_of(&m.args[0]);
            if m.method == "and" {
                return a.and(&b);
            }
            match (a.methods, b.methods) {
                (Some(x), Some(y)) if !a.narrowed && !b.narrowed => Guard {
                    methods: Some(x.union(&y).cloned().collect()),
                    narrowed: false,
                },
                _ => narrowed,
            }
        }
        syn::Expr::Paren(p) => guard_of(&p.expr),
        _ => narrowed,
    }
}

/// `NormalizePath::trim()`·`NormalizePath::new(TrailingSlash::X)`·`::default()`.
fn normalize_of(e: &syn::Expr) -> Option<Normalize> {
    let syn::Expr::Call(c) = e else {
        return None;
    };
    let syn::Expr::Path(p) = &*c.func else {
        return None;
    };
    let segs = path_segments(&p.path);
    let pos = segs.iter().position(|s| s == "NormalizePath")?;
    Some(match segs.get(pos + 1).map(String::as_str) {
        Some("trim") | Some("default") => Normalize::Trim,
        Some("new") => match c.args.first() {
            Some(syn::Expr::Path(a)) => match a
                .path
                .segments
                .last()
                .map(|s| s.ident.to_string())
                .as_deref()
            {
                Some("Trim") => Normalize::Trim,
                Some("Always") => Normalize::Always,
                Some("MergeOnly") => Normalize::MergeOnly,
                _ => Normalize::Unknown,
            },
            _ => Normalize::Unknown,
        },
        _ => Normalize::Unknown,
    })
}

/// 라우트 매크로 속성 하나를 읽는다(codegen route.rs:148-313).
fn parse_macro(
    site: &FnSite,
    attr: &syn::Attribute,
    verb: Option<String>,
) -> Result<MacroReg, String> {
    let args = attr
        .parse_args_with(Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated)
        .map_err(|_| "has arguments the analyzer cannot read".to_string())?;
    let mut iter = args.iter();
    let Some((path, span)) = iter.next().and_then(str_lit) else {
        return Err("has no literal path".to_string());
    };
    let loc = Loc {
        file: site.file.clone(),
        span,
    };
    let mut methods: BTreeSet<String> = verb.into_iter().collect();
    let mut narrowed = false;
    for arg in iter {
        let syn::Expr::Assign(a) = arg else { continue };
        let key = match &*a.left {
            syn::Expr::Path(p) => p.path.get_ident().map(|i| i.to_string()),
            _ => None,
        };
        match key.as_deref() {
            Some("method") => {
                let Some((m, _)) = str_lit(&a.right) else {
                    return Err("has a non-literal method".to_string());
                };
                if !HTTP_METHODS.contains(&m.as_str()) {
                    return Err(format!(
                        "uses method {m}, which the http domain has no verb for"
                    ));
                }
                methods.insert(m);
            }
            Some("guard") => narrowed = true,
            _ => {}
        }
    }
    if methods.iter().any(|m| m == "CONNECT") {
        return Err("is a CONNECT route, which the http domain has no verb for".to_string());
    }
    Ok(MacroReg {
        path: PathArg::Lit(path, loc.clone()),
        loc,
        methods: Some(methods),
        narrowed,
    })
}

/// 스코프 접두사 결합 — `ResourceDef::root_prefix`는 비어 있지 않은 경로에 앞 `/`를
/// 붙이고(resource.rs:327-329), 안쪽 리소스는 `ensure_leading_slash`(dev.rs:30-47)다.
fn actix_join(prefix: &str, path: &str) -> String {
    if path.is_empty() || path.starts_with('/') {
        format!("{prefix}{path}")
    } else {
        format!("{prefix}/{path}")
    }
}

/// 서비스를 선언·한계로 바꾼다.
struct Emit<'a, 'o> {
    ctx: &'a Ctx<'a>,
    out: &'o mut Output,
    /// 매크로 핸들러 ID → 등록들.
    macros: &'a BTreeMap<String, Vec<MacroReg>>,
    /// base 선언의 접미사 스코프 원소(없으면 스코프 불가).
    bases: Vec<Option<String>>,
}

impl Emit<'_, '_> {
    fn at(&self, loc: &Loc) -> String {
        match self.ctx.locate(loc) {
            Some(l) => format!("{}:{}", l.path, l.line),
            None => "<unknown location>".to_string(),
        }
    }

    /// 서비스 목록을 등록 순서대로 편다. `order`가 있으면 리소스마다 index 하나.
    fn services(
        &mut self,
        svcs: &[Svc],
        prefix: &JPath,
        guard: &Guard,
        normalize: Normalize,
        mut order: Option<(&str, &mut u64)>,
    ) {
        for svc in svcs {
            match svc {
                Svc::Resource(r) => {
                    let idx = order.as_mut().map(|(g, i)| {
                        let n = **i;
                        **i += 1;
                        (g.to_string(), n)
                    });
                    self.resource_decls(
                        &r.paths,
                        &r.loc,
                        &guard.and(&r.guard),
                        &r.routes,
                        prefix,
                        normalize,
                        idx,
                        None,
                    );
                }
                Svc::Macro(id) => {
                    let regs = self.macro_regs_of(id);
                    for reg in regs {
                        let idx = order.as_mut().map(|(g, i)| {
                            let n = **i;
                            **i += 1;
                            (g.to_string(), n)
                        });
                        self.resource_decls(
                            std::slice::from_ref(&reg.path),
                            &reg.loc,
                            &guard.and(&Guard {
                                methods: reg.methods.clone(),
                                narrowed: reg.narrowed,
                            }),
                            &[RouteVal::any(HandlerRef::Unknown)],
                            prefix,
                            normalize,
                            idx,
                            Some(Handler::Usr(id.clone())),
                        );
                    }
                }
                Svc::Scope(s) => {
                    let inner = self.scope_prefix(prefix, &s.prefix);
                    for d in &s.defaults {
                        let text = format!(
                            "a scope default service at {} receives requests under the scope that no route matches",
                            self.at(d)
                        );
                        self.prefix_gap("route-coverage:", text, &inner);
                    }
                    let n = s.normalize.unwrap_or(normalize);
                    let reborrow = order.as_mut().map(|(g, i)| (*g, &mut **i));
                    self.services(&s.services, &inner, &guard.and(&s.guard), n, reborrow);
                }
                Svc::Files { prefix: p } => {
                    let inner = self.scope_prefix(prefix, p);
                    let text = format!(
                        "an actix-files service at {} serves files under its prefix",
                        self.at(p.loc())
                    );
                    self.prefix_gap_methods(
                        "framework-provided-routes:",
                        text,
                        &inner,
                        &["GET", "HEAD"],
                    );
                }
                Svc::Unknown { loc, what } => {
                    let text = format!("{what} at {} may register routes", self.at(loc));
                    self.prefix_gap("route-coverage:", text, prefix);
                }
            }
        }
    }

    /// 매크로 핸들러의 등록 목록.
    fn macro_regs_of(&self, id: &str) -> Vec<MacroReg> {
        self.macros.get(id).cloned().unwrap_or_default()
    }

    /// 스코프 접두사를 잇는다.
    fn scope_prefix(&self, prefix: &JPath, p: &PathArg) -> JPath {
        let raw = JPath::of(p);
        let raw = match raw {
            JPath::Lit(s) if !s.is_empty() && !s.starts_with('/') => JPath::Lit(format!("/{s}")),
            other => other,
        };
        prefix.join(&raw, |a, b| format!("{a}{b}"), p.loc())
    }

    /// 접두사 아래 전부를 덮는 한계.
    fn prefix_gap(&mut self, prefix: &'static str, text: String, path: &JPath) {
        self.prefix_gap_methods(prefix, text, path, &[]);
    }

    fn prefix_gap_methods(
        &mut self,
        prefix: &'static str,
        text: String,
        path: &JPath,
        methods: &[&str],
    ) {
        if let JPath::Lit(raw) = path {
            let raw = if raw.is_empty() { "/" } else { raw.as_str() };
            if let Ok(p) = parse_actix(raw) {
                if let Some(t) = prefix_template(&p.segs) {
                    if t != "/" || !methods.is_empty() {
                        self.out.scoped_gap(
                            prefix,
                            text,
                            ScopeSpec {
                                prefixes: vec![t],
                                methods: methods.iter().map(|m| m.to_string()).collect(),
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

    /// 리소스 하나(경로 여러 개 × 라우트들)를 선언으로 낸다.
    #[allow(clippy::too_many_arguments)]
    fn resource_decls(
        &mut self,
        paths: &[PathArg],
        loc: &Loc,
        guard: &Guard,
        routes: &[RouteVal],
        prefix: &JPath,
        normalize: Normalize,
        order: Option<(String, u64)>,
        fixed_handler: Option<Handler>,
    ) {
        for p in paths {
            let joined = prefix.join(&JPath::of(p), actix_join, p.loc());
            let (path, anchor) = match &joined {
                JPath::Lit(raw) if raw.is_empty() => continue,
                JPath::Lit(raw) | JPath::Base(raw) => {
                    let base = matches!(joined, JPath::Base(_));
                    let raw = if raw.is_empty() {
                        "/".to_string()
                    } else {
                        raw.clone()
                    };
                    match parse_actix(&raw) {
                        Ok(parsed) => (
                            DeclPath::Template {
                                segs: parsed.segs,
                                empty_tail: parsed
                                    .empty_tail
                                    .then(|| empty_tail_trailing(normalize))
                                    .flatten(),
                            },
                            if base { Anchor::Base } else { Anchor::Root },
                        ),
                        Err(why) => {
                            self.out.gap(
                                "route-coverage:",
                                format!(
                                    "route pattern {raw:?} at {} {why}; the declaration is dynamic",
                                    self.at(loc)
                                ),
                            );
                            (DeclPath::Dynamic(raw.clone()), Anchor::Root)
                        }
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
                        DeclPath::Dynamic(self.ctx.source_text(&dl.file, dl.span)),
                        Anchor::Root,
                    )
                }
            };
            if anchor == Anchor::Base {
                self.bases.push(match &path {
                    DeclPath::Template { segs, .. } => base_suffix(segs),
                    DeclPath::Dynamic(_) => None,
                });
            }
            let trailing = trailing_of(&path, normalize);
            for r in routes {
                let g = guard.and(&r.guard);
                let methods: Vec<String> = match &g.methods {
                    None => vec!["ANY".to_string()],
                    Some(ms) => {
                        if ms.contains("CONNECT") {
                            self.out.gap(
                                "route-coverage:",
                                format!(
                                    "a CONNECT route at {} has no http-domain method",
                                    self.at(loc)
                                ),
                            );
                        }
                        ms.iter().filter(|m| *m != "CONNECT").cloned().collect()
                    }
                };
                if methods.is_empty() {
                    continue;
                }
                let handler = match &fixed_handler {
                    Some(h) => h.clone(),
                    None => r.handler.resolve(self.ctx),
                };
                self.out.decls.push(Decl {
                    methods,
                    path: path.clone(),
                    anchor,
                    trailing,
                    handler,
                    loc: loc.clone(),
                    order: if anchor == Anchor::Root {
                        order.clone()
                    } else {
                        None
                    },
                    narrowed: g.narrowed,
                });
            }
        }
    }

    /// base 선언 공백 — `unresolved-route-prefix:`와 순서 미상.
    fn finish_bases(&mut self) {
        if self.bases.is_empty() {
            return;
        }
        let n = self.bases.len();
        let scope = self.bases.iter().all(Option::is_some).then(|| {
            let mut suffixes: Vec<String> = self.bases.iter().flatten().cloned().collect();
            suffixes.sort();
            suffixes.dedup();
            ScopeSpec {
                suffixes,
                ..Default::default()
            }
        });
        let texts = [
            (
                "unresolved-route-prefix:",
                format!("{n} route declaration(s) are not registered on an App the analyzer could follow, or sit under a non-literal scope prefix; they are emitted with pathAnchor base"),
            ),
            (
                "route-dispatch-order-unknown:",
                format!("{n} base-anchored route declaration(s) carry no registration order"),
            ),
        ];
        for (prefix, text) in texts {
            match &scope {
                Some(s) => self.out.scoped_gap(prefix, text, s.clone()),
                None => self.out.gap(prefix, text),
            }
        }
    }
}

/// base 선언을 덮는 접미사 스코프 원소.
fn base_suffix(segs: &[Seg]) -> Option<String> {
    if segs.iter().any(|s| matches!(s, Seg::CatchAll)) {
        return None;
    }
    let t = render(segs);
    (t != "/").then_some(t)
}

/// 빈 끝 변형(`/files/`)의 끝 슬래시 판정. Trim은 그 요청을 `/files`로 바꿔 꼬리
/// 파라미터에 닿지 않게 하므로 변형이 없고, Always는 `/files`도 `/files/`로 바꿔 닿게 한다.
fn empty_tail_trailing(n: Normalize) -> Option<Trailing> {
    match n {
        Normalize::None | Normalize::MergeOnly => Some(Trailing::Strict),
        Normalize::Trim => None,
        Normalize::Always => Some(Trailing::Optional),
        Normalize::Unknown => Some(Trailing::Unknown),
    }
}

/// 끝 슬래시 판정 — 정규화 미들웨어가 없으면 엄격(resource.rs 문서 196-208).
fn trailing_of(p: &DeclPath, n: Normalize) -> Trailing {
    let DeclPath::Template { segs, .. } = p else {
        return Trailing::Unknown;
    };
    if matches!(segs.last(), Some(Seg::CatchAll)) {
        return Trailing::Unknown;
    }
    let t = render(segs);
    let ends_slash = t.len() > 1 && t.ends_with('/');
    match n {
        Normalize::None | Normalize::MergeOnly => Trailing::Strict,
        Normalize::Trim if t == "/" => Trailing::Strict,
        Normalize::Trim if !ends_slash => Trailing::Optional,
        Normalize::Always if ends_slash => Trailing::Optional,
        Normalize::Trim | Normalize::Always => Trailing::Strict,
        Normalize::Unknown => Trailing::Unknown,
    }
}
