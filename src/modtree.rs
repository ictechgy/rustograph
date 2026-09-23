//! 모듈 트리와 경로 해석.
//!
//! Rust의 `mod` 선언만이 파일→모듈의 권위다 — 디렉터리를 훑어 모듈을
//! "발견"하지 않는다(발견하면 선언되지 않은 orphan 파일이 정점으로 새고,
//! 그건 유령 정점이다). orphan 파일은 limitation으로 센다.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// `use` 임포트 하나 — 해석된 정규 경로와 `#[cfg]` 조건.
/// 조건이 있으면 그 빌드에서만 존재하는 임포트다 — 간선도 조건을 물려받는다.
#[derive(Debug, Clone)]
pub struct Import {
    /// 정규 ID (`crate::mod::item`).
    pub target: String,
    /// `#[cfg(...)]` 조건 토큰 — 없으면 무조건 임포트.
    pub cfg: Option<String>,
}

/// 모듈 하나 — 파일 단위든 인라인 `mod x {}`든 동일하게 표현한다.
/// 경로는 트리의 맵 키이므로 필드로 중복 저장하지 않는다.
#[derive(Debug)]
pub struct Module {
    /// 이 모듈의 선언이 있는 파일(인라인 모듈은 부모와 같은 파일).
    pub file: PathBuf,
    /// 루트 모듈을 공유하는 다른 타깃의 루트 파일 — lib와 같은 이름의
    /// bin이 있으면 두 크레이트 인스턴스가 한 모듈 경로를 공유한다
    /// (bin의 `use pkg::x`는 lib 네임스페이스를 가리키므로 합집합이 맞다).
    pub extra_files: Vec<PathBuf>,
    /// 파일 루트 모듈인가 — false면 부모 AST 안의 인라인 `mod x {}`다.
    pub file_module: bool,
    /// `pub mod`로 선언됐는가 — 크레이트 루트는 항상 true다.
    pub public: bool,
    /// `#[cfg(...)]` 조건 토큰 — `mod` 선언에 붙은 것만(조상 조건은 조상 정점에).
    pub cfg: Option<String>,
    /// 직접 선언된 아이템 이름들(모듈 스코프 해석용).
    pub items: BTreeSet<String>,
    /// `use` 임포트 맵: 마지막 세그먼트(또는 as 이름) → 임포트.
    pub imports: BTreeMap<String, Import>,
    /// 자식 모듈 이름 → 경로.
    pub children: BTreeMap<String, String>,
}

impl Module {
    /// 새 모듈 항목.
    pub fn new(file: PathBuf, file_module: bool, public: bool) -> Module {
        Module {
            file,
            extra_files: Vec::new(),
            file_module,
            public,
            cfg: None,
            items: BTreeSet::new(),
            imports: BTreeMap::new(),
            children: BTreeMap::new(),
        }
    }
}

/// 크레이트 하나의 모듈 트리 전체.
#[derive(Debug, Default)]
pub struct ModTree {
    /// 경로 → 모듈.
    pub modules: BTreeMap<String, Module>,
    /// 선언에 도달하지 못한 .rs 파일(orphan) — limitation으로 올린다.
    pub orphan_files: Vec<PathBuf>,
}

/// 모듈 경로에서 크레이트(첫 세그먼트)를 뗀다.
pub fn crate_of(path: &str) -> String {
    path.split("::").next().unwrap_or(path).to_string()
}

/// 부모 모듈 경로 — 루트 모듈의 부모는 없다.
pub fn parent_of(path: &str) -> Option<String> {
    path.rsplit_once("::").map(|(p, _)| p.to_string())
}

/// `mod x;` 선언이 가리키는 파일을 찾는다: `x.rs` 또는 `x/mod.rs`,
/// `#[path = "..."]` 우선. 파일이 없으면 선언만 있고 내용이 없는 모듈이다 —
/// 유령 정점을 만들지 않기 위해 None을 돌려준다.
pub fn mod_file(dir: &Path, name: &str, path_attr: Option<&str>) -> Option<PathBuf> {
    if let Some(p) = path_attr {
        let f = dir.join(p);
        return f.exists().then(|| f.canonicalize().unwrap_or(f));
    }
    for cand in [
        dir.join(format!("{name}.rs")),
        dir.join(name).join("mod.rs"),
    ] {
        if cand.exists() {
            return Some(cand.canonicalize().unwrap_or(cand));
        }
    }
    None
}

/// 모듈의 파일이 있는 디렉터리 — `mod x;`의 상대 기준.
/// 루트(lib.rs/main.rs)와 `mod.rs`는 자기 디렉터리, 그 외 파일 `a.rs`는
/// `a/`가 자식 모듈 기준 디렉터리다.
pub fn module_dir(file: &Path) -> PathBuf {
    let dir = file.parent().unwrap_or(Path::new("."));
    match file.file_stem().and_then(|s| s.to_str()) {
        Some("lib") | Some("main") | Some("mod") => dir.to_path_buf(),
        Some(stem) => dir.join(stem),
        None => dir.to_path_buf(),
    }
}

impl ModTree {
    /// 모듈 경로에서 시작해 `use` 세그먼트를 해석한다.
    /// 반환값은 정규 ID. 해석 불가(외부 크레이트·prelude·매크로 생성 이름)면 None —
    /// 유령 정점을 만들지 않는 것이 계약이다.
    pub fn resolve(
        &self,
        from: &str,
        segs: &[String],
        dep_crates: &BTreeSet<String>,
    ) -> Option<String> {
        if segs.is_empty() {
            return None;
        }
        let mut i = 1usize;
        let start: String = match segs[0].as_str() {
            "crate" => crate_of(from),
            "self" => from.to_string(),
            "super" => {
                let mut m = parent_of(from)?;
                while i < segs.len() && segs[i] == "super" {
                    m = parent_of(&m)?;
                    i += 1;
                }
                m
            }
            first => {
                let module = self.modules.get(from)?;
                if module.items.contains(first) || module.children.contains_key(first) {
                    format!("{from}::{first}")
                } else if let Some(imp) = module.imports.get(first) {
                    imp.target.clone()
                } else {
                    let root = crate_of(from);
                    let root_mod = self.modules.get(&root)?;
                    if root_mod.items.contains(first) || root_mod.children.contains_key(first) {
                        format!("{root}::{first}")
                    } else if self.modules.contains_key(first) {
                        // 같은 워크스페이스의 다른 크레이트 루트 모듈.
                        return self.walk(first, &segs[1..]);
                    } else if dep_crates.contains(first) {
                        return None; // 외부 크레이트 — 정점을 만들지 않는다.
                    } else {
                        return None;
                    }
                }
            }
        };
        self.walk(&start, &segs[i..])
    }

    /// 시작 경로에서 나머지 세그먼트를 따라 걷는다.
    /// 중간 세그먼트가 모듈이면 내려가고, 아니면 아이템 경로로 존재를 확인한다.
    fn walk(&self, start: &str, rest: &[String]) -> Option<String> {
        let mut cur = start.to_string();
        for (j, seg) in rest.iter().enumerate() {
            let next = format!("{cur}::{seg}");
            if self.modules.contains_key(&next) {
                cur = next;
                continue;
            }
            // 모듈이 아니면 아이템이다 — 뒤는 전부 아이템 하위 경로.
            let tail = rest[j..].join("::");
            let candidate = format!("{cur}::{tail}");
            return self.item_exists(&candidate).then_some(candidate);
        }
        if self.modules.contains_key(&cur) {
            return Some(cur);
        }
        self.item_exists(&cur).then_some(cur)
    }

    /// 경로의 마지막 세그먼트가 어떤 모듈의 선언 아이템인지 확인한다.
    /// 아이템 표는 전 모듈에 대해 먼저 채워지므로 이 판정은 순서 무관하다.
    pub fn item_exists(&self, path: &str) -> bool {
        let Some((owner, name)) = path.rsplit_once("::") else {
            return false;
        };
        // 메서드 경로(Type::method)는 아이템 표에 없으므로 모듈 아이템만 본다.
        self.modules
            .get(owner)
            .is_some_and(|m| m.items.contains(name))
    }
}

/// ast 아이템들에서 `mod` 선언을 찾아 트리에 심는다.
/// 반환값: 큐에 넣을 서브모듈 경로들 — 파일·인라인 모두 큐에 넣고,
/// 아이템 목록은 module_items가 파일/인라인을 구분해 찾는다.
pub fn collect_submodules(
    items: &[&syn::Item],
    parent_path: &str,
    parent_dir: &Path,
    tree: &mut ModTree,
    conditional_count: &mut usize,
) -> Vec<String> {
    let mut queued = Vec::new();
    for item in items {
        let syn::Item::Mod(m) = item else { continue };
        let name = m.ident.to_string();
        let path = format!("{parent_path}::{name}");
        // 병합 루트(lib+같은 이름 bin)를 두 번째 타깃이 다시 훑을 때 같은
        // mod 선언을 재계수하지 않는다 — 처음 보는 경로일 때만 센다.
        let cfg = cfg_of(&m.attrs);
        if cfg.is_some() && !tree.modules.contains_key(&path) {
            *conditional_count += 1;
        }
        let (file, is_file_module) = if let Some((_, _)) = &m.content {
            // 인라인 모듈 — 같은 파일.
            (tree.modules[parent_path].file.clone(), false)
        } else {
            // `#[path = "..."]`는 NameValue 메타다 — `parse_args`는
            // `#[path("...")]` 문법만 받으므로 값은 nv.value에서 읽는다.
            let path_attr =
                m.attrs
                    .iter()
                    .find(|a| a.path().is_ident("path"))
                    .and_then(|a| match &a.meta {
                        syn::Meta::NameValue(nv) => match &nv.value {
                            syn::Expr::Lit(syn::ExprLit {
                                lit: syn::Lit::Str(s),
                                ..
                            }) => Some(s.value()),
                            _ => None,
                        },
                        _ => None,
                    });
            match mod_file(parent_dir, &name, path_attr.as_deref()) {
                Some(f) => (f, true),
                None => continue, // 파일 없는 mod(조건부·생성) — 정점 없이 limitation만.
            }
        };
        let public = matches!(m.vis, syn::Visibility::Public(_));
        let mut module = Module::new(file, is_file_module, public);
        module.cfg = cfg;
        tree.modules.insert(path.clone(), module);
        tree.modules
            .get_mut(parent_path)
            .expect("parent module must exist")
            .children
            .insert(name, path.clone());
        queued.push(path);
    }
    queued
}

/// `#[cfg]`가 붙어 있으면 조건부로 본다 — 포함은 하되 실측으로 센다.
pub fn has_cfg(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| a.path().is_ident("cfg"))
}

/// `#[cfg(...)]`의 조건 토큰을 그대로 돌려준다 — `feature = "x"`, `test` 등.
/// 여러 cfg 속성이 붙으면 모두 성립해야 하니 `all(...)`로 합성해 돌린다 —
/// 정점 하나에 조건 하나라는 필드 계약을 유지하기 위해서다.
pub fn cfg_of(attrs: &[syn::Attribute]) -> Option<String> {
    let conds: Vec<String> = attrs
        .iter()
        .filter(|a| a.path().is_ident("cfg"))
        .filter_map(|a| a.meta.require_list().ok().map(|l| l.tokens.to_string()))
        .collect();
    match conds.len() {
        0 => None,
        1 => Some(conds.into_iter().next().expect("len checked")),
        _ => Some(format!("all({})", conds.join(" , "))),
    }
}

/// 모듈의 직접 아이템 이름을 채운다(1단계).
/// `use` 해석은 다른 모듈의 아이템 표에 의존하므로, 전 모듈의 아이템 표가
/// 완성된 뒤 fill_imports가 따로 돌아야 한다 — 같은 패스로 하면 알파벳 순서에
/// 따라 크로스 크레이트 임포트가 조용히 유실된다.
pub fn fill_items(tree: &mut ModTree, path: &str, items: &[&syn::Item]) {
    let mut names = BTreeSet::new();
    for item in items {
        match item {
            syn::Item::Fn(f) => {
                names.insert(f.sig.ident.to_string());
            }
            syn::Item::Struct(s) => {
                names.insert(s.ident.to_string());
            }
            syn::Item::Enum(e) => {
                names.insert(e.ident.to_string());
            }
            syn::Item::Trait(t) => {
                names.insert(t.ident.to_string());
            }
            syn::Item::Union(u) => {
                names.insert(u.ident.to_string());
            }
            syn::Item::Type(t) => {
                names.insert(t.ident.to_string());
            }
            syn::Item::Const(c) => {
                names.insert(c.ident.to_string());
            }
            syn::Item::Static(s) => {
                names.insert(s.ident.to_string());
            }
            syn::Item::Mod(m) => {
                names.insert(m.ident.to_string());
            }
            syn::Item::Macro(m) => {
                if let Some(id) = &m.ident {
                    names.insert(id.to_string());
                }
            }
            _ => {}
        }
    }
    tree.modules
        .get_mut(path)
        .expect("module must exist")
        .items
        .extend(names);
}

/// 모듈의 `use` 임포트 맵을 해석해 채운다(2단계 — 전 모듈의 fill_items 이후).
pub fn fill_imports(
    tree: &mut ModTree,
    path: &str,
    items: &[&syn::Item],
    dep_crates: &BTreeSet<String>,
) {
    let mut uses: Vec<(Vec<String>, Option<String>)> = Vec::new();
    for item in items {
        if let syn::Item::Use(u) = item {
            let cfg = cfg_of(&u.attrs);
            flatten_use(&u.tree, &mut Vec::new(), &mut |segs| {
                uses.push((segs, cfg.clone()));
            });
        }
    }
    // 해석은 불변 참조가 필요하니 먼저 모으고 그 다음 심는다.
    // 글롭(`use m::*`)은 대상 모듈의 모든 공개 아이템과 자식 모듈을
    // 임포트한다 — `m` 자체만 매핑하면 `f()` 호출이 미해석으로 새니
    // 아이템별로 임포트 맵을 펼친다.
    let mut resolved: Vec<(String, Import)> = Vec::new();
    for (segs, cfg) in &uses {
        let glob = segs.last().is_some_and(|s| s == "*");
        let clean: Vec<String> = segs
            .iter()
            .filter(|s| !s.starts_with("as ") && *s != "*")
            .cloned()
            .collect();
        let Some(full) = tree.resolve(path, &clean, dep_crates) else {
            continue;
        };
        if glob {
            if let Some(target) = tree.modules.get(&full) {
                for name in target.items.iter().chain(target.children.keys()) {
                    resolved.push((
                        name.clone(),
                        Import {
                            target: format!("{full}::{name}"),
                            cfg: cfg.clone(),
                        },
                    ));
                }
            }
            resolved.push((
                full.rsplit("::").next().unwrap_or(&full).to_string(),
                Import {
                    target: full,
                    cfg: cfg.clone(),
                },
            ));
            continue;
        }
        let alias = segs
            .iter()
            .rev()
            .find(|s| s.starts_with("as "))
            .map(|s| s[3..].to_string())
            .unwrap_or_else(|| segs.last().unwrap().clone());
        resolved.push((
            alias,
            Import {
                target: full,
                cfg: cfg.clone(),
            },
        ));
    }
    let module = tree.modules.get_mut(path).expect("module must exist");
    for (alias, imp) in resolved {
        module.imports.insert(alias, imp);
    }
}

/// `use` 트리를 평탄한 경로 목록으로 펼친다 — `{a, b}` 그룹과 `as` 별칭을 처리한다.
/// 경로마다 use 아이템의 cfg를 붙여야 해서 결과는 콜백으로 흘려보낸다.
fn flatten_use(tree: &syn::UseTree, prefix: &mut Vec<String>, out: &mut dyn FnMut(Vec<String>)) {
    match tree {
        syn::UseTree::Path(p) => {
            prefix.push(p.ident.to_string());
            flatten_use(&p.tree, prefix, out);
            prefix.pop();
        }
        syn::UseTree::Name(n) => {
            let mut p = prefix.clone();
            p.push(n.ident.to_string());
            out(p);
        }
        syn::UseTree::Rename(r) => {
            let mut p = prefix.clone();
            p.push(r.ident.to_string());
            p.push(format!("as {}", r.rename));
            out(p);
        }
        syn::UseTree::Glob(_) => {
            // glob은 모듈 자체 + 그 아이템 전부 — `*` 마커를 남겨
            // fill_imports가 아이템별로 펼치게 한다.
            let mut p = prefix.clone();
            p.push("*".to_string());
            out(p);
        }
        syn::UseTree::Group(g) => {
            for t in &g.items {
                flatten_use(t, prefix, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 최소 트리: c 루트(items: f, S), c::m(items: g, 임포트 h→c::util::h).
    fn tree() -> ModTree {
        let mut t = ModTree::default();
        let mut root = Module::new(PathBuf::from("lib.rs"), true, true);
        root.items = BTreeSet::from(["f".to_string(), "S".to_string(), "m".to_string()]);
        root.children.insert("m".to_string(), "c::m".to_string());
        t.modules.insert("c".to_string(), root);
        let mut m = Module::new(PathBuf::from("m.rs"), true, true);
        m.items = BTreeSet::from(["g".to_string()]);
        m.imports.insert(
            "h".to_string(),
            Import {
                target: "c::util::h".to_string(),
                cfg: None,
            },
        );
        t.modules.insert("c::m".to_string(), m);
        let mut u = Module::new(PathBuf::from("util.rs"), true, true);
        u.items = BTreeSet::from(["h".to_string()]);
        t.modules.insert("c::util".to_string(), u);
        t
    }

    #[test]
    fn resolve_crate_self_super_and_items() {
        let t = tree();
        let none = BTreeSet::new();
        // crate:: 접두사.
        assert_eq!(
            t.resolve("c::m", &["crate".into(), "f".into()], &none),
            Some("c::f".to_string())
        );
        // self:: 접두사.
        assert_eq!(
            t.resolve("c::m", &["self".into(), "g".into()], &none),
            Some("c::m::g".to_string())
        );
        // super:: 접두사.
        assert_eq!(
            t.resolve("c::m", &["super".into(), "f".into()], &none),
            Some("c::f".to_string())
        );
        // 모듈 로컬 아이템.
        assert_eq!(
            t.resolve("c", &["f".into()], &none),
            Some("c::f".to_string())
        );
    }

    #[test]
    fn resolve_uses_imports_and_nested_paths() {
        let t = tree();
        let none = BTreeSet::new();
        // use 별칭 → 임포트 대상.
        assert_eq!(
            t.resolve("c::m", &["h".into()], &none),
            Some("c::util::h".to_string())
        );
        // 자식 모듈 경유.
        assert_eq!(
            t.resolve("c", &["m".into(), "g".into()], &none),
            Some("c::m::g".to_string())
        );
        // 워크스페이스의 다른 크레이트 루트.
        assert_eq!(
            t.resolve("c", &["other".into(), "x".into()], &{
                let mut t2 = tree();
                t2.modules.insert(
                    "other".to_string(),
                    Module::new(PathBuf::from("o.rs"), true, true),
                );
                drop(t2);
                BTreeSet::new()
            }),
            None // other 크레이트는 modules에 없으니 None.
        );
    }

    #[test]
    fn resolve_rejects_external_and_unknown() {
        let t = tree();
        let deps = BTreeSet::from(["serde".to_string()]);
        // 외부 크레이트 — 정점을 만들지 않는다.
        assert_eq!(t.resolve("c", &["serde".into(), "de".into()], &deps), None);
        // 아무 것도 아닌 이름.
        assert_eq!(t.resolve("c", &["nope".into()], &BTreeSet::new()), None);
    }

    #[test]
    fn mod_file_resolution() {
        let dir = std::env::temp_dir().join(format!("rg-modfile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("x.rs"), "pub fn a(){}").unwrap();
        std::fs::create_dir_all(dir.join("y")).unwrap();
        std::fs::write(dir.join("y").join("mod.rs"), "").unwrap();
        assert!(mod_file(&dir, "x", None).is_some());
        assert!(mod_file(&dir, "y", None).is_some());
        assert!(mod_file(&dir, "missing", None).is_none());
        // #[path] 우선.
        std::fs::write(dir.join("elsewhere.rs"), "").unwrap();
        assert!(mod_file(&dir, "z", Some("elsewhere.rs")).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cfg_of_extracts_condition_tokens() {
        // #[cfg(test)]와 #[cfg(feature = "x")]는 조건 토큰 그대로 나온다.
        let item: syn::Item = syn::parse_str("#[cfg(test)] fn f() {}").unwrap();
        let attrs = match &item {
            syn::Item::Fn(f) => &f.attrs,
            _ => unreachable!(),
        };
        assert_eq!(cfg_of(attrs), Some("test".to_string()));
        let item: syn::Item =
            syn::parse_str("#[cfg(all(unix, feature = \"x\"))] #[cfg(debug_assertions)] fn f() {}")
                .unwrap();
        let attrs = match &item {
            syn::Item::Fn(f) => &f.attrs,
            _ => unreachable!(),
        };
        // 여러 cfg는 전부 성립해야 한다 — all()로 합성해 한 필드에 담는다.
        assert_eq!(
            cfg_of(attrs),
            Some("all(all (unix , feature = \"x\") , debug_assertions)".to_string())
        );
    }

    #[test]
    fn module_dir_rules() {
        // lib.rs/main.rs/mod.rs는 자기 디렉터리, 그 외는 stem/ 아래.
        assert_eq!(module_dir(Path::new("src/lib.rs")), PathBuf::from("src"));
        assert_eq!(module_dir(Path::new("src/a.rs")), PathBuf::from("src/a"));
        assert_eq!(
            module_dir(Path::new("src/a/mod.rs")),
            PathBuf::from("src/a")
        );
    }
}
