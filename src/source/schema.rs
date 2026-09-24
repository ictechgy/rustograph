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
//!   - diesel DSL 경로 — `x::table`, `x::dsl::y`, `x::columns::y` 꼴
//!
//! 한정되지 않은 이름(`query!`, `sql_query`, `update` 등)은 그 파일이
//! sqlx·diesel에서 해당 이름을 import할 때만 인정한다 — 이름만 같은
//! 다른 크레이트 API를 관계 참조로 오독하지 않기 위해서다.
//! 리터럴로 읽히지 않는 SQL 인자는 버리지 않고 dynamic 사실로 보존한다 —
//! 조인하지 못하는 이유를 소비자가 셀 수 있어야 한다.

use crate::cargo_meta;
use proc_macro2::{Span, TokenStream, TokenTree};
use serde::Serialize;
use std::collections::BTreeSet;
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
            Ok(ast) => scan_file(&mut scan, &file, &src, &ast),
            Err(_) => scan.unparsed += 1,
        }
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
    dynamic: usize,                        // 리터럴로 읽히지 않아 조인 불가한 SQL 인자 수
    latest: Option<std::time::SystemTime>, // 읽은 소스의 최신 mtime
    seen: BTreeSet<String>,
}

/// 한 파일의 사실을 모은다 — import 집합을 먼저 채우고(1패스) 사실을
/// 읽는다(2패스). 본문 안의 `use`도 뒤의 사용처를 위해 미리 모은다.
fn scan_file(scan: &mut SchemaScan, file: &Path, src: &str, ast: &syn::File) {
    let mut ctx = FileCtx {
        scan,
        src,
        file,
        imported: BTreeSet::new(),
        renamed: std::collections::BTreeMap::new(),
        glob: BTreeSet::new(),
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
    imported: BTreeSet<String>, // `use sqlx::query` 같은 이름 바인딩
    /// `use sqlx::query as q` — 바인딩 이름 → 원래 이름. 매크로 규칙은
    /// 원래 이름으로 찾아야 `query as q`도 query의 인자 규칙을 따른다.
    renamed: std::collections::BTreeMap<String, String>,
    glob: BTreeSet<String>, // `use sqlx::*` 같은 크레이트 글롭
    pass_two: bool,
}

impl<'ast> Visit<'ast> for FileCtx<'ast> {
    fn visit_item_use(&mut self, node: &'ast syn::ItemUse) {
        let mut prefix = Vec::new();
        collect_use(
            &node.tree,
            &mut prefix,
            &mut self.imported,
            &mut self.renamed,
            &mut self.glob,
        );
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

    fn visit_item_struct(&mut self, node: &'ast syn::ItemStruct) {
        if self.pass_two {
            self.scan_model_struct(node);
        }
        syn::visit::visit_item_struct(self, node);
    }

    fn visit_lit_str(&mut self, node: &'ast LitStr) {
        if self.pass_two {
            self.scan
                .push_sql_literal(self.scan_locate(node.span()), &node.value());
        }
        syn::visit::visit_lit_str(self, node);
    }
}

/// `use` 트리를 걸어 sqlx·diesel이보낸 이름 바인딩을 모은다.
/// 첫 세그먼트가 sqlx/diesel일 때만 마지막 세그먼트를 기록한다 —
/// 이름 충돌 판별에 크레이트 출처가 필요해서다.
fn collect_use(
    tree: &syn::UseTree,
    prefix: &mut Vec<String>,
    imported: &mut BTreeSet<String>,
    renamed: &mut std::collections::BTreeMap<String, String>,
    glob: &mut BTreeSet<String>,
) {
    let from_db = || matches!(prefix.first().map(String::as_str), Some("sqlx" | "diesel"));
    match tree {
        syn::UseTree::Path(p) => {
            prefix.push(p.ident.to_string());
            collect_use(&p.tree, prefix, imported, renamed, glob);
            prefix.pop();
        }
        syn::UseTree::Name(n) => {
            if from_db() {
                imported.insert(n.ident.to_string());
            }
        }
        syn::UseTree::Rename(r) => {
            if from_db() {
                imported.insert(r.rename.to_string());
                renamed.insert(r.rename.to_string(), r.ident.to_string());
            }
        }
        syn::UseTree::Glob(_) => {
            if let Some(krate) = prefix.first() {
                if matches!(krate.as_str(), "sqlx" | "diesel") {
                    glob.insert(krate.clone());
                }
            }
        }
        syn::UseTree::Group(g) => {
            for t in &g.items {
                collect_use(t, prefix, imported, renamed, glob);
            }
        }
    }
}

impl<'ast> FileCtx<'ast> {
    /// 비한정 이름이 sqlx·diesel에서 온 것인지 본다.
    fn imported_from_db(&self, name: &str) -> bool {
        self.imported.contains(name) || !self.glob.is_empty()
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
        // 비한정 `table!`도 받는다 — 그 이름의 매크로는 diesel이 사실상
        // 유일하고, 문법이 맞지 않으면 unparsed-table-macros로 센다.
        let owned = match segs.as_slice() {
            [name] => name == "table",
            [krate, name] => krate == "diesel" && name == "table",
            _ => false,
        };
        if !owned {
            return false;
        }
        match parse_table_macro(&m.tokens) {
            Some((relation, columns)) => {
                let loc = self.scan_locate(m.path.segments.last().unwrap().ident.span());
                // 위치가 확보된 사실만 낸다 — 위치 없는 사실은 push가 걸러준다.
                let loc = match loc {
                    Some(l) => l,
                    None => return true,
                };
                self.scan.push(RelationFact {
                    kind: "relation-use",
                    channel: escape_qualified(&relation),
                    method: None,
                    dynamic: false,
                    location: loc.clone(),
                });
                for (col, span) in columns {
                    if let Some(cloc) = self.scan_locate(span) {
                        self.scan.push(RelationFact {
                            kind: "relation-use",
                            channel: escape_qualified(&relation),
                            method: Some(col),
                            dynamic: false,
                            location: cloc,
                        });
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
        let Some(last) = segs.last() else {
            return false;
        };
        // rename 바인딩은 원래 매크로 이름으로 규칙을 찾는다 —
        // `use sqlx::query as q`의 q!는 query!의 인자 규칙을 따른다.
        let name = self
            .renamed
            .get(last.as_str())
            .map(String::as_str)
            .unwrap_or(last.as_str());
        let owned = match segs.as_slice() {
            [single] => self.imported_from_db(single),
            [krate, _leaf] => krate == "sqlx",
            _ => false,
        };
        if !owned {
            return false;
        }
        let file_arg = sqlx_file_macro_arg(name);
        let sql_arg = sqlx_macro_arg(name);
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
            (true, None) => {
                if let Some(l) = loc {
                    self.scan.push_dynamic_text(self.src, l, self.file)
                }
            }
            (false, Some(Expr::Lit(el))) => {
                if let syn::Lit::Str(lit) = &el.lit {
                    self.scan
                        .push_sql_literal(self.scan_locate(lit.span()).or(loc), &lit.value());
                } else {
                    self.scan
                        .push_dynamic(self.src, &Expr::Lit(el.clone()), loc, self.file);
                }
            }
            (false, Some(e)) => self.scan.push_dynamic(self.src, &e, loc, self.file),
            (false, None) => {
                if let Some(l) = loc {
                    self.scan.push_dynamic_text(self.src, l, self.file)
                }
            }
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
        // 비한정 이름은 rename 바인딩의 원래 이름으로 규칙을 찾는다.
        let resolve =
            |n: &String| -> String { self.renamed.get(n).cloned().unwrap_or_else(|| n.clone()) };
        let owned = match segs.as_slice() {
            [name] => {
                self.imported_from_db(name)
                    && (SQLX_SQL_FNS.contains(&resolve(name).as_str())
                        || DIESEL_SQL_FNS.contains(&resolve(name).as_str()))
            }
            [krate, name] => {
                (krate == "sqlx" && SQLX_SQL_FNS.contains(&name.as_str()))
                    || (krate == "diesel" && DIESEL_SQL_FNS.contains(&name.as_str()))
            }
            _ => false,
        };
        if !owned {
            return;
        }
        if let Some(arg) = call.args.first() {
            let is_lit = matches!(arg, Expr::Lit(el) if matches!(el.lit, syn::Lit::Str(_)));
            if !is_lit {
                let loc = self.scan_locate(fp.path.span());
                self.scan.push_dynamic(self.src, arg, loc, self.file);
            }
        }
    }

    /// diesel DSL 모양의 경로를 읽는다 — `x::table`은 관계 x,
    /// `x::dsl::y`·`x::columns::y`는 관계 x의 컬럼 y다. 경로는 식
    /// 위치에서만 본다 — 타입 위치의 `users::table`은 diesel의 생성 타입
    /// 경로라 같은 규칙으로 읽힌다.
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
        match segs.as_slice() {
            [.., table, last] if last == "table" => {
                if let Some(l) = loc_of(segs.len() - 1) {
                    self.scan.push(RelationFact {
                        kind: "relation-use",
                        channel: escape_qualified(table),
                        method: None,
                        dynamic: false,
                        location: l,
                    });
                }
            }
            [.., table, middle, last] if middle == "dsl" || middle == "columns" => {
                if let Some(l) = loc_of(segs.len() - 2) {
                    self.scan.push(RelationFact {
                        kind: "relation-use",
                        channel: escape_qualified(table),
                        method: None,
                        dynamic: false,
                        location: l.clone(),
                    });
                    self.scan.push(RelationFact {
                        kind: "relation-use",
                        channel: escape_qualified(table),
                        method: Some(last.clone()),
                        dynamic: false,
                        location: l,
                    });
                }
            }
            _ => {}
        }
    }

    /// `#[diesel(table_name = x)]`·`#[sea_orm(table_name = "x")]` 구조체를
    /// 읽어 관계 참조와 필드별 컬럼 참조를 낸다.
    /// 관계 바인딩 없는 `column_name`/`sqlx::rename`은 귀속 불가 수로 센다.
    fn scan_model_struct(&mut self, st: &syn::ItemStruct) {
        let mut table: Option<(String, Span)> = None;
        for attr in &st.attrs {
            if let Some(v) = meta_name_value(attr, &["diesel", "sea_orm"], "table_name") {
                table = Some(v);
            }
        }
        let Some((name, span)) = table else {
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
            .scan_locate(span)
            .or_else(|| self.scan_locate(st.ident.span()))
        else {
            return;
        };
        let channel = escape_qualified(&name);
        self.scan.push(RelationFact {
            kind: "relation-use",
            channel: channel.clone(),
            method: None,
            dynamic: false,
            location: rloc,
        });
        for f in &st.fields {
            let mut col: Option<String> = None;
            let mut col_span = f.ident.as_ref().map(|i| i.span());
            for attr in &f.attrs {
                if let Some((v, sp)) = meta_name_value(attr, &["diesel", "sea_orm"], "column_name")
                    .or_else(|| meta_name_value(attr, &["sqlx"], "rename"))
                {
                    col = Some(v);
                    col_span = Some(sp);
                }
            }
            let column = match col.or_else(|| f.ident.as_ref().map(|i| i.to_string())) {
                Some(c) if !c.is_empty() => c.trim_start_matches("r#").to_string(),
                _ => continue,
            };
            if let Some(cloc) = col_span.and_then(|s| self.scan_locate(s)) {
                self.scan.push(RelationFact {
                    kind: "relation-use",
                    channel: channel.clone(),
                    method: Some(column),
                    dynamic: false,
                    location: cloc,
                });
            }
        }
    }

    /// 토큰 span을 프로젝트 상대의 계약 위치로 바꾼다.
    /// 프로젝트 밖 파일은 상대 경로가 없어 사실로 만들지 않는다.
    fn scan_locate(&self, span: Span) -> Option<BridgeLocation> {
        self.scan.locate(self.src, span, self.file)
    }
}

/// `#[krate(key = value)]` 형태의 중첩 메타에서 값을 읽는다.
/// 반환은 (값 문자열, 값 span) — 식별자·경로·문자열 모두 받는다.
fn meta_name_value(attr: &syn::Attribute, crates: &[&str], key: &str) -> Option<(String, Span)> {
    let krate = attr.path().segments.first()?.ident.to_string();
    if !crates.contains(&krate.as_str()) {
        return None;
    }
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
                syn::Lit::Str(s) => Some((s.value(), s.span())),
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
                Some((name, p.path.segments.last()?.ident.span()))
            }
            _ => None,
        };
    }
    None
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
                    if let Some(loc) = scan.locate(src, lit.span(), file) {
                        scan.push_sql_literal(Some(loc), &lit.value());
                    }
                }
            }
            _ => {}
        }
    }
}

impl SchemaScan {
    /// SQL 형태의 리터럴에서 관계 이름을 읽어 사실로 낸다.
    fn push_sql_literal(&mut self, loc: Option<BridgeLocation>, text: &str) {
        let Some(loc) = loc else {
            return;
        };
        if !looks_like_sql(text) {
            return;
        }
        for name in sql_relations(text) {
            self.push(RelationFact {
                kind: "relation-use",
                channel: name,
                method: None,
                dynamic: false,
                location: loc.clone(),
            });
        }
    }

    /// 리터럴로 읽히지 않는 SQL 인자를 동적 사실로 보존한다.
    /// channel에는 잘린 원문 표현식을 실어 어느 위치의 호출인지 남긴다.
    fn push_dynamic(&mut self, src: &str, expr: &Expr, loc: Option<BridgeLocation>, file: &Path) {
        let Some(loc) = loc.or_else(|| self.locate(src, expr.span(), file)) else {
            return;
        };
        let text = expr_text(src, expr);
        self.push_dynamic_str(&text, loc);
    }

    /// 표현식을 파싱하지 못했을 때의 dynamic 사실 — 매크로 위치를 남긴다.
    fn push_dynamic_text(&mut self, _src: &str, loc: BridgeLocation, _file: &Path) {
        self.push_dynamic_str("<unparsed macro argument>", loc);
    }

    /// dynamic 사실 공통 경로 — 원문은 120자로 자른다.
    fn push_dynamic_str(&mut self, text: &str, loc: BridgeLocation) {
        let text: String = text.chars().take(120).collect();
        self.push(RelationFact {
            kind: "relation-use",
            channel: if text.len() < 120 {
                text
            } else {
                format!("{}...", text.trim_end())
            },
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
        let column = line_text
            .get(..start.column)
            .map(|s| s.chars().map(|c| c.len_utf16() as u32).sum::<u32>())
            .unwrap_or(0)
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
        if self.dynamic > 0 {
            // isthmus가 미사용 진단을 unverified로 내리는 근거다 — 접두사를
            // 바꾸면 조인기의 severity 계산이 새로 인식하지 못한다.
            out.push(format!(
                "unjoined-dynamic-relations: {} SQL argument(s) were not literals; their relations are uncounted",
                self.dynamic
            ));
        }
        out.sort();
        out.dedup();
        out
    }
}

/// 사실의 결정적 순서다 — 위치·종류·이름 순.
fn fact_cmp(a: &RelationFact, b: &RelationFact) -> std::cmp::Ordering {
    (
        &a.location.path,
        a.location.line,
        a.location.column,
        a.kind,
        &a.channel,
        &a.method,
    )
        .cmp(&(
            &b.location.path,
            b.location.line,
            b.location.column,
            b.kind,
            &b.channel,
            &b.method,
        ))
}

/// 표현식의 원문을 span 범위로 잘라낸다 — 토큰 재조합보다 소스 그대로가
/// 진단 단서로 정확하다.
fn expr_text(src: &str, expr: &Expr) -> String {
    let span = expr.span();
    let (s, e) = (span.start(), span.end());
    if s.line == 0 || e.line == 0 {
        return "<dynamic expression>".to_string();
    }
    let mut starts = Vec::new();
    let mut off = 0usize;
    for line in src.lines() {
        starts.push(off);
        off += line.len() + 1;
    }
    let get = |line: usize, col: usize| -> usize {
        starts.get(line - 1).copied().unwrap_or(src.len()) + col
    };
    let (a, b) = (get(s.line, s.column), get(e.line, e.column));
    src.get(a..b).unwrap_or("<dynamic expression>").to_string()
}

/// 문자열이 SQL로 보이는지 본다 — 동사가 없는 리터럴은 스캔하지 않아
/// 산문 속 "from" 같은 오탐을 막는다.
fn looks_like_sql(text: &str) -> bool {
    lex_sql(text)
        .iter()
        .any(|t| !t.quoted && is_sql_verb(&t.text))
}

/// SQL 동사 표다 — 이 단어들이 있어야 문자열을 SQL로 읽는다.
/// gartograph와 같은 표다 — `WITH` 절은 SELECT를 동반하므로 빠진다.
fn is_sql_verb(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "select"
            | "insert"
            | "update"
            | "delete"
            | "create"
            | "alter"
            | "drop"
            | "truncate"
            | "replace"
            | "merge"
    )
}

/// SQL 어휘 하나다 — 인용된 식별자는 키워드가 아니다.
struct SqlToken {
    text: String,
    quoted: bool,
}

/// 뒤따르는 식별자가 관계 이름인 키워드다.
fn is_relation_keyword(word: &str) -> bool {
    matches!(
        word.to_ascii_lowercase().as_str(),
        "from" | "join" | "into" | "update" | "table" | "truncate"
    )
}

/// SQL 텍스트에서 관계 이름을 읽는다.
/// 한정 이름(`schema.table`)은 그대로 두고, 이름 자체에 점이 있는 인용
/// 식별자("a.b")는 한 세그먼트로 읽는다 — escape는 사실 기록 시에 한다.
fn sql_relations(text: &str) -> Vec<String> {
    let tokens = lex_sql(text);
    let mut out = Vec::new();
    let mut seen = BTreeSet::new(); // TRUNCATE TABLE처럼 겹치는 키워드 창의 중복을 막는다
    let mut consumed = vec![false; tokens.len()]; // 이름·수식어로 소비된 토큰
    for i in 0..tokens.len() {
        let tok = &tokens[i];
        if consumed[i] || tok.quoted || !is_relation_keyword(&tok.text) {
            continue;
        }
        let mut j = i + 1;
        // ONLY·IF NOT EXISTS 같은 수식어는 건너뛴다. `table`은 TRUNCATE 뒤의
        // 수식어일 때만 건너뛴다 — UPDATE table 같은 문에서 table이 진짜
        // 관계 이름일 수 있고, 억지로 건너뛰면 SET 같은 다음 단어가
        // 관계명으로 읽힌다.
        let head_is_truncate = tok.text.eq_ignore_ascii_case("truncate");
        while j < tokens.len()
            && !tokens[j].quoted
            && (tokens[j].text.eq_ignore_ascii_case("only")
                || tokens[j].text.eq_ignore_ascii_case("if")
                || tokens[j].text.eq_ignore_ascii_case("not")
                || tokens[j].text.eq_ignore_ascii_case("exists")
                || (head_is_truncate && tokens[j].text.eq_ignore_ascii_case("table")))
        {
            consumed[j] = true;
            j += 1;
        }
        // 쉼표로 이어지는 목록(`FROM a, b`)을 읽는다.
        while j < tokens.len() {
            let Some((name, next)) = read_qualified_name(&tokens, j) else {
                break;
            };
            if seen.insert(name.clone()) {
                out.push(name);
            }
            for c in consumed.iter_mut().take(next).skip(j) {
                *c = true;
            }
            if next < tokens.len() && tokens[next].text == "," {
                j = next + 1;
                continue;
            }
            break;
        }
    }
    out
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
                if matches!(c, b'.' | b',' | b'(' | b')') {
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
    } else if !is_name_token(first) {
        return None;
    }
    let mut name = escape_segment(first);
    let mut i = start + 1;
    while i + 1 < tokens.len() && tokens[i].text == "." && !tokens[i].quoted {
        let next = &tokens[i + 1];
        if !next.quoted && !is_name_token(next) {
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
            // 수식어 건너뛰기.
            ("SELECT * FROM ONLY users", &["users"]),
            ("DROP TABLE IF EXISTS legacy", &["legacy"]),
            ("CREATE TABLE IF NOT EXISTS fresh (id int)", &["fresh"]),
            // FROM 없는 SQL은 관계 없음.
            ("SELECT 1", &[]),
            ("VALUES (1, 2)", &[]),
        ];
        for (sql, want) in cases {
            assert_eq!(&sql_relations(sql), want, "sql: {sql}");
        }
    }

    #[test]
    fn looks_like_sql_gates_on_verbs() {
        assert!(looks_like_sql("select 1"));
        assert!(looks_like_sql("WITH x AS (SELECT 1) DELETE FROM t"));
        assert!(!looks_like_sql("from the beginning"));
        assert!(!looks_like_sql(""));
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
            meta_name_value(&attr, &["diesel"], "table_name").unwrap().0,
            "users"
        );
        let attr: syn::Attribute = syn::parse_quote!(#[sea_orm(table_name = "audit_log")]);
        assert_eq!(
            meta_name_value(&attr, &["sea_orm"], "table_name")
                .unwrap()
                .0,
            "audit_log"
        );
        // 다른 크레이트 어트리뷰트나 없는 키는 None이다.
        let attr: syn::Attribute = syn::parse_quote!(#[serde(rename = "x")]);
        assert!(meta_name_value(&attr, &["diesel"], "rename").is_none());
        let attr: syn::Attribute = syn::parse_quote!(#[diesel(check_for_backend(Pg))]);
        assert!(meta_name_value(&attr, &["diesel"], "table_name").is_none());
    }
}
