//! 함수 본문 스캐너 — 문자열·URL 값과 수신자 타입을 따라가며 HTTP 요청 호출을 찾는다.
//!
//! 두 번 돈다. `Collect`는 구조체 리터럴의 필드 값(생성자에서 base URL을 리터럴·
//! 상수로 채우는지)을 모으고, `Emit`은 그 표로 `self.base_url` 같은 필드를 풀어
//! 호출 사실을 낸다. 해석은 구문 수준이다 — 증명하지 못한 값은 [`Origin`]을 단
//! 값 조각으로 남고, 조립 규칙(`compose`)이 dynamic으로 판정한다.

use super::super::common::{is_test_item, Ctx, Imports, Loc};
use super::super::compose::{Origin, Outcome, PathAnchor, Piece, UrlVal};
use super::super::wrappers::{self, ArgValue, CallArg, Wrapper, WrapperKind};
use super::index::{module_groups, workspace_resolve, Index};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::rc::Rc;
use syn::spanned::Spanned;
use syn::visit::Visit;

/// 계약 동사.
const VERBS: &[&str] = wrappers::VERBS;

/// RequestBuilder에서 요청을 보내는 메서드 — 수신자를 증명하지 못한 요청 모양 호출을 센다.
const SEND_METHODS: &[&str] = &[
    "send",
    "call",
    "send_string",
    "send_json",
    "send_form",
    "send_bytes",
    "send_empty",
];

/// 요청 빌더를 이어 가는 메서드 — 보내기 호출에서 동사 호출까지 거슬러 오를 때 건넌다.
const BUILDER_METHODS: &[&str] = &[
    "header",
    "headers",
    "query",
    "json",
    "form",
    "body",
    "timeout",
    "basic_auth",
    "bearer_auth",
    "multipart",
    "version",
    "set",
    "config",
    "content_type",
];

/// 값을 제자리에서 바꾸는 메서드 — 그 지역 변수·필드의 값은 믿지 않는다.
const MUTATORS: &[&str] = &[
    "push_str",
    "push",
    "insert",
    "insert_str",
    "clear",
    "truncate",
    "extend",
    "retain",
    "drain",
    "remove",
    "replace_range",
    "set_path",
    "set_query",
    "set_fragment",
    "set_host",
    "set_port",
    "set_scheme",
    "set_username",
    "set_password",
    "path_segments_mut",
    "query_pairs_mut",
    "make_ascii_lowercase",
    "make_ascii_uppercase",
    "as_mut_str",
];

/// 값을 그대로 넘기는 메서드(문자열·URL·Result).
const PASS_THROUGH: &[&str] = &[
    "to_string",
    "to_owned",
    "into",
    "as_str",
    "as_ref",
    "clone",
    "borrow",
    "as_deref",
    "into_owned",
    "unwrap",
    "expect",
];

/// 스캔 단계.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Collect,
    Emit,
}

/// 크레이트 하나가 직접 의존하는 HTTP 클라이언트 라이브러리(lib 이름).
#[derive(Clone, Debug, Default)]
pub(super) struct Libs {
    pub reqwest: Option<String>,
    /// (lib 이름, 메이저 버전) — 2.x는 `url::Url`, 3.x는 `http::Uri`로 해석한다.
    pub ureq: Option<(String, u64)>,
    /// 모델링하지 않는 클라이언트 크레이트의 lib 이름.
    pub unmodelled: Vec<String>,
}

/// 요청 라이브러리 종류.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Client {
    Reqwest,
    Ureq,
}

/// 식을 평가한 값.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Val {
    /// 문자열 조각과 앞머리의 `baseRef` 후보(필드·상수 id).
    Str(Vec<Piece>, Option<String>),
    /// `url::Url` 값.
    Url(UrlVal, Option<String>),
    /// 문자열·URL로 모르는 값.
    Other(Origin),
}

impl Val {
    /// 문자열 문맥의 조각.
    fn pieces(&self) -> Vec<Piece> {
        match self {
            Val::Str(p, _) => p.clone(),
            Val::Url(u, _) => u.to_pieces(),
            Val::Other(o) => vec![Piece::Value(o.clone())],
        }
    }

    /// 앞머리 값의 `baseRef`.
    fn base_ref(&self) -> Option<String> {
        match self {
            Val::Str(_, r) | Val::Url(_, r) => r.clone(),
            Val::Other(Origin::Base(id)) => Some(id.clone()),
            Val::Other(_) => None,
        }
    }

    /// `baseRef`를 바꾼 값.
    fn with_ref(self, r: Option<String>) -> Val {
        match self {
            Val::Str(p, _) => Val::Str(p, r),
            Val::Url(u, _) => Val::Url(u, r),
            other => other,
        }
    }

    fn unknown() -> Val {
        Val::Other(Origin::Unknown)
    }
}

/// 찾은 호출 하나.
#[derive(Clone, Debug)]
pub(super) struct CallSite {
    pub loc: Loc,
    /// None이면 `methodDynamic`.
    pub method: Option<String>,
    pub outcome: Outcome,
    pub base_ref: Option<String>,
    pub service: Option<String>,
}

/// 스캔 결과와 계수.
#[derive(Default)]
pub(super) struct Collected {
    pub calls: Vec<CallSite>,
    /// 수신자를 reqwest·ureq 클라이언트로 증명하지 못한 요청 모양 호출.
    pub unproven: usize,
    /// base 없는 상대 URL이라 요청이 되지 않는 호출.
    pub unrequestable: usize,
    /// 모델링하지 않는 클라이언트 lib 이름 → 사용 위치 수.
    pub unmodelled: BTreeMap<String, usize>,
    /// 선언 위치 → 찾은 호출 수.
    pub wrapper_calls: BTreeMap<usize, usize>,
    /// 매개변수를 URL·동사로 흘려보내는 선언되지 않은 래퍼 싱크.
    pub undeclared: usize,
}

/// 필드 값 누적 — 모든 생성 위치가 같은 값이어야 확정이다.
#[derive(Default)]
struct FieldAcc {
    value: Option<Val>,
    conflict: bool,
}

/// 스캔 전체가 공유하는 상태.
pub(super) struct Shared<'a> {
    pub ctx: &'a Ctx<'a>,
    pub index: &'a Index,
    pub wrappers: &'a [Wrapper],
    /// 함수·연관 함수 래퍼: 정점 ID → 선언 순번.
    fn_wrappers: BTreeMap<String, usize>,
    /// 구조체 리터럴·튜플 생성 래퍼: 구조체 ID → 선언 순번.
    ctor_wrappers: BTreeMap<String, usize>,
    fields: RefCell<BTreeMap<(String, String), FieldAcc>>,
    /// 확정한 필드 값(`Emit` 단계).
    resolved: RefCell<BTreeMap<(String, String), Val>>,
    /// 어디선가 제자리 수정되는 필드 — (소유 구조체, 이름). 소유 타입을 모르면 None이라
    /// 그 이름의 모든 필드를 믿지 않는다.
    poisoned: RefCell<BTreeSet<(Option<String>, String)>>,
    consts: RefCell<BTreeMap<String, Option<Val>>>,
    pub out: RefCell<Collected>,
}

impl<'a> Shared<'a> {
    /// 선언 순번 표를 만든다. `rust` 선언 중 정점에 닿는 것만 매칭 대상이다.
    pub fn new(ctx: &'a Ctx<'a>, index: &'a Index, wrappers: &'a [Wrapper]) -> Shared<'a> {
        let mut fn_wrappers = BTreeMap::new();
        let mut ctor_wrappers = BTreeMap::new();
        for (i, w) in wrappers.iter().enumerate() {
            let type_name = w.owner.rsplit("::").next().unwrap_or(&w.owner);
            if w.kind == WrapperKind::Constructor && w.name == type_name {
                if index.structs.contains_key(&w.owner) {
                    ctor_wrappers.insert(w.owner.clone(), i);
                }
                continue;
            }
            let id = format!("{}::{}", w.owner, w.name);
            if ctx.ids.contains(id.as_str()) {
                fn_wrappers.insert(id, i);
            }
        }
        let mut fields: BTreeMap<(String, String), FieldAcc> = BTreeMap::new();
        for (id, s) in &index.structs {
            if s.derives_default {
                for f in s.fields.keys() {
                    fields.entry((id.clone(), f.clone())).or_default().conflict = true;
                }
            }
        }
        Shared {
            ctx,
            index,
            wrappers,
            fn_wrappers,
            ctor_wrappers,
            fields: RefCell::new(fields),
            resolved: RefCell::new(BTreeMap::new()),
            poisoned: RefCell::new(BTreeSet::new()),
            consts: RefCell::new(BTreeMap::new()),
            out: RefCell::new(Collected::default()),
        }
    }

    /// 선언이 정점·구조체에 닿았는가.
    pub fn wrapper_resolved(&self, i: usize) -> bool {
        self.fn_wrappers.values().any(|&j| j == i) || self.ctor_wrappers.values().any(|&j| j == i)
    }

    /// `Collect` 결과를 확정 표로 바꾼다.
    pub fn finish_collect(&self) {
        let poisoned = self.poisoned.borrow();
        let mut resolved = self.resolved.borrow_mut();
        for ((s, f), acc) in self.fields.borrow().iter() {
            let hit = poisoned.contains(&(None, f.clone()))
                || poisoned.contains(&(Some(s.clone()), f.clone()));
            if acc.conflict || hit {
                continue;
            }
            if let Some(v) = &acc.value {
                if is_constant(v) {
                    resolved.insert((s.clone(), f.clone()), v.clone());
                }
            }
        }
    }

    /// 필드 값 하나를 누적한다(생성 위치마다).
    fn add_field(&self, owner: &str, field: &str, v: Val) {
        let v = v.with_ref(None);
        let mut fields = self.fields.borrow_mut();
        let acc = fields
            .entry((owner.to_string(), field.to_string()))
            .or_default();
        match &acc.value {
            None => acc.value = Some(v),
            Some(prev) if *prev == v => {}
            Some(_) => acc.conflict = true,
        }
    }
}

/// 크레이트 하나를 스캔한다.
pub(super) fn scan_crate(sh: &Shared, krate: &str, libs: &Libs, mode: Mode) {
    for (module, file, items) in module_groups(sh.ctx, krate) {
        let imports = sh.index.imports(sh.ctx, &module);
        let mut s = Scanner {
            sh,
            mode,
            module,
            file,
            imports,
            libs,
            self_ty: None,
            env: Env::default(),
            mutated: BTreeSet::new(),
            in_wrapper: false,
        };
        for item in items {
            s.scan_item(item);
        }
    }
}

/// 이름 하나의 바인딩.
#[derive(Clone, Debug)]
struct Binding {
    val: Val,
    ty: Option<String>,
}

/// 어휘 스코프 스택.
#[derive(Default)]
struct Env {
    scopes: Vec<BTreeMap<String, Binding>>,
}

impl Env {
    fn push(&mut self) {
        self.scopes.push(BTreeMap::new());
    }

    fn pop(&mut self) {
        self.scopes.pop();
    }

    fn bind(&mut self, name: String, b: Binding) {
        if self.scopes.is_empty() {
            self.push();
        }
        if let Some(top) = self.scopes.last_mut() {
            top.insert(name, b);
        }
    }

    fn get(&self, name: &str) -> Option<&Binding> {
        self.scopes.iter().rev().find_map(|s| s.get(name))
    }
}

/// 모듈 파일 하나를 훑는 방문자.
struct Scanner<'s, 'a> {
    sh: &'s Shared<'a>,
    mode: Mode,
    module: String,
    file: PathBuf,
    imports: Rc<Imports>,
    libs: &'s Libs,
    self_ty: Option<String>,
    env: Env,
    /// 지금 함수에서 제자리 수정되는 지역 변수 이름.
    mutated: BTreeSet<String>,
    /// 지금 함수가 선언된 래퍼 본문인가 — 그 안의 dynamic 호출은 래퍼 호출 사실이 대신한다.
    in_wrapper: bool,
}

impl Scanner<'_, '_> {
    /// 모듈 수준 아이템 하나.
    fn scan_item(&mut self, item: &syn::Item) {
        match item {
            syn::Item::Fn(f) if !is_test_item(&f.attrs) => {
                self.scan_fn(None, &f.sig, &f.block);
            }
            syn::Item::Impl(i) if !is_test_item(&i.attrs) => self.scan_impl(i),
            syn::Item::Trait(t) if !is_test_item(&t.attrs) => {
                for ti in &t.items {
                    if let syn::TraitItem::Fn(f) = ti {
                        if let Some(body) = &f.default {
                            self.scan_fn(None, &f.sig, body);
                        }
                    }
                }
            }
            syn::Item::Const(c) => self.scan_detached(&c.expr),
            syn::Item::Static(s) => self.scan_detached(&s.expr),
            _ => {}
        }
    }

    /// impl 블록의 메서드들.
    fn scan_impl(&mut self, i: &syn::ItemImpl) {
        let self_ty = self
            .sh
            .index
            .resolve_type(self.sh.ctx, &self.module, None, &i.self_ty);
        for item in &i.items {
            if let syn::ImplItem::Fn(f) = item {
                if !is_test_item(&f.attrs) {
                    self.scan_fn(self_ty.clone(), &f.sig, &f.block);
                }
            }
        }
    }

    /// 함수 밖의 식(상수·static 초기식) — 빈 환경으로 본다.
    fn scan_detached(&mut self, e: &syn::Expr) {
        let saved = (
            std::mem::take(&mut self.env),
            self.self_ty.take(),
            self.in_wrapper,
        );
        self.in_wrapper = false;
        self.env.push();
        self.visit_expr(e);
        (self.env, self.self_ty, self.in_wrapper) = saved;
    }

    /// 함수 하나 — 매개변수를 바인딩하고 본문을 방문한다. 중첩 함수도 여기로 온다.
    fn scan_fn(&mut self, self_ty: Option<String>, sig: &syn::Signature, block: &syn::Block) {
        let saved_env = std::mem::take(&mut self.env);
        let saved_ty = std::mem::replace(&mut self.self_ty, self_ty);
        let saved_mut = std::mem::replace(&mut self.mutated, mutations(block));
        let saved_wrapper = self.in_wrapper;
        let own = Loc {
            file: self.file.clone(),
            span: sig.ident.span(),
        };
        self.in_wrapper = self
            .sh
            .ctx
            .owner_of(&own)
            .is_some_and(|id| self.sh.fn_wrappers.contains_key(&id));
        self.env.push();
        for input in &sig.inputs {
            if let syn::FnArg::Typed(t) = input {
                let ty = self.resolve_type(&t.ty);
                for name in pat_names(&t.pat) {
                    let b = Binding {
                        val: Val::Other(Origin::Param(name.clone())),
                        ty: ty.clone(),
                    };
                    self.env.bind(name, b);
                }
            }
        }
        self.visit_block(block);
        self.env = saved_env;
        self.self_ty = saved_ty;
        self.mutated = saved_mut;
        self.in_wrapper = saved_wrapper;
    }

    fn resolve_type(&self, ty: &syn::Type) -> Option<String> {
        self.sh
            .index
            .resolve_type(self.sh.ctx, &self.module, self.self_ty.as_deref(), ty)
    }

    fn resolve_path(&self, segs: &[String]) -> Option<String> {
        self.sh
            .index
            .resolve_path(self.sh.ctx, &self.module, self.self_ty.as_deref(), segs)
    }

    /// 경로를 `use`로 펼친다(외부 API 판정).
    fn expand(&self, segs: &[String]) -> Vec<String> {
        self.imports.expand(segs)
    }

    /// 경로가 워크스페이스 아이템을 가리키는가 — 같은 이름의 외부 API로 읽지 않기 위해서다.
    fn is_workspace(&self, segs: &[String]) -> bool {
        let dep = crate::modtree::DepCrates::new();
        segs.first().is_some_and(|f| f == "Self")
            || (1..=segs.len()).any(|cut| {
                workspace_resolve(self.sh.ctx, self.sh.index, &self.module, &segs[..cut], &dep)
                    .is_some()
            })
    }

    // ── 타입 ─────────────────────────────────────────────

    /// 식의 타입(경로 문자열) — 수신자 판정에 필요한 만큼만 추론한다.
    fn ty(&self, e: &syn::Expr) -> Option<String> {
        match e {
            syn::Expr::Path(p) => {
                let segs = crate::harvest::path_segments(&p.path);
                if let [one] = segs.as_slice() {
                    if one == "self" {
                        return self.self_ty.clone();
                    }
                    if let Some(b) = self.env.get(one) {
                        return b.ty.clone();
                    }
                }
                let id = self.resolve_path(&segs)?;
                let c = self.sh.index.consts.get(&id)?;
                self.sh
                    .index
                    .resolve_type(self.sh.ctx, &c.module, c.self_ty.as_deref(), c.ty)
            }
            syn::Expr::Field(f) => {
                let owner = self.ty(&f.base)?;
                let syn::Member::Named(name) = &f.member else {
                    return None;
                };
                self.sh
                    .index
                    .field_type(self.sh.ctx, &owner, &name.to_string())
            }
            syn::Expr::Reference(r) => self.ty(&r.expr),
            syn::Expr::Paren(p) => self.ty(&p.expr),
            syn::Expr::Group(g) => self.ty(&g.expr),
            syn::Expr::Unary(u) if matches!(u.op, syn::UnOp::Deref(_)) => self.ty(&u.expr),
            syn::Expr::Try(t) => self.ty(&t.expr),
            syn::Expr::Await(a) => self.ty(&a.base),
            syn::Expr::Struct(s) => self.resolve_path(&crate::harvest::path_segments(&s.path)),
            syn::Expr::Call(c) => self.call_ty(c),
            syn::Expr::MethodCall(m) => self.method_ty(m),
            _ => None,
        }
    }

    /// 호출식의 타입 — 알려진 생성자와 워크스페이스 함수의 반환 타입.
    fn call_ty(&self, c: &syn::ExprCall) -> Option<String> {
        let syn::Expr::Path(p) = &*c.func else {
            return None;
        };
        let segs = crate::harvest::path_segments(&p.path);
        if !self.is_workspace(&segs) {
            let full = self.expand(&segs);
            if let Some(t) = self.library_ctor(&full) {
                return Some(t);
            }
            if matches!(full.as_slice(), [.., w, n] if n == "new" && matches!(w.as_str(), "Arc" | "Rc" | "Box"))
            {
                return c.args.first().and_then(|a| self.ty(a));
            }
        }
        let id = self.resolve_path(&segs)?;
        if let Some(f) = self.sh.index.fns.get(&id) {
            return self.sh.index.return_type(self.sh.ctx, f);
        }
        let (owner, name) = id.rsplit_once("::")?;
        if let Some(fs) = self
            .sh
            .index
            .methods
            .get(&(owner.to_string(), name.to_string()))
        {
            return fs
                .iter()
                .find_map(|f| self.sh.index.return_type(self.sh.ctx, f));
        }
        // 외부 타입의 `T::new()`·`T::default()`는 T다.
        if matches!(name, "new" | "default") && !self.is_workspace(&segs) {
            return Some(owner.to_string());
        }
        None
    }

    /// 라이브러리 생성자 경로의 타입(reqwest·ureq).
    fn library_ctor(&self, full: &[String]) -> Option<String> {
        let parts: Vec<&str> = full.iter().map(String::as_str).collect();
        if let Some(lib) = &self.libs.reqwest {
            let tail = match parts.split_first() {
                Some((first, tail)) if first == lib => tail,
                _ => &[][..],
            };
            let t = match tail {
                ["Client", "new" | "default"] => Some("Client"),
                ["blocking", "Client", "new" | "default"] => Some("blocking::Client"),
                ["Client", "builder"] | ["ClientBuilder", "new"] => Some("ClientBuilder"),
                ["blocking", "Client", "builder"] | ["blocking", "ClientBuilder", "new"] => {
                    Some("blocking::ClientBuilder")
                }
                _ => None,
            };
            if let Some(t) = t {
                return Some(format!("{lib}::{t}"));
            }
        }
        if let Some((lib, _)) = &self.libs.ureq {
            let tail = match parts.split_first() {
                Some((first, tail)) if first == lib => tail,
                _ => &[][..],
            };
            let t = match tail {
                ["agent"] | ["Agent", "new" | "new_with_defaults" | "new_with_config"] => {
                    Some("Agent")
                }
                ["builder"] | ["AgentBuilder", "new"] => Some("AgentBuilder"),
                ["Agent", "config_builder"] | ["config", "Config", "builder"] => {
                    Some("ConfigBuilder")
                }
                _ => None,
            };
            if let Some(t) = t {
                return Some(format!("{lib}::{t}"));
            }
        }
        None
    }

    /// 메서드 호출식의 타입.
    fn method_ty(&self, m: &syn::ExprMethodCall) -> Option<String> {
        let name = m.method.to_string();
        let recv = self.ty(&m.receiver);
        let pass = PASS_THROUGH.contains(&name.as_str())
            || matches!(
                name.as_str(),
                "get_or_init" | "get_or_try_init" | "lock" | "read"
            )
            || (name == "get" && m.args.is_empty());
        if pass {
            return recv;
        }
        let recv = recv?;
        if let Some(t) = self.builder_step(&recv, &name) {
            return Some(t);
        }
        self.sh
            .index
            .methods
            .get(&(recv, name))?
            .iter()
            .find_map(|f| self.sh.index.return_type(self.sh.ctx, f))
    }

    /// 클라이언트 빌더 체인의 다음 타입.
    fn builder_step(&self, recv: &str, method: &str) -> Option<String> {
        if let Some(lib) = &self.libs.reqwest {
            for (builder, client) in [
                ("ClientBuilder", "Client"),
                ("blocking::ClientBuilder", "blocking::Client"),
            ] {
                if recv == format!("{lib}::{builder}") {
                    return Some(if method == "build" {
                        format!("{lib}::{client}")
                    } else {
                        recv.to_string()
                    });
                }
            }
        }
        if let Some((lib, _)) = &self.libs.ureq {
            if recv == format!("{lib}::AgentBuilder") {
                return Some(if method == "build" {
                    format!("{lib}::Agent")
                } else {
                    recv.to_string()
                });
            }
            if recv == format!("{lib}::ConfigBuilder") {
                return Some(if method == "build" {
                    format!("{lib}::Config")
                } else {
                    recv.to_string()
                });
            }
            if recv == format!("{lib}::Config") && method == "new_agent" {
                return Some(format!("{lib}::Agent"));
            }
        }
        None
    }

    /// 타입이 요청 클라이언트인가.
    fn client_of(&self, ty: Option<&str>) -> Option<Client> {
        let ty = ty?;
        if let Some(lib) = &self.libs.reqwest {
            if ty == format!("{lib}::Client") || ty == format!("{lib}::blocking::Client") {
                return Some(Client::Reqwest);
            }
        }
        if let Some((lib, _)) = &self.libs.ureq {
            if ty == format!("{lib}::Agent") {
                return Some(Client::Ureq);
            }
        }
        None
    }

    // ── 값 ───────────────────────────────────────────────

    /// 식의 문자열·URL 값.
    fn eval(&self, e: &syn::Expr) -> Val {
        self.eval_depth(e, 0)
    }

    fn eval_depth(&self, e: &syn::Expr, depth: usize) -> Val {
        if depth > 32 {
            return Val::unknown();
        }
        let d = depth + 1;
        match e {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) => Val::Str(vec![Piece::Lit(s.value())], None),
            syn::Expr::Path(p) => self.eval_path(&crate::harvest::path_segments(&p.path), d),
            syn::Expr::Field(f) => self.eval_field(f),
            syn::Expr::Macro(m) => self.eval_macro(&m.mac, d),
            syn::Expr::Binary(b) if matches!(b.op, syn::BinOp::Add(_)) => {
                concat(self.eval_depth(&b.left, d), self.eval_depth(&b.right, d))
            }
            syn::Expr::Reference(r) => self.eval_depth(&r.expr, d),
            syn::Expr::Paren(p) => self.eval_depth(&p.expr, d),
            syn::Expr::Group(g) => self.eval_depth(&g.expr, d),
            syn::Expr::Unary(u) if matches!(u.op, syn::UnOp::Deref(_)) => {
                self.eval_depth(&u.expr, d)
            }
            syn::Expr::Try(t) => self.eval_depth(&t.expr, d),
            syn::Expr::MethodCall(m) => self.eval_method(m, d),
            syn::Expr::Call(c) => self.eval_call(c, d),
            _ => Val::unknown(),
        }
    }

    /// 경로 값 — 지역 변수, 문자열 상수·static. `depth`는 상수가 상수를 가리키는 사슬에도
    /// 이어져 깊은 사슬이 스택을 넘기지 않게 한다.
    fn eval_path(&self, segs: &[String], depth: usize) -> Val {
        if let [one] = segs {
            if let Some(b) = self.env.get(one) {
                return b.val.clone();
            }
        }
        match self.resolve_path(segs) {
            Some(id) if self.sh.index.consts.contains_key(&id) => self.eval_const(&id, depth),
            _ => Val::unknown(),
        }
    }

    /// 상수·static의 값(순환 방지 캐시). 깊이 상한에 닿은 사슬은 모르는 값(`baseRef`)이다.
    fn eval_const(&self, id: &str, depth: usize) -> Val {
        if let Some(cached) = self.sh.consts.borrow().get(id) {
            return cached
                .clone()
                .unwrap_or_else(|| Val::Other(Origin::Base(id.to_string())));
        }
        self.sh.consts.borrow_mut().insert(id.to_string(), None);
        let Some(c) = self.sh.index.consts.get(id) else {
            return Val::unknown();
        };
        let scanner = Scanner {
            sh: self.sh,
            mode: self.mode,
            module: c.module.clone(),
            file: self.file.clone(),
            imports: self.sh.index.imports(self.sh.ctx, &c.module),
            libs: self.libs,
            self_ty: c.self_ty.clone(),
            env: Env::default(),
            mutated: BTreeSet::new(),
            in_wrapper: false,
        };
        let v = scanner.eval_depth(c.expr, depth);
        let known = match v {
            Val::Str(..) | Val::Url(..) => Some(v.with_ref(Some(id.to_string()))),
            Val::Other(_) => None,
        };
        self.sh
            .consts
            .borrow_mut()
            .insert(id.to_string(), known.clone());
        known.unwrap_or_else(|| Val::Other(Origin::Base(id.to_string())))
    }

    /// 필드 값 — 생성자들이 모두 같은 리터럴·상수로 채운 필드만 값이 있다.
    fn eval_field(&self, f: &syn::ExprField) -> Val {
        let syn::Member::Named(name) = &f.member else {
            return Val::unknown();
        };
        let Some(owner) = self.ty(&f.base) else {
            return Val::unknown();
        };
        if !self.sh.index.structs.contains_key(&owner) {
            return Val::unknown();
        }
        let name = name.to_string();
        let id = format!("{owner}::{name}");
        match self.sh.resolved.borrow().get(&(owner, name)) {
            Some(v) => v.clone().with_ref(Some(id)),
            None => Val::Other(Origin::Base(id)),
        }
    }

    /// 메서드 호출 값 — 문자열 변환, `Url::join`.
    fn eval_method(&self, m: &syn::ExprMethodCall, d: usize) -> Val {
        let name = m.method.to_string();
        if PASS_THROUGH.contains(&name.as_str()) {
            return self.eval_depth(&m.receiver, d);
        }
        if name == "join" && m.args.len() == 1 {
            if let Val::Url(u, r) = self.eval_depth(&m.receiver, d) {
                let arg = self.eval_depth(&m.args[0], d);
                return Val::Url(u.join(&arg.pieces()), r);
            }
        }
        Val::unknown()
    }

    /// 호출 값 — `Url::parse`, `String::from`, `String::new`.
    fn eval_call(&self, c: &syn::ExprCall, d: usize) -> Val {
        let syn::Expr::Path(p) = &*c.func else {
            return Val::unknown();
        };
        let segs = crate::harvest::path_segments(&p.path);
        if self.is_workspace(&segs) {
            return Val::unknown();
        }
        let full = self.expand(&segs);
        let parts: Vec<&str> = full.iter().map(String::as_str).collect();
        let url_lib = |l: &str| l == "url" || self.libs.reqwest.as_deref() == Some(l);
        match parts.as_slice() {
            [lib, "Url", "parse"] if url_lib(lib) && c.args.len() == 1 => {
                let arg = self.eval_depth(&c.args[0], d);
                let r = arg.base_ref();
                Val::Url(UrlVal::parse(&arg.pieces(), true), r)
            }
            ["String", "from"] | ["std", "string", "String", "from"] if c.args.len() == 1 => {
                self.eval_depth(&c.args[0], d)
            }
            ["String", "new"] | ["String", "default"] => Val::Str(Vec::new(), None),
            _ => Val::unknown(),
        }
    }

    /// 매크로 값 — `format!` 계열과 `concat!`.
    fn eval_macro(&self, mac: &syn::Macro, d: usize) -> Val {
        let name = mac
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default();
        let args = mac.parse_body_with(
            syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated,
        );
        let Ok(args) = args else {
            return Val::unknown();
        };
        let args: Vec<syn::Expr> = args.into_iter().collect();
        match name.as_str() {
            "format" | "format_args" => self.eval_format(&args, d),
            "concat" => {
                let mut pieces = Vec::new();
                for a in &args {
                    match a {
                        syn::Expr::Lit(l) => match &l.lit {
                            syn::Lit::Str(s) => pieces.push(Piece::Lit(s.value())),
                            syn::Lit::Int(i) => pieces.push(Piece::Lit(i.base10_digits().into())),
                            syn::Lit::Char(c) => pieces.push(Piece::Lit(c.value().to_string())),
                            syn::Lit::Bool(b) => pieces.push(Piece::Lit(b.value.to_string())),
                            _ => pieces.push(Piece::Value(Origin::Unknown)),
                        },
                        _ => pieces.push(Piece::Value(Origin::Unknown)),
                    }
                }
                Val::Str(pieces, None)
            }
            _ => Val::unknown(),
        }
    }

    /// `format!("..", args)` — Display 자리표시자만 값을 잇고, 서식 지정이 있으면 모르는
    /// 값이다(`{:?}`는 따옴표를 붙인다).
    fn eval_format(&self, args: &[syn::Expr], d: usize) -> Val {
        let Some((fmt, rest)) = args.split_first() else {
            return Val::unknown();
        };
        let syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(fmt),
            ..
        }) = fmt
        else {
            return Val::unknown();
        };
        let mut positional: Vec<&syn::Expr> = Vec::new();
        let mut named: BTreeMap<String, &syn::Expr> = BTreeMap::new();
        for a in rest {
            match a {
                syn::Expr::Assign(asg) => {
                    if let syn::Expr::Path(p) = &*asg.left {
                        if let Some(id) = p.path.get_ident() {
                            named.insert(id.to_string(), &asg.right);
                            continue;
                        }
                    }
                    return Val::unknown();
                }
                other => positional.push(other),
            }
        }
        let Some(segments) = parse_format(&fmt.value()) else {
            return Val::unknown();
        };
        let mut pieces = Vec::new();
        let mut next = 0usize;
        let mut lead_ref = None;
        for (i, seg) in segments.into_iter().enumerate() {
            match seg {
                FmtSeg::Lit(t) => pieces.push(Piece::Lit(t)),
                FmtSeg::Arg { key, spec } => {
                    let v = match key {
                        FmtKey::Next => {
                            next += 1;
                            positional.get(next - 1).map(|e| self.eval_depth(e, d))
                        }
                        FmtKey::Index(n) => positional.get(n).map(|e| self.eval_depth(e, d)),
                        FmtKey::Name(n) => match named.get(&n) {
                            Some(e) => Some(self.eval_depth(e, d)),
                            None => Some(self.eval_path(&[n], d)),
                        },
                    };
                    let Some(v) = v else {
                        return Val::unknown();
                    };
                    if i == 0 {
                        lead_ref = v.base_ref();
                    }
                    if spec.is_empty() {
                        pieces.extend(v.pieces());
                    } else {
                        let origin = match v {
                            Val::Other(o) => o,
                            _ => Origin::Unknown,
                        };
                        pieces.push(Piece::Value(origin));
                    }
                }
            }
        }
        Val::Str(pieces, lead_ref)
    }

    /// 지역 변수 초기식 — 모든 가지가 `?`로 시작하거나 빈 문자열이면 query 꼬리다.
    fn eval_local(&self, e: &syn::Expr) -> Val {
        if matches!(e, syn::Expr::If(_) | syn::Expr::Match(_)) {
            let mut branches = Vec::new();
            if branch_tails(e, &mut branches)
                && !branches.is_empty()
                && branches.iter().all(|b| self.is_query_like(b))
            {
                return Val::Str(vec![Piece::QueryTail], None);
            }
            return Val::unknown();
        }
        self.eval(e)
    }

    /// 값이 빈 문자열이거나 `?` 리터럴로 시작하는가.
    fn is_query_like(&self, e: &syn::Expr) -> bool {
        match self.eval(e) {
            Val::Str(p, _) => match p.first() {
                None => true,
                Some(Piece::Lit(l)) => l.is_empty() || l.starts_with('?'),
                Some(_) => false,
            },
            _ => false,
        }
    }

    // ── 호출 사실 ─────────────────────────────────────────

    /// 호출 위치의 CallSite를 기록한다(`Emit` 단계만).
    ///
    /// `method_param`은 동사 식이 매개변수인지다 — 동사를 흘려보내는 함수도 선언되지
    /// 않은 래퍼 싱크다.
    fn record(
        &self,
        span: proc_macro2::Span,
        method: Option<String>,
        v: &Val,
        lib: Client,
        method_param: bool,
    ) {
        let dots = !(lib == Client::Ureq && self.libs.ureq.as_ref().is_some_and(|(_, m)| *m >= 3));
        let outcome = match v {
            Val::Str(p, _) => UrlVal::parse(p, dots).outcome(),
            Val::Url(u, _) => u.outcome(),
            Val::Other(_) => dynamic(),
        };
        let dynamic_path = matches!(outcome, Outcome::Dynamic { .. });
        let param_driven = (dynamic_path && passes_param(&v.pieces())) || method_param;
        self.push_call(
            span,
            method,
            outcome,
            v.base_ref(),
            None,
            None,
            param_driven,
        );
    }

    /// 식이 감싸는 함수의 매개변수 그대로인가.
    fn is_param(&self, e: &syn::Expr) -> bool {
        match strip(e) {
            syn::Expr::Path(p) => p.path.get_ident().is_some_and(|id| {
                matches!(
                    self.env.get(&id.to_string()),
                    Some(Binding {
                        val: Val::Other(Origin::Param(_)),
                        ..
                    })
                )
            }),
            _ => false,
        }
    }

    /// 사실 하나를 결과에 넣는다. 선언된 래퍼 본문의 dynamic 호출은 싣지 않는다.
    #[allow(clippy::too_many_arguments)]
    fn push_call(
        &self,
        span: proc_macro2::Span,
        method: Option<String>,
        outcome: Outcome,
        base_ref: Option<String>,
        service: Option<String>,
        wrapper: Option<usize>,
        param_driven: bool,
    ) {
        if self.mode != Mode::Emit {
            return;
        }
        let mut out = self.sh.out.borrow_mut();
        if let Some(i) = wrapper {
            *out.wrapper_calls.entry(i).or_default() += 1;
        }
        if outcome == Outcome::Unrequestable {
            out.unrequestable += 1;
            return;
        }
        let unproven = matches!(outcome, Outcome::Dynamic { .. }) || method.is_none();
        if unproven && self.in_wrapper && wrapper.is_none() {
            return;
        }
        if unproven && param_driven && wrapper.is_none() {
            out.undeclared += 1;
        }
        out.calls.push(CallSite {
            loc: Loc {
                file: self.file.clone(),
                span,
            },
            method,
            outcome,
            base_ref,
            service,
        });
    }

    /// `reqwest::Method::GET`·`http::Method::GET` 경로의 동사 — 라이브러리 상수만 푼다.
    fn library_method(&self, e: &syn::Expr) -> Option<String> {
        let syn::Expr::Path(p) = strip(e) else {
            return None;
        };
        let full = self.expand(&crate::harvest::path_segments(&p.path));
        let parts: Vec<&str> = full.iter().map(String::as_str).collect();
        let lib_ok = |l: &str| l == "http" || self.libs.reqwest.as_deref() == Some(l);
        match parts.as_slice() {
            [lib, "Method", v] if lib_ok(lib) && VERBS.contains(v) => Some(v.to_string()),
            _ => None,
        }
    }

    /// 문자열 리터럴(상수 포함) 동사 — ureq `request("GET", url)`.
    fn literal_method(&self, e: &syn::Expr) -> Option<String> {
        match self.eval(e) {
            Val::Str(p, _) => match p.as_slice() {
                [Piece::Lit(s)] if VERBS.contains(&s.as_str()) => Some(s.clone()),
                _ => None,
            },
            _ => None,
        }
    }

    /// 자유 함수·연관 함수 호출이 요청·래퍼 호출인가.
    fn check_call(&mut self, c: &syn::ExprCall) {
        let syn::Expr::Path(p) = &*c.func else {
            return;
        };
        let segs = crate::harvest::path_segments(&p.path);
        if self.is_workspace(&segs) {
            self.check_wrapper_call(c, &segs);
            return;
        }
        let full = self.expand(&segs);
        let parts: Vec<&str> = full.iter().map(String::as_str).collect();
        let args: Vec<&syn::Expr> = c.args.iter().collect();
        let span = c.span();
        if let Some(lib) = self.libs.reqwest.clone() {
            match (parts.as_slice(), args.as_slice()) {
                ([l, "get"] | [l, "blocking", "get"], [url]) if *l == lib => {
                    let v = self.eval(url);
                    self.record(span, Some("GET".into()), &v, Client::Reqwest, false);
                }
                ([l, "Request", "new"] | [l, "blocking", "Request", "new"], [m, url])
                    if *l == lib =>
                {
                    let method = self.library_method(m);
                    let v = self.eval(url);
                    self.record(span, method, &v, Client::Reqwest, self.is_param(m));
                }
                _ => {}
            }
        }
        if let Some((lib, _)) = self.libs.ureq.clone() {
            match (parts.as_slice(), args.as_slice()) {
                ([l, verb], [url]) if *l == lib && is_verb_method(Client::Ureq, verb) => {
                    let v = self.eval(url);
                    self.record(
                        span,
                        Some(verb.to_ascii_uppercase()),
                        &v,
                        Client::Ureq,
                        false,
                    );
                }
                ([l, "request" | "request_url"], [m, url]) if *l == lib => {
                    let method = self.literal_method(m);
                    let v = self.eval(url);
                    self.record(span, method, &v, Client::Ureq, self.is_param(m));
                }
                _ => {}
            }
        }
    }

    /// 메서드 호출이 클라이언트 요청·래퍼 호출인가. 보내기 메서드면 수신자를
    /// 증명하지 못한 요청 모양 호출을 센다.
    fn check_method(&mut self, m: &syn::ExprMethodCall) {
        let name = m.method.to_string();
        let recv_ty = self.ty(&m.receiver);
        let client = self.client_of(recv_ty.as_deref());
        let args: Vec<&syn::Expr> = m.args.iter().collect();
        let span = m.span();
        match (client, name.as_str(), args.as_slice()) {
            (Some(k), verb, [url]) if is_verb_method(k, verb) => {
                let v = self.eval(url);
                self.record(span, Some(verb.to_ascii_uppercase()), &v, k, false);
                return;
            }
            (Some(k), "request", [method_expr, url]) => {
                let method = match k {
                    Client::Reqwest => self.library_method(method_expr),
                    Client::Ureq => self.literal_method(method_expr),
                };
                let v = self.eval(url);
                self.record(span, method, &v, k, self.is_param(method_expr));
                return;
            }
            _ => {}
        }
        if SEND_METHODS.contains(&name.as_str()) && self.mode == Mode::Emit {
            if let Some(verb_call) = request_call_in_chain(&m.receiver) {
                if self
                    .client_of(self.ty(&verb_call.receiver).as_deref())
                    .is_none()
                    && (self.libs.reqwest.is_some() || self.libs.ureq.is_some())
                {
                    self.sh.out.borrow_mut().unproven += 1;
                }
            }
        }
        if let Some(owner) = recv_ty {
            self.check_wrapper_method(m, &owner, &name);
        }
    }

    /// 워크스페이스 함수·연관 함수·튜플 구조체 생성이 선언된 래퍼인가.
    fn check_wrapper_call(&mut self, c: &syn::ExprCall, segs: &[String]) {
        let Some(id) = self.resolve_path(segs) else {
            return;
        };
        let args: Vec<CallArg> = c.args.iter().map(|a| self.call_arg(None, a)).collect();
        let exprs: Vec<&syn::Expr> = c.args.iter().collect();
        if let Some(&i) = self.sh.ctor_wrappers.get(&id) {
            self.emit_wrapper(i, c.span(), &args, &exprs);
            return;
        }
        let vertex = self.sh.ctx.resolve_vertex(&self.module, segs).unwrap_or(id);
        let Some(&i) = self.sh.fn_wrappers.get(&vertex) else {
            return;
        };
        // 메서드를 경로로 부르면(UFCS) 첫 인자가 수신자다 — 선언 index는 수신자를 뺀다.
        let skip = usize::from(self.has_receiver(&vertex));
        self.emit_wrapper(
            i,
            c.span(),
            &args[skip.min(args.len())..],
            &exprs[skip.min(exprs.len())..],
        );
    }

    /// 정점 ID가 self 수신자를 받는 메서드인가.
    fn has_receiver(&self, vertex: &str) -> bool {
        let Some((owner, name)) = vertex.rsplit_once("::") else {
            return false;
        };
        let owner = owner.split("::<").next().unwrap_or(owner);
        self.sh
            .index
            .methods
            .get(&(owner.to_string(), name.to_string()))
            .is_some_and(|fs| fs.iter().any(|f| f.sig.receiver().is_some()))
    }

    /// 수신자 타입이 래퍼 소유 타입인 메서드 호출.
    fn check_wrapper_method(&mut self, m: &syn::ExprMethodCall, owner: &str, name: &str) {
        let inherent = format!("{owner}::{name}");
        let trait_prefix = format!("{owner}::<");
        let suffix = format!(">::{name}");
        let hit = self.sh.fn_wrappers.iter().find(|(id, _)| {
            **id == inherent || (id.starts_with(&trait_prefix) && id.ends_with(&suffix))
        });
        let Some((_, &i)) = hit else {
            return;
        };
        let args: Vec<CallArg> = m.args.iter().map(|a| self.call_arg(None, a)).collect();
        let exprs: Vec<&syn::Expr> = m.args.iter().collect();
        self.emit_wrapper(i, m.span(), &args, &exprs);
    }

    /// 구조체 리터럴 생성 래퍼(`Endpoint { method, path }`).
    fn check_wrapper_struct(&mut self, s: &syn::ExprStruct) {
        let Some(id) = self.resolve_path(&crate::harvest::path_segments(&s.path)) else {
            return;
        };
        let Some(&i) = self.sh.ctor_wrappers.get(&id) else {
            return;
        };
        let mut args = Vec::new();
        let mut exprs = Vec::new();
        for fv in &s.fields {
            let label = match &fv.member {
                syn::Member::Named(n) => Some(n.to_string()),
                syn::Member::Unnamed(_) => None,
            };
            args.push(self.call_arg(label, &fv.expr));
            exprs.push(&fv.expr);
        }
        self.emit_wrapper(i, s.span(), &args, &exprs);
    }

    /// 호출 인자 하나를 바인딩 입력으로 바꾼다.
    fn call_arg(&self, label: Option<String>, e: &syn::Expr) -> CallArg {
        let value = match strip(e) {
            syn::Expr::Path(p) => {
                let segs = crate::harvest::path_segments(&p.path);
                let local = matches!(segs.as_slice(), [one] if self.env.get(one).is_some());
                match self.eval_path(&segs, 0) {
                    Val::Str(pieces, _) if !local => match pieces.as_slice() {
                        [Piece::Lit(s)] => ArgValue::Literal(s.clone()),
                        _ => ArgValue::Other,
                    },
                    _ if local => ArgValue::Other,
                    _ => segs
                        .last()
                        .map_or(ArgValue::Other, |s| ArgValue::EnumCase(s.clone())),
                }
            }
            other => match self.eval(other) {
                Val::Str(pieces, _) => match pieces.as_slice() {
                    [Piece::Lit(s)] => ArgValue::Literal(s.clone()),
                    _ => ArgValue::Other,
                },
                _ => ArgValue::Other,
            },
        };
        CallArg { label, value }
    }

    /// 선언된 래퍼 호출을 사실로 낸다.
    fn emit_wrapper(
        &mut self,
        i: usize,
        span: proc_macro2::Span,
        args: &[CallArg],
        exprs: &[&syn::Expr],
    ) {
        let w = &self.sh.wrappers[i];
        let method = wrappers::bind_method(&w.method_spec(), args);
        let path_pos = wrappers::find_arg(&w.path_arg, args)
            .and_then(|a| args.iter().position(|x| std::ptr::eq(x, a)));
        let anchor = if w.path_anchor == "root" {
            PathAnchor::Root
        } else {
            PathAnchor::Base
        };
        // 래퍼 경로 인자는 base 뒤의 경로라 baseRef를 싣지 않는다(base는 래퍼 안에 있다).
        let outcome = match path_pos.and_then(|p| exprs.get(p)) {
            Some(e) => wrapper_outcome(&self.eval(e).pieces(), anchor),
            None => dynamic(),
        };
        let service = w.service.clone();
        self.push_call(span, method, outcome, None, service, Some(i), false);
    }

    /// 모델링하지 않는 클라이언트 크레이트 경로의 사용을 센다.
    fn check_unmodelled(&mut self, segs: &[String]) {
        if self.mode != Mode::Emit || self.libs.unmodelled.is_empty() || self.is_workspace(segs) {
            return;
        }
        let full = self.expand(segs);
        let Some(first) = full.first() else { return };
        if !self.libs.unmodelled.contains(first) {
            return;
        }
        // hyper·hyper-util은 서버에도 쓰인다 — client 경로만 센다.
        let server_capable = first == "hyper" || first == "hyper_util";
        if server_capable && !full.iter().any(|s| s == "client" || s == "Client") {
            return;
        }
        *self
            .sh
            .out
            .borrow_mut()
            .unmodelled
            .entry(first.clone())
            .or_default() += 1;
    }

    /// 패턴의 이름들을 바인딩한다. 단순 이름이면 값·타입을 싣고, 분해 패턴은 모르는 값이다.
    fn bind_pat(&mut self, pat: &syn::Pat, val: Val, ty: Option<String>) {
        match pat {
            syn::Pat::Ident(i) if i.subpat.is_none() => {
                let name = i.ident.to_string();
                let val = if self.mutated.contains(&name) {
                    Val::unknown()
                } else {
                    val
                };
                self.env.bind(name, Binding { val, ty });
            }
            syn::Pat::Type(t) => {
                let ty = self.resolve_type(&t.ty).or(ty);
                self.bind_pat(&t.pat, val, ty);
            }
            other => {
                for name in pat_names(other) {
                    self.env.bind(
                        name,
                        Binding {
                            val: Val::unknown(),
                            ty: None,
                        },
                    );
                }
            }
        }
    }

    /// 조건식 안의 `let` 패턴 이름을 바인딩한다(`if let`·`while let`·let 체인).
    fn bind_lets(&mut self, cond: &syn::Expr) {
        match cond {
            syn::Expr::Let(l) => {
                let v = Val::unknown();
                self.bind_pat(&l.pat, v, None);
            }
            syn::Expr::Binary(b) if matches!(b.op, syn::BinOp::And(_)) => {
                self.bind_lets(&b.left);
                self.bind_lets(&b.right);
            }
            syn::Expr::Paren(p) => self.bind_lets(&p.expr),
            _ => {}
        }
    }
}

impl<'ast> Visit<'ast> for Scanner<'_, '_> {
    fn visit_block(&mut self, b: &'ast syn::Block) {
        self.env.push();
        syn::visit::visit_block(self, b);
        self.env.pop();
    }

    fn visit_local(&mut self, l: &'ast syn::Local) {
        if let Some(init) = &l.init {
            self.visit_expr(&init.expr);
            if let Some((_, div)) = &init.diverge {
                self.visit_expr(div);
            }
        }
        let (val, ty) = match &l.init {
            Some(init) if init.diverge.is_none() => {
                (self.eval_local(&init.expr), self.ty(&init.expr))
            }
            _ => (Val::unknown(), None),
        };
        self.bind_pat(&l.pat, val, ty);
    }

    fn visit_expr_closure(&mut self, c: &'ast syn::ExprClosure) {
        self.env.push();
        for input in &c.inputs {
            self.bind_pat(input, Val::unknown(), None);
        }
        self.visit_expr(&c.body);
        self.env.pop();
    }

    fn visit_expr_if(&mut self, e: &'ast syn::ExprIf) {
        self.visit_expr(&e.cond);
        self.env.push();
        self.bind_lets(&e.cond);
        self.visit_block(&e.then_branch);
        self.env.pop();
        if let Some((_, other)) = &e.else_branch {
            self.visit_expr(other);
        }
    }

    fn visit_expr_while(&mut self, e: &'ast syn::ExprWhile) {
        self.visit_expr(&e.cond);
        self.env.push();
        self.bind_lets(&e.cond);
        self.visit_block(&e.body);
        self.env.pop();
    }

    fn visit_expr_for_loop(&mut self, e: &'ast syn::ExprForLoop) {
        self.visit_expr(&e.expr);
        self.env.push();
        self.bind_pat(&e.pat, Val::unknown(), None);
        self.visit_block(&e.body);
        self.env.pop();
    }

    fn visit_arm(&mut self, a: &'ast syn::Arm) {
        self.env.push();
        self.bind_pat(&a.pat, Val::unknown(), None);
        if let Some((_, g)) = &a.guard {
            self.visit_expr(g);
        }
        self.visit_expr(&a.body);
        self.env.pop();
    }

    fn visit_item_fn(&mut self, f: &'ast syn::ItemFn) {
        if !is_test_item(&f.attrs) {
            self.scan_fn(None, &f.sig, &f.block);
        }
    }

    fn visit_item_impl(&mut self, i: &'ast syn::ItemImpl) {
        if !is_test_item(&i.attrs) {
            self.scan_impl(i);
        }
    }

    fn visit_item_mod(&mut self, _: &'ast syn::ItemMod) {
        // 모듈은 모듈 트리가 따로 훑는다(테스트 모듈 제외 규칙 포함).
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        // 매크로 인자가 쉼표로 나뉜 식이면 그 안의 요청도 본다(`tokio::join!` 등).
        let parsed = mac.parse_body_with(
            syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated,
        );
        if let Ok(exprs) = parsed {
            for e in &exprs {
                self.visit_expr(e);
            }
        }
    }

    fn visit_expr_call(&mut self, c: &'ast syn::ExprCall) {
        self.check_call(c);
        syn::visit::visit_expr_call(self, c);
    }

    fn visit_expr_method_call(&mut self, m: &'ast syn::ExprMethodCall) {
        if MUTATORS.contains(&m.method.to_string().as_str()) {
            self.poison_field(&m.receiver);
        }
        self.check_method(m);
        syn::visit::visit_expr_method_call(self, m);
    }

    fn visit_expr_struct(&mut self, s: &'ast syn::ExprStruct) {
        match self.mode {
            Mode::Collect => self.collect_struct(s),
            Mode::Emit => self.check_wrapper_struct(s),
        }
        syn::visit::visit_expr_struct(self, s);
    }

    fn visit_expr_path(&mut self, p: &'ast syn::ExprPath) {
        self.check_unmodelled(&crate::harvest::path_segments(&p.path));
        syn::visit::visit_expr_path(self, p);
    }

    fn visit_expr_assign(&mut self, a: &'ast syn::ExprAssign) {
        self.poison_field(&a.left);
        syn::visit::visit_expr_assign(self, a);
    }

    fn visit_expr_binary(&mut self, b: &'ast syn::ExprBinary) {
        if is_compound_assign(&b.op) {
            self.poison_field(&b.left);
        }
        syn::visit::visit_expr_binary(self, b);
    }

    fn visit_expr_reference(&mut self, r: &'ast syn::ExprReference) {
        if r.mutability.is_some() {
            self.poison_field(&r.expr);
        }
        syn::visit::visit_expr_reference(self, r);
    }
}

impl Scanner<'_, '_> {
    /// 구조체 리터럴의 필드 값을 누적한다(`Collect`).
    fn collect_struct(&self, s: &syn::ExprStruct) {
        let Some(id) = self.resolve_path(&crate::harvest::path_segments(&s.path)) else {
            return;
        };
        let Some(info) = self.sh.index.structs.get(&id) else {
            return;
        };
        let mut listed = BTreeSet::new();
        for fv in &s.fields {
            if let syn::Member::Named(n) = &fv.member {
                let name = n.to_string();
                self.sh.add_field(&id, &name, self.eval(&fv.expr));
                listed.insert(name);
            }
        }
        if s.rest.is_some() {
            for f in info.fields.keys().filter(|f| !listed.contains(*f)) {
                self.sh.add_field(&id, f, Val::unknown());
            }
        }
    }

    /// 필드를 제자리 수정하는 식이면 그 필드 이름을 믿지 않는다(`Collect`).
    fn poison_field(&self, target: &syn::Expr) {
        if self.mode != Mode::Collect {
            return;
        }
        if let syn::Expr::Field(f) = strip(target) {
            if let syn::Member::Named(n) = &f.member {
                let owner = self.ty(&f.base);
                self.sh.poisoned.borrow_mut().insert((owner, n.to_string()));
            }
        }
    }
}

/// 리터럴·상수만으로 된 값인가 — 생성 위치의 매개변수·지역 값에서 온 필드는
/// 인스턴스마다 다르므로 필드 값으로 확정하지 않는다.
fn is_constant(v: &Val) -> bool {
    let all_lit = |p: &[Piece]| p.iter().all(|x| matches!(x, Piece::Lit(_)));
    match v {
        Val::Str(p, _) => all_lit(p),
        Val::Url(u, _) => {
            u.authority.is_some()
                && matches!(&u.path, super::super::compose::UrlPath::Known { anchor: PathAnchor::Root, pieces } if all_lit(pieces))
        }
        Val::Other(_) => false,
    }
}

/// 매개변수를 URL로 그대로 흘려보내는 모양인가 — URL 전체가 매개변수이거나, 끝의
/// 매개변수가 `/` 없이 앞 리터럴(보통 base)에 붙는다. 세그먼트 일부를 채우는
/// 매개변수(`/files/{name}.json`)는 래퍼 선언으로 풀리지 않으므로 세지 않는다.
fn passes_param(pieces: &[Piece]) -> bool {
    match pieces {
        [Piece::Value(Origin::Param(_))] => true,
        [.., Piece::Lit(prev), Piece::Value(Origin::Param(_))] => !prev.ends_with('/'),
        [.., Piece::Value(_), Piece::Value(Origin::Param(_))] => true,
        _ => false,
    }
}

/// 요청 라이브러리가 그 동사 메서드를 갖는가.
fn is_verb_method(k: Client, name: &str) -> bool {
    let reqwest = ["get", "post", "put", "patch", "delete", "head"];
    let ureq = [
        "get", "post", "put", "patch", "delete", "head", "options", "trace",
    ];
    match k {
        Client::Reqwest => reqwest.contains(&name),
        Client::Ureq => ureq.contains(&name),
    }
}

/// 보내기 호출의 수신자 체인에서 빌더 메서드를 건너 동사·`request` 호출을 찾는다.
fn request_call_in_chain(e: &syn::Expr) -> Option<&syn::ExprMethodCall> {
    let syn::Expr::MethodCall(m) = strip(e) else {
        return None;
    };
    let name = m.method.to_string();
    let verb = ["get", "post", "put", "patch", "delete", "head"].contains(&name.as_str())
        && m.args.len() == 1;
    if verb || (name == "request" && m.args.len() == 2) {
        return Some(m);
    }
    if BUILDER_METHODS.contains(&name.as_str()) {
        return request_call_in_chain(&m.receiver);
    }
    None
}

/// 래퍼 경로 인자의 조립 결과 — 전체 URL이면 절대 해석, `/`로 시작하면 선언
/// 앵커, 상대 경로는 결합 방식을 모르므로 dynamic(base 앵커면 `ambiguous-base-join:`).
fn wrapper_outcome(pieces: &[Piece], anchor: PathAnchor) -> Outcome {
    match pieces.first() {
        Some(Piece::Lit(l)) if l.contains("://") => UrlVal::parse(pieces, true).outcome(),
        Some(Piece::Lit(l)) if l.starts_with('/') => {
            super::super::compose::compose_path(anchor, pieces, None, false)
        }
        Some(Piece::Lit(_)) => Outcome::Dynamic {
            prefix: None,
            anchor,
            ambiguous: anchor == PathAnchor::Base,
            masked_segments: 0,
        },
        _ => Outcome::Dynamic {
            prefix: None,
            anchor,
            ambiguous: false,
            masked_segments: 0,
        },
    }
}

/// 접두사 없는 dynamic.
fn dynamic() -> Outcome {
    Outcome::Dynamic {
        prefix: None,
        anchor: PathAnchor::Base,
        ambiguous: false,
        masked_segments: 0,
    }
}

/// 두 값을 문자열로 잇는다(`a + b`).
fn concat(a: Val, b: Val) -> Val {
    let r = match &a {
        Val::Str(p, _) if p.is_empty() => b.base_ref(),
        _ => a.base_ref(),
    };
    let mut pieces = a.pieces();
    pieces.extend(b.pieces());
    Val::Str(pieces, r)
}

/// 괄호·참조·그룹을 벗긴다.
fn strip(e: &syn::Expr) -> &syn::Expr {
    match e {
        syn::Expr::Paren(p) => strip(&p.expr),
        syn::Expr::Reference(r) => strip(&r.expr),
        syn::Expr::Group(g) => strip(&g.expr),
        other => other,
    }
}

/// 복합 대입 연산자인가(`+=` 등).
fn is_compound_assign(op: &syn::BinOp) -> bool {
    use syn::BinOp::*;
    matches!(
        op,
        AddAssign(_)
            | SubAssign(_)
            | MulAssign(_)
            | DivAssign(_)
            | RemAssign(_)
            | BitXorAssign(_)
            | BitAndAssign(_)
            | BitOrAssign(_)
            | ShlAssign(_)
            | ShrAssign(_)
    )
}

/// 패턴이 묶는 모든 이름.
fn pat_names(p: &syn::Pat) -> Vec<String> {
    struct Names(Vec<String>);
    impl<'ast> Visit<'ast> for Names {
        fn visit_pat_ident(&mut self, i: &'ast syn::PatIdent) {
            self.0.push(i.ident.to_string());
            syn::visit::visit_pat_ident(self, i);
        }
    }
    let mut n = Names(Vec::new());
    n.visit_pat(p);
    n.0
}

/// 함수 본문에서 제자리 수정되는 지역 변수 이름(대입·복합 대입·`&mut`·수정 메서드).
fn mutations(block: &syn::Block) -> BTreeSet<String> {
    struct Finder(BTreeSet<String>);
    impl Finder {
        fn target(&mut self, e: &syn::Expr) {
            if let syn::Expr::Path(p) = strip(e) {
                if let Some(id) = p.path.get_ident() {
                    self.0.insert(id.to_string());
                }
            }
        }
    }
    impl<'ast> Visit<'ast> for Finder {
        fn visit_expr_assign(&mut self, a: &'ast syn::ExprAssign) {
            self.target(&a.left);
            syn::visit::visit_expr_assign(self, a);
        }
        fn visit_expr_binary(&mut self, b: &'ast syn::ExprBinary) {
            if is_compound_assign(&b.op) {
                self.target(&b.left);
            }
            syn::visit::visit_expr_binary(self, b);
        }
        fn visit_expr_reference(&mut self, r: &'ast syn::ExprReference) {
            if r.mutability.is_some() {
                self.target(&r.expr);
            }
            syn::visit::visit_expr_reference(self, r);
        }
        fn visit_expr_method_call(&mut self, m: &'ast syn::ExprMethodCall) {
            if MUTATORS.contains(&m.method.to_string().as_str()) {
                self.target(&m.receiver);
            }
            syn::visit::visit_expr_method_call(self, m);
        }
    }
    let mut f = Finder(BTreeSet::new());
    f.visit_block(block);
    f.0
}

/// `if`/`match` 식의 가지 끝 식들. 끝 식이 없는 가지가 있으면 false.
fn branch_tails<'e>(e: &'e syn::Expr, out: &mut Vec<&'e syn::Expr>) -> bool {
    match e {
        syn::Expr::If(i) => {
            let Some((_, other)) = &i.else_branch else {
                return false;
            };
            block_tail(&i.then_branch).is_some_and(|t| branch_tails(t, out))
                && branch_tails(other, out)
        }
        syn::Expr::Match(m) => m.arms.iter().all(|a| branch_tails(&a.body, out)),
        syn::Expr::Block(b) => block_tail(&b.block).is_some_and(|t| branch_tails(t, out)),
        syn::Expr::Paren(p) => branch_tails(&p.expr, out),
        other => {
            out.push(other);
            true
        }
    }
}

/// 블록의 끝 식(세미콜론 없는 마지막 식).
fn block_tail(b: &syn::Block) -> Option<&syn::Expr> {
    match b.stmts.last() {
        Some(syn::Stmt::Expr(e, None)) => Some(e),
        _ => None,
    }
}

/// `format!` 서식 문자열 조각.
enum FmtSeg {
    Lit(String),
    Arg { key: FmtKey, spec: String },
}

/// 자리표시자가 가리키는 인자.
enum FmtKey {
    Next,
    Index(usize),
    Name(String),
}

/// 서식 문자열을 읽는다. 너비·정밀도를 인자로 받는 `$`·`*` 지정처럼 인자 순서를
/// 바꾸는 서식은 None(값 전체를 모른다).
fn parse_format(fmt: &str) -> Option<Vec<FmtSeg>> {
    let mut out = Vec::new();
    let mut lit = String::new();
    let mut chars = fmt.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                lit.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                lit.push('}');
            }
            '{' => {
                let mut inner = String::new();
                for c in chars.by_ref() {
                    if c == '}' {
                        break;
                    }
                    inner.push(c);
                }
                if !lit.is_empty() {
                    out.push(FmtSeg::Lit(std::mem::take(&mut lit)));
                }
                let (name, spec) = inner.split_once(':').unwrap_or((&inner, ""));
                if spec.contains(['$', '*']) {
                    return None;
                }
                let name = name.trim();
                let key = if name.is_empty() {
                    FmtKey::Next
                } else if let Ok(n) = name.parse::<usize>() {
                    FmtKey::Index(n)
                } else {
                    FmtKey::Name(name.to_string())
                };
                out.push(FmtSeg::Arg {
                    key,
                    spec: spec.to_string(),
                });
            }
            other => lit.push(other),
        }
    }
    if !lit.is_empty() {
        out.push(FmtSeg::Lit(lit));
    }
    Some(out)
}
