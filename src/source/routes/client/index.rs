//! 클라이언트 추출기의 워크스페이스 색인 — 구조체 필드 타입, impl 메서드, 자유
//! 함수, 문자열 상수·static, 모듈별 `use` 표.
//!
//! 타입은 경로 문자열로 다룬다. 워크스페이스 아이템은 rustograph 정점 ID
//! (`app::api::ApiClient`), 외부 타입은 `use`를 펼친 경로(`reqwest::Client`)다.
//! 제네릭은 버리고, 역참조로 같은 메서드를 부르는 포장(`&`·`Arc`·`Rc`·`Box`·
//! `Lazy`·`LazyLock`·`OnceLock`·`Result`)은 벗긴다 — 수신자가 reqwest 클라이언트인지
//! 가리는 데 필요한 만큼만 본다.

use super::super::common::{is_test_item, Ctx, Imports};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// 역참조·언랩으로 안쪽 타입의 메서드를 그대로 부르는 포장 타입 이름.
const TRANSPARENT: &[&str] = &[
    "Arc", "Rc", "Box", "Lazy", "LazyLock", "LazyCell", "OnceLock", "OnceCell", "Result", "Cow",
];

/// 함수 하나의 서명과 위치.
pub(super) struct FnInfo {
    pub module: String,
    pub sig: &'static syn::Signature,
    /// impl 메서드면 그 self 타입 ID.
    pub self_ty: Option<String>,
}

/// 문자열 상수·static 하나.
pub(super) struct ConstInfo {
    pub module: String,
    pub expr: &'static syn::Expr,
    pub ty: &'static syn::Type,
    pub self_ty: Option<String>,
}

/// 구조체 하나 — 이름 붙은 필드의 타입과 선언 모듈.
pub(super) struct StructInfo {
    pub module: String,
    pub fields: BTreeMap<String, &'static syn::Type>,
    /// `#[derive(Default)]` — 모든 필드가 기본값(빈 문자열)인 생성 경로가 있다.
    pub derives_default: bool,
}

/// 워크스페이스 색인.
pub(super) struct Index {
    pub structs: BTreeMap<String, StructInfo>,
    /// (self 타입 ID, 메서드 이름) → 서명들(고유 impl과 트레이트 impl).
    pub methods: BTreeMap<(String, String), Vec<FnInfo>>,
    pub fns: BTreeMap<String, FnInfo>,
    pub consts: BTreeMap<String, ConstInfo>,
    /// 스캔하는 멤버 크레이트 루트 이름.
    pub roots: std::collections::BTreeSet<String>,
    imports: RefCell<BTreeMap<String, std::rc::Rc<Imports>>>,
}

impl Index {
    /// 크레이트 루트들의 테스트가 아닌 모듈을 훑어 색인을 만든다.
    pub fn build(ctx: &Ctx, krates: &[String]) -> Index {
        let mut index = Index {
            structs: BTreeMap::new(),
            methods: BTreeMap::new(),
            fns: BTreeMap::new(),
            consts: BTreeMap::new(),
            roots: krates.iter().cloned().collect(),
            imports: RefCell::new(BTreeMap::new()),
        };
        for krate in krates {
            for module in ctx.crate_modules(krate) {
                for (_, items) in ctx.parts.module_items(&module) {
                    for item in items {
                        index.add_item(ctx, &module, item);
                    }
                }
            }
        }
        index
    }

    /// 모듈 수준 아이템 하나를 색인에 넣는다.
    fn add_item(&mut self, ctx: &Ctx, module: &str, item: &'static syn::Item) {
        match item {
            syn::Item::Struct(s) if !is_test_item(&s.attrs) => {
                let fields = match &s.fields {
                    syn::Fields::Named(n) => n
                        .named
                        .iter()
                        .filter_map(|f| Some((f.ident.as_ref()?.to_string(), &f.ty)))
                        .collect(),
                    _ => BTreeMap::new(),
                };
                self.structs.insert(
                    format!("{module}::{}", s.ident),
                    StructInfo {
                        module: module.to_string(),
                        fields,
                        derives_default: derives_default(&s.attrs),
                    },
                );
            }
            syn::Item::Fn(f) if !is_test_item(&f.attrs) => {
                self.fns.insert(
                    format!("{module}::{}", f.sig.ident),
                    FnInfo {
                        module: module.to_string(),
                        sig: &f.sig,
                        self_ty: None,
                    },
                );
            }
            syn::Item::Const(c) => self.insert_const(
                format!("{module}::{}", c.ident),
                module,
                &c.expr,
                &c.ty,
                None,
            ),
            syn::Item::Static(s) => self.insert_const(
                format!("{module}::{}", s.ident),
                module,
                &s.expr,
                &s.ty,
                None,
            ),
            syn::Item::Impl(i) if !is_test_item(&i.attrs) => self.add_impl(ctx, module, i),
            _ => {}
        }
    }

    /// impl 블록의 메서드와 연관 상수를 넣는다.
    fn add_impl(&mut self, ctx: &Ctx, module: &str, i: &'static syn::ItemImpl) {
        let Some(self_ty) = self.resolve_type(ctx, module, None, &i.self_ty) else {
            return;
        };
        for item in &i.items {
            match item {
                syn::ImplItem::Fn(f) if !is_test_item(&f.attrs) => {
                    self.methods
                        .entry((self_ty.clone(), f.sig.ident.to_string()))
                        .or_default()
                        .push(FnInfo {
                            module: module.to_string(),
                            sig: &f.sig,
                            self_ty: Some(self_ty.clone()),
                        });
                }
                syn::ImplItem::Const(c) => self.insert_const(
                    format!("{self_ty}::{}", c.ident),
                    module,
                    &c.expr,
                    &c.ty,
                    Some(self_ty.clone()),
                ),
                _ => {}
            }
        }
    }

    fn insert_const(
        &mut self,
        id: String,
        module: &str,
        expr: &'static syn::Expr,
        ty: &'static syn::Type,
        self_ty: Option<String>,
    ) {
        self.consts.insert(
            id,
            ConstInfo {
                module: module.to_string(),
                expr,
                ty,
                self_ty,
            },
        );
    }

    /// 모듈의 `use` 표(캐시).
    pub fn imports(&self, ctx: &Ctx, module: &str) -> std::rc::Rc<Imports> {
        if let Some(i) = self.imports.borrow().get(module) {
            return i.clone();
        }
        let groups = ctx.parts.module_items(module);
        let imports = std::rc::Rc::new(Imports::of(&groups));
        self.imports
            .borrow_mut()
            .insert(module.to_string(), imports.clone());
        imports
    }

    /// 경로를 타입 문자열로 — 워크스페이스 아이템이면 정점 ID, 아니면 `use`를 펼친
    /// 외부 경로. `Self`는 self 타입이다.
    pub fn resolve_path(
        &self,
        ctx: &Ctx,
        module: &str,
        self_ty: Option<&str>,
        segs: &[String],
    ) -> Option<String> {
        let (first, rest) = segs.split_first()?;
        if first == "Self" {
            let mut out = self_ty?.to_string();
            for s in rest {
                out.push_str("::");
                out.push_str(s);
            }
            return Some(out);
        }
        let dep = crate::modtree::DepCrates::new();
        // 가장 긴 워크스페이스 접두사를 찾는다 — `ApiClient::new`는 타입까지만 모듈
        // 트리에 있고 연관 함수 이름은 그 뒤에 붙는다.
        for cut in (1..=segs.len()).rev() {
            if let Some(id) = workspace_resolve(ctx, self, module, &segs[..cut], &dep) {
                let mut out = id;
                for s in &segs[cut..] {
                    out.push_str("::");
                    out.push_str(s);
                }
                return Some(out);
            }
        }
        let full = self.imports(ctx, module).expand(segs);
        (!full.is_empty()).then(|| full.join("::"))
    }

    /// 타입 식을 타입 문자열로 — 참조·괄호·포장 타입을 벗긴다.
    pub fn resolve_type(
        &self,
        ctx: &Ctx,
        module: &str,
        self_ty: Option<&str>,
        ty: &syn::Type,
    ) -> Option<String> {
        match ty {
            syn::Type::Reference(r) => self.resolve_type(ctx, module, self_ty, &r.elem),
            syn::Type::Paren(p) => self.resolve_type(ctx, module, self_ty, &p.elem),
            syn::Type::Group(g) => self.resolve_type(ctx, module, self_ty, &g.elem),
            syn::Type::Path(p) => {
                let last = p.path.segments.last()?;
                if TRANSPARENT.contains(&last.ident.to_string().as_str()) {
                    if let syn::PathArguments::AngleBracketed(a) = &last.arguments {
                        let inner = a.args.iter().find_map(|g| match g {
                            syn::GenericArgument::Type(t) => Some(t),
                            _ => None,
                        })?;
                        return self.resolve_type(ctx, module, self_ty, inner);
                    }
                }
                let segs = crate::harvest::path_segments(&p.path);
                self.resolve_path(ctx, module, self_ty, &segs)
            }
            _ => None,
        }
    }

    /// 서명의 반환 타입.
    pub fn return_type(&self, ctx: &Ctx, f: &FnInfo) -> Option<String> {
        match &f.sig.output {
            syn::ReturnType::Type(_, ty) => {
                self.resolve_type(ctx, &f.module, f.self_ty.as_deref(), ty)
            }
            syn::ReturnType::Default => None,
        }
    }

    /// 구조체 필드의 타입.
    pub fn field_type(&self, ctx: &Ctx, owner: &str, field: &str) -> Option<String> {
        let s = self.structs.get(owner)?;
        let ty = s.fields.get(field)?;
        self.resolve_type(ctx, &s.module, Some(owner), ty)
    }
}

/// 경로를 모듈 트리로 풀되 스캔하는 멤버 크레이트 안의 ID만 받는다 — 트리는 외부
/// 크레이트 이름(`reqwest`)도 크레이트 정점으로 풀고, 워크스페이스 밖 path 의존도
/// 모듈로 담을 수 있다.
pub(super) fn workspace_resolve(
    ctx: &Ctx,
    index: &Index,
    module: &str,
    segs: &[String],
    dep: &crate::modtree::DepCrates,
) -> Option<String> {
    let id = ctx.parts.tree.resolve(module, segs, dep)?;
    index
        .roots
        .contains(&crate::modtree::crate_of(&id))
        .then_some(id)
}

/// `#[derive(.., Default, ..)]`인가.
fn derives_default(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("derive")
            && a.parse_args_with(
                syn::punctuated::Punctuated::<syn::Path, syn::Token![,]>::parse_terminated,
            )
            .is_ok_and(|list| {
                list.iter()
                    .any(|p| p.segments.last().is_some_and(|s| s.ident == "Default"))
            })
    })
}

/// 모듈 파일 목록(테스트 모듈 제외) — 스캐너가 아이템을 훑는 단위다.
pub(super) fn module_groups(
    ctx: &Ctx,
    krate: &str,
) -> Vec<(String, PathBuf, &'static [syn::Item])> {
    let mut out = Vec::new();
    for module in ctx.crate_modules(krate) {
        for (file, items) in ctx.parts.module_items(&module) {
            out.push((module.clone(), file, items));
        }
    }
    out
}
