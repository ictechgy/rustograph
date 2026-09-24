//! isthmus `bridge-facts` v1 — persistence 도메인의 Rust 생산자다.
//!
//! 계약의 정본은 ../isthmus의 docs/GRAPH-EXCHANGE.md다. 이 모듈은
//! 수확만 한다 — 이름 해석(한정·비한정·모호성)과 진단은 isthmus의 몫이다.
//!
//! 수확 범위는 이름 기반이다(타입 해석 없이):
//!   - SQL 형태의 문자열 리터럴 전부 — 변수에 담겨 호출로 이어지는 쿼리도
//!     문자열 자체가 관계 참조이므로 위치와 함께 낸다
//!   - `sqlx::query*` 계열 매크로·함수 호출의 SQL 인자 — 리터럴이 아니면
//!     원문을 실은 dynamic 사실로 보존한다
//!   - `sqlx::query*_file!` 계열은 SQL이 파일에 있으므로 dynamic 사실로 센다
//!   - diesel `table!` 매크로의 관계·컬럼 선언 참조
//!   - `#[diesel(table_name = ..)]`·`#[sea_orm(table_name = "..")]` 구조체와
//!     그 필드(또는 `column_name`/`sqlx::rename` 재명명)의 컬럼 참조
//!   - diesel DSL 경로 — `x::table`, `x::dsl::y`, `x::columns::y` 꼴.
//!     x가 워크스페이스의 `table!`·`table_name` 선언 이름과 맞을 때만
//!     정적 사실로 읽고, 아니면 동적 사실로 남긴다
//!
//! 한정되지 않은 이름(`query!`, `sql_query` 등)은 그 파일이 같은 크레이트에서
//! 해당 이름을 import할 때만 인정한다 — 이름만 같은 다른 크레이트 API를
//! 관계 참조로 오독하지 않기 위해서다. 산문 속 "update the .."·"into main"
//! 같은 키워드 모양은 문맥 규칙으로 걸러낸다.
//! 리터럴로 읽히지 않는 SQL 인자는 버리지 않고 dynamic 사실로 보존한다 —
//! 조인하지 못하는 이유를 소비자가 셀 수 있어야 한다.

use crate::cargo_meta;
use proc_macro2::{Span, TokenStream, TokenTree};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::visit::Visit;
use syn::{Expr, LitStr, Meta, Token};

/// isthmus bridge-facts v1의 relation-use 사실이다.
/// 키 순서는 계약 문서의 나열 순서를 따라 diff 가능하게 유지한다.
#[derive(Serialize)]
pub struct RelationFact {
    pub kind: &'static str,
    pub channel: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    pub dynamic: bool,
    pub location: BridgeLocation,
}

/// 계약의 1 기반 소스 위치다 — 열은 UTF-16 코드 단위다.
#[derive(Serialize, Clone)]
pub struct BridgeLocation {
    pub path: String,
    pub line: u32,
    pub column: u32,
}

/// 문서를 생산한 도구 식별자다.
#[derive(Serialize)]
pub struct BridgeFactsTool {
    pub name: &'static str,
    pub version: String,
}

/// isthmus bridge-facts v1 문서다. 키 순서는 계약 문서의 나열 순서를 따른다.
#[derive(Serialize)]
pub struct BridgeFactsDocument {
    pub format: &'static str,
    pub version: u8,
    pub tool: BridgeFactsTool,
    #[serde(rename = "generatedAt")]
    pub generated_at: String,
    #[serde(rename = "sourceModifiedAt", skip_serializing_if = "Option::is_none")]
    pub source_modified_at: Option<String>,
    pub platform: &'static str,
    /// 계약: target은 사실이 있을 때만 `persistence`다 — 빈 문서는 null.
    pub target: Option<&'static str>,
    pub project: String,
    pub facts: Vec<RelationFact>,
    /// 계약상 항상 배열이다 — 비어 있어도 키를 생략하면 파서가 거부한다.
    pub limitations: Vec<String>,
}

/// `dir`의 cargo 워크스페이스 아래 Rust 소스를 스캔해 persistence target의
/// bridge-facts 문서를 만든다.
/// 워크스페이스를 못 읽으면 오류다 — 빈 문서로 성공한 척하지 않는다.
pub fn facts(dir: &Path, tool_version: &str) -> Result<BridgeFactsDocument, String> {
    let root = dir
        .canonicalize()
        .map_err(|e| format!("cannot resolve {}: {e}", dir.display()))?;
    let meta = cargo_meta::load(dir)?;
    let mut scan = SchemaScan {
        root: root.clone(),
        ..Default::default()
    };
    scan.limitations.extend(meta.limitations.iter().cloned());
    let mut files: Vec<(PathBuf, String, syn::File)> = Vec::new();
    for file in workspace_rs_files(&meta) {
        let Ok(src) = std::fs::read_to_string(&file) else {
            scan.unparsed += 1;
            continue;
        };
        if let Ok(modified) = std::fs::metadata(&file).and_then(|m| m.modified()) {
            if scan.latest.is_none_or(|t| modified > t) {
                scan.latest = Some(modified);
            }
        }
        match syn::parse_file(&src) {
            Ok(ast) => files.push((file, src, ast)),
            Err(_) => scan.unparsed += 1,
        }
    }
    // 0패스: 워크스페이스 전체의 diesel `table!`·`table_name` 선언 이름을
    // 모은다 — `x::table`·`x::dsl::y` 경로의 x를 이 목록으로만 관계에
    // 귀속해 같은 모양의 비diesel 경로를 정적 사실로 오독하지 않는다.
    for (_, _, ast) in &files {
        collect_diesel_names(ast, &mut scan.diesel_tables);
    }
    for (file, src, ast) in &files {
        scan_file(&mut scan, file, src, ast);
    }
    let mut facts = std::mem::take(&mut scan.list);
    facts.sort_by(fact_cmp);
    let limitations = scan.limitations();
    Ok(BridgeFactsDocument {
        format: "bridge-facts",
        version: 1,
        tool: BridgeFactsTool {
            name: "rustograph",
            version: tool_version.to_string(),
        },
        generated_at: rfc3339_utc_now(),
        source_modified_at: scan.latest.map(rfc3339_utc),
        platform: "rust",
        target: if facts.is_empty() {
            None
        } else {
            Some("persistence")
        },
        project: root.display().to_string(),
        facts,
        limitations,
    })
}

/// 워크스페이스 멤버 각 패키지 루트 아래의 .rs 파일 목록이다 —
/// src·tests·examples를 모두 커버하되 `target`과 숨김 디렉터리는 건너뛴다.
/// cargo가 아는 패키지만 스캔해 벤더·빌드 산출물을 제외한다.
fn workspace_rs_files(meta: &cargo_meta::Metadata) -> Vec<PathBuf> {
    let mut roots: BTreeSet<PathBuf> = BTreeSet::new();
    for p in meta.packages.iter().filter(|p| p.workspace_member) {
        for t in &p.targets {
            if let Some(root) = package_root(&t.src) {
                roots.insert(root);
            }
        }
    }
    let mut out: BTreeSet<PathBuf> = BTreeSet::new();
    for root in roots {
        walk_rs(&root, &mut out);
    }
    out.into_iter().collect()
}

/// 타깃 소스의 조상 중 Cargo.toml을 가진 디렉터리가 패키지 루트다.
fn package_root(target_src: &Path) -> Option<PathBuf> {
    for dir in target_src.ancestors().skip(1) {
        if dir.join("Cargo.toml").exists() {
            return Some(dir.to_path_buf());
        }
    }
    None
}

/// 디렉터리를 재귀로 훑어 .rs를 모은다 — `target`·숨김 디렉터리 제외.
fn walk_rs(dir: &Path, out: &mut BTreeSet<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name();
        let name = name.to_string_lossy();
        if p.is_dir() {
            if name != "target" && !name.starts_with('.') {
                walk_rs(&p, out);
            }
        } else if p.extension().is_some_and(|x| x == "rs") {
            if let Ok(c) = p.canonicalize() {
                out.insert(c);
            }
        }
    }
}

/// sqlx 계열 매크로의 SQL 인자 위치다 — `query_as` 계열은 첫 인자가
/// 결과 타입이라 SQL은 두 번째다.
fn sqlx_macro_arg(name: &str) -> Option<usize> {
    match name {
        "query" | "query_unchecked" | "query_scalar" | "query_scalar_unchecked" => Some(0),
        "query_as" | "query_as_unchecked" => Some(1),
        _ => None,
    }
}

/// SQL이 파일에 있는 sqlx 매크로다 — 내용을 읽을 수 없어 dynamic으로 센다.
/// 값은 SQL 파일 경로 인자의 위치다 — `query_file_as` 계열은 첫 인자가
/// 결과 타입이라 경로는 두 번째다.
fn sqlx_file_macro_arg(name: &str) -> Option<usize> {
    match name {
        "query_file"
        | "query_file_unchecked"
        | "query_scalar_file"
        | "query_scalar_file_unchecked" => Some(0),
        "query_file_as" | "query_file_as_unchecked" => Some(1),
        _ => None,
    }
}

/// SQL 텍스트를 첫 인자로 받는 sqlx 경로 함수다.
const SQLX_SQL_FNS: &[&str] = &[
    "query",
    "query_as",
    "query_unchecked",
    "query_as_unchecked",
    "query_scalar",
    "query_scalar_unchecked",
    "raw_sql",
];

/// diesel의 SQL 텍스트 함수다 — `sql_query`는 import해 쓰는 것이 관례다.
const DIESEL_SQL_FNS: &[&str] = &["sql_query"];

/// persistence 수확의 중간 상태다.
#[derive(Default)]
struct SchemaScan {
    root: PathBuf,
    list: Vec<RelationFact>,
    limitations: Vec<String>,
    unparsed: usize,                       // 파싱·읽기에 실패한 .rs 수
    unparsed_table_macros: usize,          // table! 문법에 맞지 않은 매크로 수
    unattributed: usize,                   // 관계를 알 수 없는 컬럼 어트리뷰트 수
    unresolved_diesel: usize,              // 선언 이름과 맞지 않는 DSL 모양 경로 수
    unlocated: usize,                      // span을 위치로 변환하지 못해 버린 사실 수
    dynamic: usize,                        // 리터럴로 읽히지 않아 조인 불가한 SQL 인자 수
    diesel_tables: BTreeSet<String>,       // 워크스페이스의 diesel 선언 이름
    latest: Option<std::time::SystemTime>, // 읽은 소스의 최신 mtime
    seen: BTreeSet<String>,
}

/// `use` 트리에서 모은 sqlx·diesel 바인딩 집합이다.
#[derive(Default)]
struct UseBindings {
    /// `use sqlx::x`·`use diesel::x`의 바인딩 → (크레이트, 원래 이름).
    /// `as` 별칭은 키가 별칭이고 값은 원래 이름이다 — 매크로·함수 규칙은
    /// 원래 이름으로 찾아야 `use sqlx::query as q`도 query의 규칙을 따른다.
    db_imports: BTreeMap<String, (&'static str, String)>,
    /// `use sqlx as db` 같은 크레이트 별칭 → 크레이트 이름.
    /// 한정 경로의 첫 세그먼트를 정규화할 때만 쓰고, 별칭 자체를
    /// 매크로·함수 이름으로 풀지는 않는다.
    crate_aliases: BTreeMap<String, &'static str>,
    sqlx_glob: bool,   // `use sqlx::...::*`
    diesel_glob: bool, // `use diesel::...::*`
}

/// 한 파일의 사실을 모은다 — import 집합을 먼저 채우고(1패스) 사실을
/// 읽는다(2패스). 본문 안의 `use`도 뒤의 사용처를 위해 미리 모은다.
fn scan_file(scan: &mut SchemaScan, file: &Path, src: &str, ast: &syn::File) {
    let mut ctx = FileCtx {
        scan,
        src,
        file,
        binds: UseBindings::default(),
        pass_two: false,
    };
    ctx.visit_file(ast); // 1패스: use 수집
    ctx.pass_two = true;
    ctx.visit_file(ast); // 2패스: 사실 수확
}

/// 파일 단위 스캔 문맥 — sqlx·diesel에서 import한 이름과 누적 상태.
struct FileCtx<'a> {
    scan: &'a mut SchemaScan,
    src: &'a str,
    file: &'a Path,
    binds: UseBindings,
    pass_two: bool,
}

impl<'ast> Visit<'ast> for FileCtx<'ast> {
    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        let mut prefix = Vec::new();
        collect_use(&node.tree, &mut prefix, &mut self.binds);
    }

    fn visit_macro(&mut self, node: &'ast syn::Macro) {
        if !self.pass_two {
            return;
        }
        if self.scan_table_macro(node) || self.scan_sqlx_macro(node) {
            return;
        }
        // 알려진 매크로가 아니면 토큰 안의 SQL 리터럴만 훑는다 —
        // format!("DELETE FROM {}", t) 같은 템플릿도 관계 참조다.
        scan_macro_literals(self.scan, node, self.src, self.file);
    }

    fn visit_expr_call(&mut self, node: &'ast syn::ExprCall) {
        if self.pass_two {
            self.scan_sql_call(node);
        }
        syn::visit::visit_expr_call(self, node);
    }

    fn visit_expr_path(&mut self, node: &'ast syn::ExprPath) {
        if self.pass_two {
            self.scan_diesel_path(&node.path);
        }
        syn::visit::visit_expr_path(self, node);
    }

    fn visit_type_path(&mut self, node: &'ast syn::TypePath) {
        // diesel의 생성 타입 경로(`Select<users::table>` 안의 users::table)도
        // 같은 DSL 모양이다 — 식 위치와 같은 규칙으로 읽는다.
        if self.pass_two {
            self.scan_diesel_path(&node.path);
        }
        syn::visit::visit_type_path(self, node);
    }

    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        if self.pass_two {
            self.scan_model_struct(node);
        }
        syn::visit::visit_item_struct(self, node);
    }

    fn visit_lit_str(&mut self, node: &'ast LitStr) {
        if self.pass_two {
            // 맥락 없는 리터럴은 산문 오탐을 막는 동사 게이트를 적용한다.
            self.scan
                .push_sql_literal(self.scan_locate(node.span()), &node.value(), false);
        }
        syn::visit::visit_lit_str(self, node);
    }
}

/// `use` 트리를 걸어 sqlx·diesel에서 온 이름 바인딩을 모은다.
/// 첫 세그먼트가 sqlx/diesel일 때만 기록한다 — 이름 충돌 판별에 크레이트
/// 출처가 필요해서다. `use sqlx::prelude::*` 같은 중첩 글롭도 첫 세그먼트로
/// 귀속된다. `use sqlx as db` 같은 최상위 별칭은 크레이트 별칭으로 모은다.
fn collect_use(tree: &syn::UseTree, prefix: &mut Vec<String>, binds: &mut UseBindings) {
    let krate = db_crate(prefix.first().map(String::as_str));
    match tree {
        syn::UseTree::Path(p) => {
            prefix.push(p.ident.to_string());
            collect_use(&p.tree, prefix, binds);
            prefix.pop();
        }
        syn::UseTree::Name(n) => {
            if let Some(k) = krate {
                let name = n.ident.to_string();
                binds.db_imports.insert(name.clone(), (k, name));
            }
        }
        syn::UseTree::Rename(r) => {
            if prefix.is_empty() {
                // `use sqlx as db` — 첫 세그먼트 자체가 별칭이 된다.
                if let Some(k) = db_crate(Some(r.ident.to_string().as_str())) {
                    binds.crate_aliases.insert(r.rename.to_string(), k);
                }
            } else if let Some(k) = krate {
                binds
                    .db_imports
                    .insert(r.rename.to_string(), (k, r.ident.to_string()));
            }
        }
        syn::UseTree::Glob(_) => match krate {
            Some("sqlx") => binds.sqlx_glob = true,
            Some("diesel") => binds.diesel_glob = true,
            _ => {}
        },
        syn::UseTree::Group(g) => {
            for t in &g.items {
                collect_use(t, prefix, binds);
            }
        }
    }
}

/// use 첫 세그먼트를 크레이트 이름으로 정규화한다 — sqlx·diesel만 안다.
fn db_crate(first: Option<&str>) -> Option<&'static str> {
    match first {
        Some("sqlx") => Some("sqlx"),
        Some("diesel") => Some("diesel"),
        _ => None,
    }
}

/// 비한정 이름을 바인딩으로 푼다 — `kind`가 "macro"면 매크로 표,
/// "fn"이면 함수 표로 글롭 근거를 가른다.
fn resolve_use_name(
    name: &str,
    binds: &UseBindings,
    macro_position: bool,
) -> Option<(&'static str, String)> {
    if let Some((k, orig)) = binds.db_imports.get(name) {
        return Some((*k, orig.clone()));
    }
    if binds.sqlx_glob {
        let known = if macro_position {
            sqlx_macro_arg(name).is_some() || sqlx_file_macro_arg(name).is_some()
        } else {
            SQLX_SQL_FNS.contains(&name)
        };
        if known {
            return Some(("sqlx", name.to_string()));
        }
    }
    if binds.diesel_glob {
        let known = if macro_position {
            name == "table"
        } else {
            DIESEL_SQL_FNS.contains(&name)
        };
        if known {
            return Some(("diesel", name.to_string()));
        }
    }
    None
}

/// 한정 경로의 첫 세그먼트를 크레이트 이름으로 정규화한다 —
/// `use sqlx as db` 별칭만 바꾸고 나머지는 그대로 둔다.
fn crate_name<'a>(seg: &'a str, binds: &UseBindings) -> &'a str {
    binds.crate_aliases.get(seg).copied().unwrap_or(seg)
}

impl<'ast> FileCtx<'ast> {
    /// 비한정 매크로 이름을 (크레이트, 원래 이름)으로 푼다.
    /// 글롭은 그 크레이트가 실제로 가진 이름일 때만 근거가 된다 —
    /// `use diesel::*`가 sqlx 매크로 이름을 열어주지 않게 한다.
    fn resolve_macro(&self, name: &str) -> Option<(&'static str, String)> {
        resolve_use_name(name, &self.binds, true)
    }

    /// 비한정 함수 이름을 (크레이트, 원래 이름)으로 푼다 — 글롭은 그
    /// 크레이트의 SQL 함수 목록에 있는 이름에만 적용된다.
    fn resolve_fn(&self, name: &str) -> Option<(&'static str, String)> {
        resolve_use_name(name, &self.binds, false)
    }

    /// `table!`/`diesel::table!` 매크로를 읽는다. 처리한 매크로면 true다.
    fn scan_table_macro(&mut self, m: &syn::Macro) -> bool {
        let segs: Vec<String> = m
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect();
        if segs.is_empty() {
            return false;
        }
        // 비한정 `table!`도 받되 바인딩이 diesel을 가리킬 때만이다 —
        // `use diesel::table`·`use diesel::*`·`use diesel::table as t`
        // 모두 여기로 온다. 출처를 모르는 같은 이름의 매크로는 건드리지
        // 않는다.
        let owned = match segs.as_slice() {
            [name] => {
                matches!(self.resolve_macro(name), Some(("diesel", orig)) if orig == "table")
            }
            [krate, name] => crate_name(krate, &self.binds) == "diesel" && name == "table",
            _ => false,
        };
        if !owned {
            return false;
        }
        match parse_table_macro(&m.tokens) {
            Some((relation, columns)) => {
                let loc = self.scan_locate(m.path.segments.last().unwrap().ident.span());
                // 위치 없는 사실은 조인기가 쓸 수 없다 — 버린 만큼 센다.
                let loc = match loc {
                    Some(l) => l,
                    None => {
                        self.scan.unlocated += 1 + columns.len();
                        return true;
                    }
                };
                self.scan.push(RelationFact {
                    kind: "relation-use",
                    channel: escape_qualified(&relation),
                    method: None,
                    dynamic: false,
                    location: loc.clone(),
                });
                for (col, span) in columns {
                    match self.scan_locate(span) {
                        Some(cloc) => self.scan.push(RelationFact {
                            kind: "relation-use",
                            channel: escape_qualified(&relation),
                            method: Some(col),
                            dynamic: false,
                            location: cloc,
                        }),
                        None => self.scan.unlocated += 1,
                    }
                }
            }
            None => self.scan.unparsed_table_macros += 1,
        }
        true
    }

    /// `sqlx::query!` 계열 매크로의 SQL 인자를 읽는다. 처리한 매크로면 true다.
    fn scan_sqlx_macro(&mut self, m: &syn::Macro) -> bool {
        let segs: Vec<String> = m
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect();
        if segs.is_empty() {
            return false;
        }
        // 비한정·별칭 매크로는 바인딩의 크레이트가 sqlx일 때만 sqlx 규칙을
        // 적용한다 — `use diesel::x as q`의 q!는 sqlx가 아니다. 한정 경로의
        // 첫 세그먼트는 크레이트 별칭(`use sqlx as db`)을 풀어 판정한다.
        let name = match segs.as_slice() {
            [single] => match self.resolve_macro(single) {
                Some(("sqlx", orig)) => orig,
                _ => return false,
            },
            [krate, leaf] if crate_name(krate, &self.binds) == "sqlx" => leaf.clone(),
            _ => return false,
        };
        let file_arg = sqlx_file_macro_arg(&name);
        let sql_arg = sqlx_macro_arg(&name);
        if file_arg.is_none() && sql_arg.is_none() {
            return false;
        }
        // 위치 표시는 매크로 이름 지점 — 인자 span은 파일 SQL 위치를 못 가리킨다.
        let loc = self
            .scan_locate(m.path.segments.last().unwrap().ident.span())
            .or_else(|| self.scan_locate(m.path.span()));
        let args = split_top_level_commas(&m.tokens);
        let is_file = file_arg.is_some();
        let pick = file_arg.or(sql_arg);
        let expr = pick
            .and_then(|i| args.get(i))
            .and_then(|ts| syn::parse2::<Expr>(ts.clone()).ok());
        match (is_file, expr) {
            // 파일 매크로는 SQL이 파일에 있으므로 항상 dynamic이다 — 채널에는
            // 경로 리터럴의 원문을 실어 어느 호출인지 남긴다.
            (true, Some(e)) => self.scan.push_dynamic(self.src, &e, loc, self.file),
            (true, None) => match loc {
                Some(l) => self.scan.push_dynamic_text(l),
                None => self.scan.unlocated += 1,
            },
            (false, Some(Expr::Lit(el))) => {
                if let syn::Lit::Str(lit) = &el.lit {
                    // 확인된 sqlx 인자 자리의 리터럴은 SQL 컨텍스트가
                    // 확정됐다 — 동사 게이트를 건너뛴다.
                    self.scan.push_sql_literal(
                        self.scan_locate(lit.span()).or(loc),
                        &lit.value(),
                        true,
                    );
                } else {
                    self.scan
                        .push_dynamic(self.src, &Expr::Lit(el.clone()), loc, self.file);
                }
            }
            (false, Some(e)) => self.scan.push_dynamic(self.src, &e, loc, self.file),
            (false, None) => match loc {
                Some(l) => self.scan.push_dynamic_text(l),
                None => self.scan.unlocated += 1,
            },
        }
        true
    }

    /// `sqlx::query("...")`·`diesel::sql_query("...")`·import된 비한정 호출의
    /// SQL 인자를 본다 — 리터럴은 lit 스캔이 따로 잡으므로 여기서는
    /// 비리터럴을 dynamic 사실로 보존한다.
    fn scan_sql_call(&mut self, call: &syn::ExprCall) {
        let syn::Expr::Path(fp) = call.func.as_ref() else {
            return;
        };
        let segs: Vec<String> = fp
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect();
        if segs.is_empty() {
            return;
        }
        // 비한정 이름은 바인딩의 크레이트로 규칙을 고른다 — sqlx로 확인된
        // 이름은 sqlx 함수 표를, diesel로 확인된 이름은 diesel 표를 본다.
        // 한정 경로의 첫 세그먼트는 크레이트 별칭을 풀어 판정한다.
        let owned = match segs.as_slice() {
            [name] => match self.resolve_fn(name) {
                Some(("sqlx", orig)) => SQLX_SQL_FNS.contains(&orig.as_str()),
                Some(("diesel", orig)) => DIESEL_SQL_FNS.contains(&orig.as_str()),
                _ => false,
            },
            [krate, name] => {
                let krate = crate_name(krate, &self.binds);
                (krate == "sqlx" && SQLX_SQL_FNS.contains(&name.as_str()))
                    || (krate == "diesel" && DIESEL_SQL_FNS.contains(&name.as_str()))
            }
            _ => false,
        };
        if !owned {
            return;
        }
        if let Some(arg) = call.args.first() {
            match arg {
                // 확인된 SQL 인자 자리의 리터럴은 동사 게이트를 건너뛴다 —
                // SET·GRANT 같은 비관계 동사 구문도 스캔 대상이다.
                Expr::Lit(el) => {
                    if let syn::Lit::Str(lit) = &el.lit {
                        let loc = self
                            .scan_locate(lit.span())
                            .or_else(|| self.scan_locate(fp.path.span()));
                        self.scan.push_sql_literal(loc, &lit.value(), true);
                    }
                    // 비문자열 리터럴(`query(1)`)은 SQL 근거가 아니다 —
                    // 계약상 유일하게 계수하지 않고 버리는 예외다.
                }
                e => {
                    let loc = self.scan_locate(fp.path.span());
                    self.scan.push_dynamic(self.src, e, loc, self.file);
                }
            }
        }
    }

    /// diesel DSL 모양의 경로를 읽는다 — `x::table`은 관계 x,
    /// `x::dsl::y`·`x::columns::y`는 관계 x의 컬럼 y다. 식 위치와 타입
    /// 위치(`Select<users::table>` 같은) 모두 같은 규칙으로 읽는다.
    fn scan_diesel_path(&mut self, path: &syn::Path) {
        let segs: Vec<String> = path
            .segments
            .iter()
            .map(|s| s.ident.to_string().trim_start_matches("r#").to_string())
            .collect();
        let loc_of = |i: usize| {
            self.scan
                .locate(self.src, path.segments[i].ident.span(), self.file)
        };
        // `x::dsl::table`처럼 dsl·columns 아래의 `table`은 컬럼 이름이다 —
        // 끝 세그먼트가 table인 팔보다 이 팔을 먼저 맞춰야 한다.
        let (table, column, loc_idx) = match segs.as_slice() {
            [.., table, middle, col] if middle == "dsl" || middle == "columns" => {
                (table, Some(col.clone()), segs.len() - 2)
            }
            [.., table, tail] if tail == "table" => (table, None, segs.len() - 1),
            _ => return,
        };
        let Some(loc) = loc_of(loc_idx) else {
            self.scan.unlocated += 1;
            return;
        };
        if !self.scan.diesel_tables.contains(table) {
            // 선언된 table! 이름과 맞지 않는 같은 모양의 경로는 관계를
            // 확정하지 않고 동적 근거로만 남긴다 — 모듈이 다른 크레이트나
            // 매크로 생성물에서 왔을 수 있다.
            self.scan.unresolved_diesel += 1;
            self.scan.push_dynamic_str(&segs.join("::"), loc);
            return;
        }
        self.scan.push(RelationFact {
            kind: "relation-use",
            channel: escape_name(table),
            method: None,
            dynamic: false,
            location: loc.clone(),
        });
        if let Some(col) = column {
            self.scan.push(RelationFact {
                kind: "relation-use",
                channel: escape_name(table),
                method: Some(col),
                dynamic: false,
                location: loc,
            });
        }
    }

    /// `#[diesel(table_name = x)]`·`#[sea_orm(table_name = "x")]` 구조체를
    /// 읽어 관계 참조와 필드별 컬럼 참조를 낸다.
    /// 관계 바인딩 없는 `column_name`/`sqlx::rename`은 귀속 불가 수로 센다.
    fn scan_model_struct(&mut self, st: &syn::ItemStruct) {
        // diesel·sea_orm 어트리뷰트가 한 구조체에 섞이면 마지막 것이
        // 이긴다 — 실제로는 한 ORM만 쓰는 것이 관례라 별도 귀속은 하지 않는다.
        let mut table: Option<MetaVal> = None;
        for attr in &st.attrs {
            if let Some(v) = meta_name_value(attr, &["diesel", "sea_orm"], "table_name") {
                table = Some(v);
            }
        }
        let Some(table) = table else {
            // 바인딩 없는 컬럼 어트리뷰트만 세어 둔다.
            for f in &st.fields {
                let col = f.attrs.iter().any(|a| {
                    meta_name_value(a, &["diesel", "sea_orm"], "column_name").is_some()
                        || meta_name_value(a, &["sqlx"], "rename").is_some()
                });
                if col {
                    self.scan.unattributed += 1;
                }
            }
            return;
        };
        let Some(rloc) = self
            .scan_locate(table.span)
            .or_else(|| self.scan_locate(st.ident.span()))
        else {
            self.scan.unlocated += 1;
            return;
        };
        // 문자열 table_name은 이름 그 자체다 — `table_name = "a.b"`는 한
        // 식별자지 한정자가 아니라 세그먼트 전체를 escape한다. 경로값
        // (`table_name = schema::name`)만 한정 이름으로 읽는다.
        let channel = if table.literal {
            escape_name(&table.text)
        } else {
            escape_qualified(&table.text)
        };
        // sea-orm은 어트리뷰트 없는 필드를 snake_case 컬럼으로 매핑하고,
        // diesel은 필드 이름을 그대로 컬럼으로 쓴다 — 규약이 다르다.
        let sea_orm = table.krate == "sea_orm";
        self.scan.push(RelationFact {
            kind: "relation-use",
            channel: channel.clone(),
            method: None,
            dynamic: false,
            location: rloc,
        });
        for f in &st.fields {
            let mut col: Option<MetaVal> = None;
            for attr in &f.attrs {
                if let Some(v) = meta_name_value(attr, &["diesel", "sea_orm"], "column_name")
                    .or_else(|| meta_name_value(attr, &["sqlx"], "rename"))
                {
                    col = Some(v);
                }
            }
            let (column, col_span) = match col {
                Some(v) => (v.text, Some(v.span)),
                None => match &f.ident {
                    Some(i) => {
                        let raw = i.to_string().trim_start_matches("r#").to_string();
                        (
                            if sea_orm { to_snake_case(&raw) } else { raw },
                            Some(i.span()),
                        )
                    }
                    None => continue,
                },
            };
            if column.is_empty() {
                continue;
            }
            match col_span.and_then(|s| self.scan_locate(s)) {
                Some(cloc) => self.scan.push(RelationFact {
                    kind: "relation-use",
                    channel: channel.clone(),
                    method: Some(column),
                    dynamic: false,
                    location: cloc,
                }),
                None => self.scan.unlocated += 1,
            }
        }
    }

    /// 토큰 span을 프로젝트 상대의 계약 위치로 바꾼다.
    /// 프로젝트 밖 파일은 상대 경로가 없어 사실로 만들지 않는다.
    fn scan_locate(&self, span: Span) -> Option<BridgeLocation> {
        self.scan.locate(self.src, span, self.file)
    }
}

/// `#[krate(key = value)]` 메타에서 읽은 값이다.
struct MetaVal {
    text: String,
    span: Span,
    /// 문자열 리터럴이면 true — 경로값(`schema::name`)과 escape 규칙이
    /// 다르다: 문자열은 이름 그 자체, 경로는 한정 이름이다.
    literal: bool,
    /// 값을 실은 어트리뷰트의 크레이트 — sea_orm은 컬럼 명명 규약이 다르다.
    krate: &'static str,
}

/// `#[krate(key = value)]` 형태의 중첩 메타에서 값을 읽는다.
/// 식별자·경로·문자열 모두 받는다.
fn meta_name_value(attr: &syn::Attribute, crates: &[&'static str], key: &str) -> Option<MetaVal> {
    let seg = attr.path().segments.first()?.ident.to_string();
    let krate = *crates.iter().find(|k| **k == seg)?;
    let Meta::List(list) = &attr.meta else {
        return None;
    };
    let items = list
        .parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
        .ok()?;
    for item in items {
        let Meta::NameValue(nv) = item else {
            continue;
        };
        if !nv.path.is_ident(key) {
            continue;
        }
        return match &nv.value {
            Expr::Lit(el) => match &el.lit {
                syn::Lit::Str(s) => Some(MetaVal {
                    text: s.value(),
                    span: s.span(),
                    literal: true,
                    krate,
                }),
                _ => None,
            },
            Expr::Path(p) => {
                let name = p
                    .path
                    .segments
                    .iter()
                    .map(|s| s.ident.to_string().trim_start_matches("r#").to_string())
                    .collect::<Vec<_>>()
                    .join(".");
                Some(MetaVal {
                    text: name,
                    span: p.path.segments.last()?.ident.span(),
                    literal: false,
                    krate,
                })
            }
            _ => None,
        };
    }
    None
}

/// 워크스페이스 파일에서 diesel 관계 선언 이름을 모은다 — `table!`의
/// 마지막 세그먼트(dsl 모듈 이름)와 diesel `table_name` 어트리뷰트 값이
/// `x::table`·`x::dsl::y` 경로의 귀속 목록이다. sea_orm 선언은 DSL 모듈을
/// 만들지 않으므로 이 목록에 넣지 않는다. import 수집이 먼저(1패스)여야
/// `use diesel::table as t` 같은 별칭 `table!`도 인식된다.
fn collect_diesel_names(ast: &syn::File, out: &mut BTreeSet<String>) {
    struct Names<'a> {
        out: &'a mut BTreeSet<String>,
        binds: UseBindings,
        pass_two: bool,
    }
    impl<'ast> Visit<'ast> for Names<'_> {
        fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
            let mut prefix = Vec::new();
            collect_use(&node.tree, &mut prefix, &mut self.binds);
        }
        fn visit_macro(&mut self, m: &'ast syn::Macro) {
            if self.pass_two {
                let segs: Vec<String> = m
                    .path
                    .segments
                    .iter()
                    .map(|s| s.ident.to_string())
                    .collect();
                let owned = match segs.as_slice() {
                    [name] => matches!(
                        resolve_use_name(name, &self.binds, true),
                        Some(("diesel", orig)) if orig == "table"
                    ),
                    [krate, name] => crate_name(krate, &self.binds) == "diesel" && name == "table",
                    _ => false,
                };
                if owned {
                    if let Some((rel, _)) = parse_table_macro(&m.tokens) {
                        if let Some(last) = rel.rsplit('.').next() {
                            self.out.insert(last.to_string());
                        }
                    }
                }
            }
            syn::visit::visit_macro(self, m);
        }
        fn visit_item_struct(&mut self, st: &'ast syn::ItemStruct) {
            if self.pass_two {
                for attr in &st.attrs {
                    if let Some(v) = meta_name_value(attr, &["diesel"], "table_name") {
                        if let Some(last) = v.text.rsplit('.').next() {
                            self.out.insert(last.to_string());
                        }
                    }
                }
            }
            syn::visit::visit_item_struct(self, st);
        }
    }
    let mut names = Names {
        out,
        binds: UseBindings::default(),
        pass_two: false,
    };
    names.visit_file(ast);
    names.pass_two = true;
    names.visit_file(ast);
}

/// diesel `table!` 매크로 본문을 읽어 (관계 이름, [(컬럼, span)])을 돌려준다.
/// 문법: `[schema.]name (pk, ...) { col -> Type, ... }` — 앞의 `use ...;`
/// 항목과 컬럼의 `#[...]` 어트리뷰트는 건너뛴다. 맞지 않으면 None이다.
fn parse_table_macro(tokens: &TokenStream) -> Option<(String, Vec<(String, Span)>)> {
    let mut tts: Vec<TokenTree> = tokens.clone().into_iter().collect();
    // 맨 앞의 `use ...;` 항목을 건너뛴다 — diesel sql_types import 관례다.
    loop {
        match tts.first() {
            Some(TokenTree::Ident(i)) if *i == "use" => {
                let mut end = 0;
                while end < tts.len() {
                    if matches!(&tts[end], TokenTree::Punct(p) if p.as_char() == ';') {
                        break;
                    }
                    end += 1;
                }
                if end >= tts.len() {
                    return None;
                }
                tts.drain(..=end);
            }
            _ => break,
        }
    }
    let mut it = tts.into_iter().peekable();
    let mut segments = vec![take_ident(&mut it)?];
    while matches!(it.peek(), Some(TokenTree::Punct(p)) if p.as_char() == '.') {
        it.next();
        segments.push(take_ident(&mut it)?);
    }
    let relation = segments.join(".");
    // 기본키 그룹 `(id)`은 내용을 읽지 않고 건너뛴다.
    match it.next() {
        Some(TokenTree::Group(g)) if g.delimiter() == proc_macro2::Delimiter::Parenthesis => {}
        Some(TokenTree::Group(g)) if g.delimiter() == proc_macro2::Delimiter::Brace => {
            return Some((relation, parse_table_columns(&g.stream())));
        }
        _ => return None,
    }
    match it.next() {
        Some(TokenTree::Group(g)) if g.delimiter() == proc_macro2::Delimiter::Brace => {
            Some((relation, parse_table_columns(&g.stream())))
        }
        _ => None,
    }
}

/// `table!`의 컬럼 블록 `{ id -> Int4, name -> Text }`을 읽는다.
/// `#[sql_name]` 같은 어트리뷰트는 건너뛰고 `ident ->` 쌍만 모은다.
fn parse_table_columns(stream: &TokenStream) -> Vec<(String, Span)> {
    let mut out = Vec::new();
    let mut it = stream.clone().into_iter().peekable();
    while let Some(tt) = it.next() {
        match tt {
            TokenTree::Punct(p) if p.as_char() == '#' => {
                // `#[..]` 어트리뷰트 — 다음 그룹을 건너뛴다.
                if matches!(it.peek(), Some(TokenTree::Group(g)) if g.delimiter() == proc_macro2::Delimiter::Bracket)
                {
                    it.next();
                }
            }
            TokenTree::Ident(id) => {
                if matches!(it.peek(), Some(TokenTree::Punct(p)) if p.as_char() == '-') {
                    // `ident -> Type` — `-` 다음 `>` 소비.
                    it.next();
                    if matches!(it.peek(), Some(TokenTree::Punct(p)) if p.as_char() == '>') {
                        it.next();
                    }
                    out.push((
                        id.to_string().trim_start_matches("r#").to_string(),
                        id.span(),
                    ));
                }
            }
            _ => {}
        }
    }
    out
}

/// 토큰 트리의 맨 앞 식별자를 꺼낸다 — raw 식별자의 `r#`는 벗긴다.
fn take_ident(it: &mut std::iter::Peekable<std::vec::IntoIter<TokenTree>>) -> Option<String> {
    match it.next() {
        Some(TokenTree::Ident(id)) => Some(id.to_string().trim_start_matches("r#").to_string()),
        _ => None,
    }
}

/// 매크로 호출 인자를 최상위 쉼표로 나눈다 — 그룹 안의 쉼표는 세지 않는다.
fn split_top_level_commas(tokens: &TokenStream) -> Vec<TokenStream> {
    let mut out = Vec::new();
    let mut cur = TokenStream::new();
    for tt in tokens.clone() {
        match tt {
            TokenTree::Punct(p)
                if p.as_char() == ',' && p.spacing() == proc_macro2::Spacing::Alone =>
            {
                out.push(std::mem::take(&mut cur));
            }
            other => cur.extend([other]),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// 알려지지 않은 매크로의 토큰 안에서 SQL 리터럴을 찾는다 —
/// format!·const concat 같은 템플릿 속 SQL도 관계 참조다.
fn scan_macro_literals(scan: &mut SchemaScan, m: &syn::Macro, src: &str, file: &Path) {
    let mut stack: Vec<TokenTree> = m.tokens.clone().into_iter().collect();
    while let Some(tt) = stack.pop() {
        match tt {
            TokenTree::Group(g) => stack.extend(g.stream()),
            TokenTree::Literal(l) => {
                if let Ok(lit) =
                    syn::parse2::<LitStr>(TokenStream::from(TokenTree::Literal(l.clone())))
                {
                    // 위치를 못 구한 SQL 리터럴도 push_sql_literal이 센다.
                    scan.push_sql_literal(scan.locate(src, lit.span(), file), &lit.value(), false);
                }
            }
            _ => {}
        }
    }
}

impl SchemaScan {
    /// SQL 형태의 리터럴에서 관계 이름을 읽어 사실로 낸다.
    /// 관계 자리에 플레이스홀더 같은 비리터럴 피연산자가 오면 관계 참조가
    /// 있었다는 근거를 동적 사실로 남긴다 — 조용히 버리지 않는다.
    /// `trusted`는 확인된 DB 인자 자리라는 뜻이다 — 그때는 동사 게이트를
    /// 건너뛴다. 위치를 못 구한 SQL 리터럴은 unlocated로 센다.
    fn push_sql_literal(&mut self, loc: Option<BridgeLocation>, text: &str, trusted: bool) {
        if !trusted && !looks_like_sql(text) {
            return;
        }
        let Some(loc) = loc else {
            self.unlocated += 1;
            return;
        };
        let (names, unresolved) = sql_relations(text);
        for name in names {
            self.push(RelationFact {
                kind: "relation-use",
                channel: name,
                method: None,
                dynamic: false,
                location: loc.clone(),
            });
        }
        if unresolved {
            self.push_dynamic_str(text, loc);
        }
    }

    /// 리터럴로 읽히지 않는 SQL 인자를 동적 사실로 보존한다.
    /// channel에는 잘린 원문 표현식을 실어 어느 위치의 호출인지 남긴다.
    /// 위치를 못 구한 동적 근거는 unlocated로 센다.
    fn push_dynamic(&mut self, src: &str, expr: &Expr, loc: Option<BridgeLocation>, file: &Path) {
        // 비문자열 리터럴 인자(`query!(1)`)는 SQL 근거가 될 수 없다 — 모든
        // 경로를 계수하는 계약에서 유일하게 "근거 없음"으로 버리는 예외다.
        if let Expr::Lit(el) = expr {
            if !matches!(el.lit, syn::Lit::Str(_)) {
                return;
            }
        }
        let Some(loc) = loc.or_else(|| self.locate(src, expr.span(), file)) else {
            self.unlocated += 1;
            return;
        };
        let text = expr_text(src, expr);
        self.push_dynamic_str(&text, loc);
    }

    /// 표현식을 파싱하지 못했을 때의 dynamic 사실 — 매크로 위치를 남긴다.
    fn push_dynamic_text(&mut self, loc: BridgeLocation) {
        self.push_dynamic_str("<unparsed macro argument>", loc);
    }

    /// dynamic 사실 공통 경로 — 원문은 120자로 자른다.
    fn push_dynamic_str(&mut self, text: &str, loc: BridgeLocation) {
        // 문자·바이트 단위를 섞지 않는다 — 실제로 잘렸을 때만 ...를 붙인다.
        let mut chars = text.chars();
        let head: String = chars.by_ref().take(120).collect();
        let channel = if chars.next().is_some() {
            format!("{}...", head.trim_end())
        } else {
            head
        };
        self.push(RelationFact {
            kind: "relation-use",
            channel,
            dynamic: true,
            method: None,
            location: loc,
        });
    }

    /// 위치가 있는 사실만 담고 같은 사실을 한 번만 남긴다.
    fn push(&mut self, fact: RelationFact) {
        let key = format!(
            "{}|{}|{}|{}|{}:{}:{}",
            fact.kind,
            fact.channel,
            fact.method.as_deref().unwrap_or(""),
            fact.dynamic,
            fact.location.path,
            fact.location.line,
            fact.location.column
        );
        if !self.seen.insert(key) {
            return;
        }
        if fact.dynamic {
            self.dynamic += 1;
        }
        self.list.push(fact);
    }

    /// 토큰 span을 프로젝트 상대의 계약 위치로 바꾼다.
    /// proc-macro2의 열은 바이트라 UTF-16 열로 환산한다.
    fn locate(&self, src: &str, span: Span, file: &Path) -> Option<BridgeLocation> {
        let start = span.start();
        if start.line == 0 {
            return None;
        }
        let rel = file.strip_prefix(&self.root).ok()?;
        let line_text = src.lines().nth(start.line - 1)?;
        // 열이 바이트 오프셋이 아니거나 문자 경계와 어긋나면 문자 수로
        // 재해석한다 — 실패 시 0 대신 그만큼의 문자열을 센 값을 쓴다.
        let column = line_text
            .get(..start.column)
            .map(|s| s.chars().map(|c| c.len_utf16() as u32).sum::<u32>())
            .unwrap_or_else(|| {
                line_text
                    .chars()
                    .take(start.column)
                    .map(|c| c.len_utf16() as u32)
                    .sum()
            })
            + 1;
        Some(BridgeLocation {
            path: rel.to_string_lossy().replace('\\', "/"),
            line: start.line as u32,
            column,
        })
    }

    /// 수확에서 실제로 센 공백만 문장으로 낸다.
    fn limitations(&self) -> Vec<String> {
        let mut out = self.limitations.clone();
        if self.unparsed > 0 {
            out.push(format!(
                "unparsed-sources: {} .rs file(s) failed to read or parse; relation uses there are uncounted",
                self.unparsed
            ));
        }
        if self.unparsed_table_macros > 0 {
            out.push(format!(
                "unparsed-table-macros: {} table! invocation(s) did not fit the diesel table grammar",
                self.unparsed_table_macros
            ));
        }
        if self.unattributed > 0 {
            out.push(format!(
                "unattributed-column-tags: {} column attribute(s) had no table_name binding",
                self.unattributed
            ));
        }
        if self.unresolved_diesel > 0 {
            out.push(format!(
                "unresolved-diesel-paths: {} diesel-DSL-shaped path(s) matched no declared table name; kept as dynamic",
                self.unresolved_diesel
            ));
        }
        if self.unlocated > 0 {
            out.push(format!(
                "unlocated-references: {} extracted reference(s) had no resolvable source span",
                self.unlocated
            ));
        }
        if self.dynamic > 0 {
            // isthmus가 미사용 진단을 unverified로 내리는 근거다 — 접두사를
            // 바꾸면 조인기의 severity 계산이 새로 인식하지 못한다.
            out.push(format!(
                "unjoined-dynamic-relations: {} SQL argument(s) or relation operand(s) were not statically readable; their relations are uncounted",
                self.dynamic
            ));
        }
        out.sort();
        out.dedup();
        out
    }
}

/// 사실의 결정적 순서다 — 위치·종류·이름·동적 플래그 순.
fn fact_cmp(a: &RelationFact, b: &RelationFact) -> std::cmp::Ordering {
    (
        &a.location.path,
        a.location.line,
        a.location.column,
        a.kind,
        &a.channel,
        &a.method,
        a.dynamic,
    )
        .cmp(&(
            &b.location.path,
            b.location.line,
            b.location.column,
            b.kind,
            &b.channel,
            &b.method,
            b.dynamic,
        ))
}

/// 표현식의 원문을 span 범위로 잘라낸다 — 토큰 재조합보다 소스 그대로가
/// 진단 단서로 정확하다. 줄 오프셋은 실제 바이트로 계산해 CRLF에서도
/// 어긋나지 않는다.
fn expr_text(src: &str, expr: &Expr) -> String {
    let span = expr.span();
    let (s, e) = (span.start(), span.end());
    if s.line == 0 || e.line == 0 {
        return "<dynamic expression>".to_string();
    }
    let mut starts = vec![0usize];
    for (i, b) in src.bytes().enumerate() {
        if b == b'\n' {
            starts.push(i + 1);
        }
    }
    // 열이 문자 경계와 어긋나면 경계까지 보정한다.
    let get = |line: usize, col: usize| -> usize {
        let mut p = starts.get(line - 1).copied().unwrap_or(src.len()) + col;
        while p < src.len() && !src.is_char_boundary(p) {
            p += 1;
        }
        p.min(src.len())
    };
    let (a, b) = (get(s.line, s.column), get(e.line, e.column));
    src.get(a..b).unwrap_or("<dynamic expression>").to_string()
}

/// 문자열이 SQL로 보이는지 본다 — 강한 동사가 없고 문장 머리가
/// UPDATE·TRUNCATE도 아닌 리터럴은 스캔하지 않아 산문 속
/// "from"·"update"·"into" 같은 오탐을 막는다.
fn looks_like_sql(text: &str) -> bool {
    let mut head = true;
    for t in lex_sql(text) {
        if !is_name_token(&t) {
            continue;
        }
        if is_sql_verb(&t.text) {
            return true;
        }
        if head {
            head = false;
            if t.text.eq_ignore_ascii_case("update") || t.text.eq_ignore_ascii_case("truncate") {
                return true;
            }
        }
    }
    false
}

/// SQL 문을 여는 강한 동사 표다 — 관계 키워드와 겹치는 update·truncate는
/// 뺀다(문장 머리 규칙이 따로 있다). WITH는 SELECT를 동반하므로 없다.
fn is_sql_verb(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "select"
            | "insert"
            | "delete"
            | "create"
            | "alter"
            | "drop"
            | "replace"
            | "merge"
            | "lock"
            | "unlock"
            | "rename"
            | "describe"
            | "desc"
            | "analyze"
            | "vacuum"
            | "grant"
            | "revoke"
    )
}

/// SQL 어휘 하나다 — 인용된 식별자는 키워드가 아니다.
struct SqlToken {
    text: String,
    quoted: bool,
}

/// 뒤따르는 식별자가 관계 이름인 키워드다. `on`은 GRANT/REVOKE 문
/// 안에서만 관계 키워드로 발화한다 — JOIN .. ON의 on은 제외다.
fn is_relation_keyword(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "from" | "join" | "into" | "update" | "table" | "truncate" | "on"
    )
}

/// SQL 텍스트에서 관계 이름을 읽는다.
/// 한정 이름(`schema.table`)은 그대로 두고, 이름 자체에 점이 있는 인용
/// 식별자("a.b")는 한 세그먼트로 읽는다 — escape는 사실 기록 시에 한다.
/// 두 번째 반환은 관계 자리의 피연산자를 읽지 못했음을 뜻한다 —
/// `FROM {}` 같은 플레이스홀더를 사실 없이 조용히 넘기지 않기 위해서다.
fn sql_relations(text: &str) -> (Vec<String>, bool) {
    let tokens = lex_sql(text);
    let mut out = Vec::new();
    let mut seen = BTreeSet::new(); // 겹치는 키워드 창의 중복을 막는다
    let mut consumed = vec![false; tokens.len()]; // 이름·별칭·수식어로 소비된 토큰
    let mut unresolved = false;
    // `;`로 갈리는 각 문장의 머리 식별자 위치와 그 문장의 동사다 —
    // update·truncate는 문장 머리에서만 관계 키워드로 열고, `on`은
    // grant·revoke 문 안에서만 연다. 다중 문장 리터럴의 뒤 문장도
    // 같은 규칙을 받는다.
    let mut stmt_head = vec![false; tokens.len()];
    let mut stmt_verb: Vec<Option<String>> = vec![None; tokens.len()];
    {
        let mut pending = true;
        let mut verb: Option<String> = None;
        for (i, t) in tokens.iter().enumerate() {
            if !t.quoted && t.text == ";" {
                pending = true;
                verb = None;
                continue;
            }
            if is_name_token(t) && pending {
                stmt_head[i] = true;
                verb = Some(t.text.to_ascii_lowercase());
                pending = false;
            }
            stmt_verb[i] = verb.clone();
        }
    }
    for i in 0..tokens.len() {
        let tok = &tokens[i];
        if consumed[i] || tok.quoted || !is_relation_keyword(&tok.text) {
            continue;
        }
        let word = tok.text.to_ascii_lowercase();
        let grant_stmt = matches!(stmt_verb[i].as_deref(), Some("grant" | "revoke"));
        // 같은 문장(`;`로 갈리는 세그먼트) 안만 본다 — 뒤 세그먼트의
        // 단어를 앞 문장의 근거로 쓰지 않는다.
        let segment_before = |end: usize| tokens[..end].iter().rev().take_while(|t| t.text != ";");
        let segment_after_has = |start: usize, w: &str| {
            tokens[start..]
                .iter()
                .take_while(|t| t.text != ";")
                .any(|t| !t.quoted && t.text.eq_ignore_ascii_case(w))
        };
        let fires = match word.as_str() {
            // 산문 속 "update the .."·upsert의 `DO UPDATE SET`을 막기 위해
            // update는 문장 머리이고 같은 문장에 SET이 있을 때만 연다.
            "update" => stmt_head[i] && segment_after_has(i + 1, "set"),
            // truncate는 항상 문장 머리 동사다 — 산문 중간의 "truncate"는 무시.
            "truncate" => stmt_head[i],
            // into는 같은 문장에 INSERT·SELECT·MERGE·REPLACE가 앞선 문맥에서만
            // 연다 — "merged the branch into main" 같은 산문을 막는다.
            // 단, 문장이 "merge"로 시작하는 산문은 SQL `MERGE INTO`와 어휘가
            // 같아 구분 못 한다 — 남은 오탐 여지로 둔다.
            "into" => segment_before(i).any(|t| {
                !t.quoted
                    && matches!(
                        t.text.to_ascii_lowercase().as_str(),
                        "insert" | "select" | "merge" | "replace"
                    )
            }),
            // table은 직전 식별자가 DDL 동사일 때만 키워드다 — 산문의
            // "the table"이나 다른 절의 단어는 읽지 않는다.
            "table" => table_keyword_context(&tokens, i),
            // on은 `GRANT .. ON t`·`REVOKE .. ON t`의 관계 자리다 — 권한
            // 단어(SELECT 등)가 앞서야 "grant access on .." 같은 산문을
            // 막는다. `CREATE INDEX/TRIGGER .. ON t`의 on도 관계 자리다.
            "on" => {
                let grant_on =
                    grant_stmt && segment_before(i).any(|t| !t.quoted && is_grant_priv(&t.text));
                let create_on = stmt_verb[i].as_deref() == Some("create")
                    && segment_before(i).any(|t| {
                        !t.quoted
                            && matches!(
                                t.text.to_ascii_lowercase().as_str(),
                                // `rule`은 제외 — CREATE RULE의 ON은 이벤트
                                // 자리(`ON INSERT TO t`)라 관계가 아니다.
                                "index" | "trigger" | "policy"
                            )
                    });
                grant_on || create_on
            }
            // grant·revoke의 FROM은 권한 주체 자리다 — 관계가 아니므로
            // from·join을 그 문장에서는 열지 않는다.
            "from" | "join" => !grant_stmt,
            _ => true,
        };
        if !fires {
            continue;
        }
        let mut j = i + 1;
        // ONLY·IF NOT EXISTS 같은 수식어는 건너뛴다. `table`은 TRUNCATE 뒤의
        // 수식어일 때만 건너뛴다 — UPDATE table 같은 문에서 table이 진짜
        // 관계 이름일 수 있고, 억지로 건너뛰면 SET 같은 다음 단어가
        // 관계명으로 읽힌다.
        let head_is_truncate = word == "truncate";
        while j < tokens.len()
            && !tokens[j].quoted
            && is_name_modifier(&tokens[j].text, head_is_truncate)
        {
            consumed[j] = true;
            j += 1;
        }
        if word == "on" && grant_stmt {
            // GRANT/REVOKE ON은 객체 종류어가 낄 수 있다 — `ON TABLE t`의
            // table은 수식어고, `ON SEQUENCE`/`ON FUNCTION`/`ON ALL TABLES`
            // 같은 비테이블 객체는 관계가 아니라 조용히 삼킨다.
            let kind = tokens
                .get(j)
                .filter(|t| !t.quoted)
                .map(|t| t.text.to_ascii_lowercase());
            match kind.as_deref() {
                Some("table" | "tables" | "view" | "materialized") => {
                    // 종류어 뒤의 이름이 관계다 — `ON FOREIGN TABLE`의
                    // foreign는 비테이블 목록으로 보낸다(서버·래퍼가 더 흔함).
                    let mut k = j;
                    while k < tokens.len()
                        && matches!(
                            tokens[k].text.to_ascii_lowercase().as_str(),
                            "table" | "tables" | "view" | "materialized"
                        )
                    {
                        consumed[k] = true;
                        k += 1;
                    }
                    j = k;
                }
                Some(
                    "all" | "sequence" | "schema" | "database" | "domain" | "type" | "function"
                    | "procedure" | "routine" | "foreign" | "server" | "wrapper" | "language"
                    | "large" | "publication" | "subscription" | "statistics" | "tablespace"
                    | "collation" | "conversion" | "extension" | "aggregate" | "operator"
                    | "policy" | "cast" | "fdw" | "parser" | "template" | "dictionary"
                    | "configuration",
                ) => {
                    // 비테이블 권한 객체 — 이름·한정자·인자 괄호까지 삼키고
                    // 사실은 내지 않는다(미해석도 아닌 정상 문법이다).
                    let mut k = j;
                    while k < tokens.len() {
                        let t = &tokens[k];
                        if !t.quoted && t.text == "(" {
                            match skip_parens(&tokens, k) {
                                Some(next) => k = next,
                                None => {
                                    unresolved = true; // 닫히지 않은 괄호.
                                    break;
                                }
                            }
                        } else if is_name_token(t) || (!t.quoted && t.text == ".") {
                            consumed[k] = true;
                            k += 1;
                        } else {
                            // 플레이스홀더 피연산자(`ON SEQUENCE {s}`)는
                            // 읽히지 않은 근거다 — 미해석으로 센다.
                            if !t.quoted
                                && matches!(t.text.as_str(), "{" | "}" | "$" | "?" | ":" | "@")
                            {
                                unresolved = true;
                            }
                            break;
                        }
                    }
                    continue;
                }
                _ => {}
            }
        }
        if j >= tokens.len() {
            unresolved = true; // 이름이 없는 키워드 — "SELECT ... FROM" 꼴.
            continue;
        }
        // GRANT/REVOKE의 ON은 형태 검증을 거친다 — name (, name)* 뒤에
        // TO·FROM·WITH·`;`·끝이 와야 한다. "grant select on the report"
        // 같은 산문은 이름이 쉼표 없이 이어져 형태가 성립하지 않으므로
        // 이름을 버퍼에 모았다가 형태가 맞을 때만 방출한다.
        let buffered_grant = word == "on" && grant_stmt;
        let mut buf: Vec<String> = Vec::new();
        let mut end_pos = j;
        // 쉼표로 이어지는 목록(`FROM a, b`)을 읽는다 — 괄호 피연산자는
        // 통째로 건너뛰고(안쪽 관계는 그 안의 키워드가 읽는다) 별칭은 삼킨다.
        while j < tokens.len() {
            // 괄호 안의 토큰은 소비 표시를 하지 않는다 — 서브쿼리 안의
            // FROM 같은 키워드가 바깥 스캔에서 읽혀야 한다.
            let operand_end = if tokens[j].text == "(" && !tokens[j].quoted {
                match skip_parens(&tokens, j) {
                    Some(next) => next,
                    None => {
                        unresolved = true; // 닫히지 않은 괄호.
                        break;
                    }
                }
            } else {
                match read_qualified_name(&tokens, j) {
                    Some((name, next)) => {
                        if buffered_grant {
                            buf.push(name);
                        } else if seen.insert(name.clone()) {
                            out.push(name);
                        }
                        for c in consumed.iter_mut().take(next).skip(j) {
                            *c = true;
                        }
                        next
                    }
                    None => {
                        // 이름 자리에 절 키워드가 오는 것(`DO UPDATE SET`,
                        // `ON TABLES TO`)은 정상 종료다 — 플레이스홀더 등
                        // 읽히지 않는 피연산자만 미해석으로 센다.
                        let clause_next = tokens
                            .get(j)
                            .is_some_and(|t| is_name_token(t) && is_clause_word(&t.text));
                        if !clause_next {
                            unresolved = true;
                        }
                        break;
                    }
                }
            };
            // `AS alias` 또는 쉼표 직전 별칭(`FROM users u, ..`)을 건너뛴다.
            let mut k = operand_end;
            if tokens
                .get(k)
                .is_some_and(|t| !t.quoted && t.text.eq_ignore_ascii_case("as"))
                && tokens.get(k + 1).is_some_and(is_name_token)
            {
                k += 2;
            } else if tokens.get(k).is_some_and(is_name_token)
                && tokens
                    .get(k + 1)
                    .is_some_and(|t| !t.quoted && t.text == ",")
            {
                k += 1;
            }
            for c in consumed.iter_mut().take(k).skip(operand_end) {
                *c = true;
            }
            end_pos = k;
            if tokens.get(k).is_some_and(|t| !t.quoted && t.text == ",") {
                j = k + 1;
                continue;
            }
            break;
        }
        if buffered_grant {
            // 피연산자 뒤가 GRANT 종결자가 아니면 산문이다 — 버퍼를 버린다.
            // `WITH GRANT OPTION`은 피연산자가 아니라 피부여자 뒤에 오고,
            // `)`는 GRANT가 중첩되지 않아 종결자가 아니다 — 둘 다 산문만 허용한다.
            let term_ok = match tokens.get(end_pos) {
                None => true,
                Some(t) => {
                    !t.quoted && matches!(t.text.to_ascii_lowercase().as_str(), "to" | "from" | ";")
                }
            };
            if term_ok {
                for name in buf {
                    if seen.insert(name.clone()) {
                        out.push(name);
                    }
                }
            }
        }
    }
    (out, unresolved)
}

/// GRANT/REVOKE의 권한 단어인지 본다 — `ON`이 관계 자리임을 확정하는 근거다.
/// "grant access on staging to the intern" 같은 산문은 권한 단어가 없어 막힌다.
fn is_grant_priv(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "select"
            | "insert"
            | "update"
            | "delete"
            | "truncate"
            | "references"
            | "trigger"
            | "execute"
            | "usage"
            | "create"
            | "connect"
            | "temporary"
            | "temp"
            | "maintain"
            | "all"
    )
}

/// `table` 토큰이 관계 키워드로 발화하는 문맥인지 본다 — 직전 비인용
/// 식별자가 DDL 동사(ALTER·DROP·CREATE·TRUNCATE·RENAME·LOCK 등)일 때만이다.
fn table_keyword_context(tokens: &[SqlToken], i: usize) -> bool {
    (0..i)
        .rev()
        .find(|&k| is_name_token(&tokens[k]))
        .is_some_and(|k| {
            matches!(
                tokens[k].text.to_ascii_lowercase().as_str(),
                "alter"
                    | "drop"
                    | "create"
                    | "truncate"
                    | "rename"
                    | "lock"
                    | "unlock"
                    | "describe"
                    | "desc"
                    | "analyze"
                    | "vacuum"
            )
        })
}

/// 관계 키워드와 이름 사이에 올 수 있는 수식어다 — `table`은 TRUNCATE
/// 뒤에서만 수식어다.
fn is_name_modifier(word: &str, after_truncate: bool) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "only" | "if" | "not" | "exists"
    ) || (after_truncate && word.eq_ignore_ascii_case("table"))
}

/// `(` 토큰부터 짝이 맞는 `)` 다음 위치를 돌려준다 — 닫히지 않으면 None.
fn skip_parens(tokens: &[SqlToken], start: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (k, t) in tokens.iter().enumerate().skip(start) {
        if t.quoted {
            continue;
        }
        if t.text == "(" {
            depth += 1;
        } else if t.text == ")" {
            depth -= 1;
            if depth == 0 {
                return Some(k + 1);
            }
        }
    }
    None
}

/// 관계 이름 위치에 올 수 없는 SQL 절 키워드다 — `FROM {} WHERE` 템플릿의
/// 빈 플레이스홀더 뒤 토큰이 관계명으로 오독되지 않게 한다.
/// (`table`은 이름으로 읽어야 해서 제외한다 — `UPDATE table SET` 참조.)
fn is_clause_word(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "where"
            | "set"
            | "on"
            | "group"
            | "order"
            | "by"
            | "having"
            | "limit"
            | "offset"
            | "union"
            | "intersect"
            | "except"
            | "values"
            | "returning"
            | "as"
            | "left"
            | "right"
            | "inner"
            | "outer"
            | "full"
            | "cross"
            | "natural"
            | "lateral"
            | "using"
            | "and"
            | "or"
            | "not"
            | "null"
            | "select"
            | "insert"
            | "delete"
            | "from"
            | "join"
            | "into"
            | "update"
            | "truncate"
            | "with"
            | "for"
            | "in"
            | "is"
            | "case"
            | "when"
            | "then"
            | "else"
            | "end"
            | "distinct"
            | "asc"
            | "desc"
            | "if"
            | "exists"
            | "only"
            | "between"
            | "like"
            | "to"
            | "grant"
            | "revoke"
            | "option"
            | "cascade"
            | "restrict"
            | "privileges"
    )
}

/// SQL 텍스트를 어휘로 나눈다 — 인용 식별자는 내용을 보존하고
/// 그 외엔 식별자 문자열과 단일 기호 토큰만 만든다.
fn lex_sql(text: &str) -> Vec<SqlToken> {
    let b = text.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        match c {
            b'"' | b'`' | b'[' => {
                let end = if c == b'[' { b']' } else { c };
                let mut j = i + 1;
                while j < b.len() && b[j] != end {
                    j += 1;
                }
                tokens.push(SqlToken {
                    text: text[i + 1..j].to_string(),
                    quoted: true,
                });
                i = j + 1;
            }
            _ if is_ident_start(c) => {
                let mut j = i + 1;
                while j < b.len() && is_ident_part(b[j]) {
                    j += 1;
                }
                tokens.push(SqlToken {
                    text: text[i..j].to_string(),
                    quoted: false,
                });
                i = j;
            }
            b'-' if i + 1 < b.len() && b[i + 1] == b'-' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'/' if i + 1 < b.len() && b[i + 1] == b'*' => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                    i += 1;
                }
                i += 2;
            }
            b'\'' => {
                // 문자열 리터럴은 이름이 아니다 — '' 와 \' 는 escape다.
                i += 1;
                while i < b.len() {
                    if b[i] == b'\\' {
                        i += 2;
                        continue;
                    }
                    if b[i] == b'\'' {
                        if i + 1 < b.len() && b[i + 1] == b'\'' {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
            }
            _ => {
                // `;`는 문장 경계다 — 다중 문장 리터럴의 머리 동사 추적에 쓴다.
                // 플레이스홀더 기호(`{}`·`$n`·`?`·`:name`·`@`)도 남긴다 — 관계
                // 자리에 오면 읽히지 않는 피연산자로 미해석을 세야 하기 때문이다.
                if matches!(
                    c,
                    b'.' | b',' | b'(' | b')' | b';' | b'{' | b'}' | b'$' | b'?' | b':' | b'@'
                ) {
                    tokens.push(SqlToken {
                        text: (c as char).to_string(),
                        quoted: false,
                    });
                }
                i += 1;
            }
        }
    }
    tokens
}

/// `ident(.ident)*` 한정 이름을 읽어 (이름, 다음 위치)를 돌려준다.
fn read_qualified_name(tokens: &[SqlToken], start: usize) -> Option<(String, usize)> {
    let first = tokens.get(start)?;
    if first.quoted {
        if first.text.is_empty() {
            return None;
        }
    } else if !is_name_token(first) || is_clause_word(&first.text) {
        // 절 키워드(WHERE·SET·AS …)는 이름이 아니다 — `FROM {} WHERE`의
        // where 같은 토큰이 관계명으로 읽히지 않게 한다.
        return None;
    }
    let mut name = escape_segment(first);
    let mut i = start + 1;
    while i + 1 < tokens.len() && tokens[i].text == "." && !tokens[i].quoted {
        let next = &tokens[i + 1];
        if !next.quoted && (!is_name_token(next) || is_clause_word(&next.text)) {
            break;
        }
        name.push('.');
        name.push_str(&escape_segment(next));
        i += 2;
    }
    Some((name, i))
}

/// 인용 세그먼트의 `%`와 `.`을 escape한다 — `"a.b"` 같은 한 식별자가
/// 한정자로 오독되지 않게 하고, escape 문자 자체의 충돌을 막는다.
/// 비인용 세그먼트는 점을 담을 수 없어 `%`만 escape한다.
fn escape_segment(tok: &SqlToken) -> String {
    if tok.quoted {
        tok.text.replace('%', "%25").replace('.', "%2E")
    } else {
        tok.text.replace('%', "%25")
    }
}

/// 한정 이름의 각 세그먼트를 escape해 합친다 — 어트리뷰트·매크로에서
/// 온 이름도 `.`가 한정자인 계약과 같게 맞춘다.
fn escape_qualified(name: &str) -> String {
    name.split('.')
        .map(|seg| seg.replace('%', "%25").replace('.', "%2E"))
        .collect::<Vec<_>>()
        .join(".")
}

/// 이름 문자열 그대로를 한 세그먼트로 escape한다 — `table_name = "a.b"`
/// 같은 문자열 값은 한정자가 아니라 한 식별자다.
fn escape_name(name: &str) -> String {
    name.replace('%', "%25").replace('.', "%2E")
}

/// snake_case 변환이다 — sea-orm이 어트리뷰트 없는 필드를 컬럼으로
/// 매핑하는 규약과 맞춘다(연속 대문자는 한 단어로 묶는다).
fn to_snake_case(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::new();
    for (i, c) in chars.iter().enumerate() {
        if c.is_uppercase() && i > 0 {
            let after_lower = chars[i - 1].is_lowercase() || chars[i - 1].is_ascii_digit();
            let word_start =
                chars[i - 1].is_uppercase() && chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            if after_lower || word_start {
                out.push('_');
            }
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

/// 비인용 토큰이 식별자인지 본다 — 기호·빈 문자열은 아니다.
fn is_name_token(tok: &SqlToken) -> bool {
    !tok.quoted && !tok.text.is_empty() && is_ident_start(tok.text.as_bytes()[0])
}

/// SQL 식별자 시작 문자인지 본다.
fn is_ident_start(c: u8) -> bool {
    c == b'_' || c == b'$' || c.is_ascii_alphabetic() || c >= 0x80
}

/// SQL 식별자의 이어지는 문자인지 본다.
fn is_ident_part(c: u8) -> bool {
    is_ident_start(c) || c.is_ascii_digit()
}

/// bridge-facts 계약이 요구하는 RFC 3339 UTC 타임스탬프를 만든다.
/// 달력 변환은 외부 의존 없이 표준 civil 알고리즘으로 처리한다.
fn rfc3339_utc_now() -> String {
    rfc3339_utc(std::time::SystemTime::now())
}

/// SystemTime을 RFC 3339 UTC 문자열로 변환한다.
fn rfc3339_utc(t: std::time::SystemTime) -> String {
    let secs = t
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Unix 일수를 그레고리력 (year, month, day)로 변환한다 — Howard Hinnant의
/// civil_from_days 알고리즘이다.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    if month <= 2 {
        year += 1;
    }
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// SQL 렉서·관계 추출의 표 테스트 — gartograph의 schema_test.go와
    /// 같은 경계를 검증한다.
    #[test]
    fn sql_relations_reads_keywords_and_lists() {
        let cases: &[(&str, &[&str])] = &[
            ("SELECT * FROM users", &["users"]),
            (
                "select id from public.users u join orders o",
                &["public.users", "orders"],
            ),
            ("INSERT INTO app.events (id) VALUES (1)", &["app.events"]),
            ("UPDATE sessions SET seen = 1", &["sessions"]),
            ("DELETE FROM audit_log", &["audit_log"]),
            ("FROM a, b, c.x", &["a", "b", "c.x"]),
            // 별칭이 붙은 쉼표 목록도 모두 읽는다.
            ("FROM users u, orders o", &["users", "orders"]),
            ("FROM a AS x, b", &["a", "b"]),
            // 서브쿼리 안쪽은 그 안의 키워드가 읽고 목록은 이어진다 —
            // 안쪽 이름이 스캔 순서상 뒤에 나온다(최종 출력은 위치·이름으로 정렬).
            ("FROM (SELECT * FROM a) t, b", &["b", "a"]),
            (
                "FROM a JOIN (SELECT 1) x ON x.i = a.i JOIN b ON true",
                &["a", "b"],
            ),
            // 인용 식별자는 한 세그먼트 — 점을 담으면 escape된다.
            (r#"FROM "a.b"."c""#, &["a%2Eb.c"]),
            (r#"FROM `schema`.`table`"#, &["schema.table"]),
            // 주석·문자열 리터럴은 이름이 아니다.
            ("SELECT 'x' -- from fake\nFROM real_t", &["real_t"]),
            ("SELECT 'it''s' FROM logs", &["logs"]),
            ("/* join fake */ INSERT INTO t2 SELECT 1", &["t2"]),
            // TRUNCATE TABLE의 TABLE은 수식어다.
            ("TRUNCATE TABLE events", &["events"]),
            ("TRUNCATE events", &["events"]),
            // UPDATE 문의 table은 진짜 관계 이름일 수 있다.
            ("UPDATE table SET x = 1", &["table"]),
            ("SELECT set FROM table", &["table"]),
            // 산문 속 키워드 모양은 관계가 아니다.
            ("please update the config", &[]),
            ("merged the branch into main", &[]),
            ("upsert ... on conflict do update set x = 1", &[]),
            // 수식어 건너뛰기.
            ("SELECT * FROM ONLY users", &["users"]),
            ("DROP TABLE IF EXISTS legacy", &["legacy"]),
            ("CREATE TABLE IF NOT EXISTS fresh (id int)", &["fresh"]),
            // `;`로 갈린 뒤 문장의 머리 동사도 같은 규칙으로 연다.
            ("SELECT 1; UPDATE users SET x = 1", &["users"]),
            ("SELECT 1; TRUNCATE sessions", &["sessions"]),
            // GRANT/REVOKE의 ON은 관계 자리 — 그 문장의 FROM은 주체 자리다.
            ("GRANT SELECT ON t TO r", &["t"]),
            ("REVOKE SELECT ON t FROM r", &["t"]),
            // ON 뒤의 객체 종류어는 수식어다 — 이름이 관계다.
            ("GRANT SELECT ON TABLE metrics TO r", &["metrics"]),
            ("GRANT SELECT ON a, b TO r", &["a", "b"]),
            // 테이블이 아닌 권한 객체는 관계가 아니다 — 조용히 삼킨다.
            ("GRANT USAGE ON SEQUENCE seq TO r", &[]),
            ("GRANT EXECUTE ON FUNCTION f(int) TO r", &[]),
            ("GRANT ALL ON SCHEMA s TO r", &[]),
            ("GRANT SELECT ON ALL TABLES IN SCHEMA s TO r", &[]),
            ("REVOKE ALL ON DATABASE d FROM r", &[]),
            // 권한 단어 없는 산문의 grant/on은 관계가 아니다.
            ("grant access on staging-db to the intern", &[]),
            ("revoke permission on friday", &[]),
            // 권한 단어가 있어도 피연산자 뒤에 TO/FROM/WITH/;/끝이 없으면
            // 산문이다 — "on the report"는 이름이 쉼표 없이 이어진다.
            ("grant select on the report to auditors", &[]),
            ("grant select on staging to the team", &["staging"]),
            (
                "revoke insert on public.sessions from r",
                &["public.sessions"],
            ),
            // CREATE INDEX·TRIGGER·POLICY의 ON 대상도 관계다 — RULE의
            // ON은 이벤트 자리라 관계가 아니다.
            ("CREATE UNIQUE INDEX i ON users (email)", &["users"]),
            ("CREATE TRIGGER tr ON audit AFTER UPDATE", &["audit"]),
            ("CREATE POLICY p ON orders", &["orders"]),
            ("CREATE RULE r AS ON INSERT TO emp DO INSTEAD NOTHING", &[]),
            // 다른 문장의 ON은 조인 조건이다 — 관계가 아니다.
            ("SELECT * FROM a ON CONFLICT DO NOTHING", &["a"]),
            // `;` 너머의 단어를 앞 문장의 근거로 쓰지 않는다.
            ("select columns; update the readme; set up CI next", &[]),
            ("SELECT 1; sign into the portal", &[]),
            // FROM 없는 SQL은 관계 없음.
            ("SELECT 1", &[]),
            ("VALUES (1, 2)", &[]),
        ];
        for (sql, want) in cases {
            assert_eq!(&sql_relations(sql).0, want, "sql: {sql}");
        }
    }

    /// 관계 자리가 비리터럴(플레이스홀더)이면 미해석으로 센다 —
    /// 사실 없이 조용히 넘기면 isthmus가 "참조 없음"으로 읽는다.
    #[test]
    fn sql_relations_flags_unresolved_operands() {
        assert_eq!(
            sql_relations("DELETE FROM {} WHERE id = $1"),
            (vec![], true)
        );
        assert_eq!(sql_relations("INSERT INTO {} VALUES (1)"), (vec![], true));
        // 괄호 피연산자(서브쿼리)는 미해석이 아니다 — 안쪽은 따로 읽힌다.
        assert_eq!(
            sql_relations("FROM (SELECT * FROM a) t"),
            (vec!["a".to_string()], false)
        );
    }

    #[test]
    fn looks_like_sql_gates_on_verbs() {
        assert!(looks_like_sql("select 1"));
        assert!(looks_like_sql("WITH x AS (SELECT 1) DELETE FROM t"));
        assert!(looks_like_sql("UPDATE t SET x = 1"));
        assert!(looks_like_sql("TRUNCATE t"));
        assert!(!looks_like_sql("from the beginning"));
        // 산문 속 "update"는 문장 머리가 아니면 SQL로 보지 않는다.
        assert!(!looks_like_sql("please update the config"));
        assert!(!looks_like_sql(""));
    }

    #[test]
    fn snake_case_matches_sea_orm_convention() {
        assert_eq!(to_snake_case("userId"), "user_id");
        assert_eq!(to_snake_case("id"), "id");
        assert_eq!(to_snake_case("HTTPReq"), "http_req");
    }

    #[test]
    fn table_macro_parses_qualified_and_columns() {
        let ts: TokenStream = "public.users (id) { id -> Int4, name -> Nullable<Text>, }"
            .parse()
            .unwrap();
        let (rel, cols) = parse_table_macro(&ts).unwrap();
        assert_eq!(rel, "public.users");
        assert_eq!(
            cols.iter().map(|(c, _)| c.as_str()).collect::<Vec<_>>(),
            ["id", "name"]
        );
    }

    #[test]
    fn table_macro_skips_use_items_and_column_attrs() {
        let ts: TokenStream =
            "use diesel::sql_types::*; events (id) { #[sql_name = \"x\"] id -> Int4 }"
                .parse()
                .unwrap();
        let (rel, cols) = parse_table_macro(&ts).unwrap();
        assert_eq!(rel, "events");
        assert_eq!(cols.len(), 1);
    }

    #[test]
    fn table_macro_rejects_other_grammars() {
        assert!(parse_table_macro(&"x".parse().unwrap()).is_none());
        assert!(parse_table_macro(&"".parse().unwrap()).is_none());
    }

    #[test]
    fn split_args_respects_groups() {
        let ts: TokenStream = "User, \"SELECT 1\", (a, b)".parse().unwrap();
        assert_eq!(split_top_level_commas(&ts).len(), 3);
    }

    #[test]
    fn civil_calendar_roundtrip() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_000), (2024, 10, 4));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
    }

    #[test]
    fn meta_name_value_reads_diesel_and_sea_orm() {
        let attr: syn::Attribute = syn::parse_quote!(#[diesel(table_name = users)]);
        assert_eq!(
            meta_name_value(&attr, &["diesel"], "table_name")
                .unwrap()
                .text,
            "users"
        );
        let attr: syn::Attribute = syn::parse_quote!(#[sea_orm(table_name = "audit_log")]);
        let v = meta_name_value(&attr, &["sea_orm"], "table_name").unwrap();
        assert_eq!(v.text, "audit_log");
        assert!(v.literal);
        assert_eq!(v.krate, "sea_orm");
        // 다른 크레이트 어트리뷰트나 없는 키는 None이다.
        let attr: syn::Attribute = syn::parse_quote!(#[serde(rename = "x")]);
        assert!(meta_name_value(&attr, &["diesel"], "rename").is_none());
        let attr: syn::Attribute = syn::parse_quote!(#[diesel(check_for_backend(Pg))]);
        assert!(meta_name_value(&attr, &["diesel"], "table_name").is_none());
    }
}
