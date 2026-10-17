//! tests/fixture-schema 워크스페이스에 대한 persistence 생산자 통합 테스트.
//! fixture는 sqlx 매크로·diesel table!·sea_orm 어트리뷰트·DSL 경로·
//! 동적 인자를 커버한다 — 사실·limitation·계약 필드를 실제 출력으로 검증한다.

use rustograph::cli;
use rustograph::source::schema;
use std::io::Cursor;
use std::path::PathBuf;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixture-schema")
}

fn doc() -> serde_json::Value {
    let d = schema::facts(&fixture(), "test").expect("schema facts failed");
    serde_json::to_value(&d).expect("document must serialize")
}

/// (channel, method, dynamic) 튜플로 사실을 조회한다.
fn has_fact(doc: &serde_json::Value, channel: &str, method: Option<&str>, dynamic: bool) -> bool {
    doc["facts"].as_array().unwrap().iter().any(|f| {
        f["channel"].as_str() == Some(channel)
            && f["method"].as_str() == method
            && f["dynamic"].as_bool() == Some(dynamic)
            && f["kind"] == "relation-use"
            && f["location"]["line"].as_u64().is_some_and(|l| l > 0)
            && f["location"]["path"].as_str() == Some("schema_app/src/lib.rs")
    })
}

#[test]
fn document_carries_persistence_contract() {
    let d = doc();
    assert_eq!(d["format"], "bridge-facts");
    assert_eq!(d["version"], 1);
    assert_eq!(d["tool"]["name"], "rustograph");
    assert_eq!(d["platform"], "rust");
    // 사실이 있으므로 target은 persistence다.
    assert_eq!(d["target"], "persistence");
    assert_eq!(
        d["project"],
        fixture().canonicalize().unwrap().display().to_string()
    );
    // 두 시각 모두 계약 문법(UTC, 밀리초 세 자리)으로 정규화한다.
    for key in ["generatedAt", "sourceModifiedAt"] {
        assert!(
            d[key].as_str().is_some_and(is_contract_timestamp),
            "{key}: {}",
            d[key]
        );
    }
}

/// `YYYY-MM-DDTHH:mm:ss.SSSZ`(GRAPH-EXCHANGE가 정한 UTC 밀리초 세 자리) 모양인가.
fn is_contract_timestamp(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() == 24
        && b.iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b'-',
            10 => *c == b'T',
            13 | 16 => *c == b':',
            19 => *c == b'.',
            23 => *c == b'Z',
            _ => c.is_ascii_digit(),
        })
}

#[test]
fn repeated_runs_are_byte_identical() {
    // 시각 필드를 빼고 두 실행의 직렬화가 같아야 한다 — diff·캐시의
    // 계약이 되는 결정성이다.
    let strip = |d: &mut serde_json::Value| {
        let o = d.as_object_mut().unwrap();
        o.remove("generatedAt");
        o.remove("sourceModifiedAt");
    };
    let mut a = doc();
    let mut b = doc();
    strip(&mut a);
    strip(&mut b);
    assert_eq!(
        serde_json::to_string(&a).unwrap(),
        serde_json::to_string(&b).unwrap()
    );
}

#[test]
fn sqlx_macros_and_calls_emit_relations() {
    let d = doc();
    // 한정·비한정 리터럴 모두 읽힌다.
    for ch in [
        "users",
        "app.orders",
        "reporting.events",
        "sessions",
        "public.members",
    ] {
        assert!(has_fact(&d, ch, None, false), "missing relation {ch}");
    }
}

#[test]
fn diesel_table_macro_emits_relation_and_columns() {
    let d = doc();
    assert!(has_fact(&d, "public.users", None, false));
    for col in ["id", "name", "nick_name"] {
        assert!(
            has_fact(&d, "public.users", Some(col), false),
            "missing column {col}"
        );
    }
    assert!(has_fact(&d, "audit_log", None, false));
    assert!(has_fact(&d, "audit_log", Some("message"), false));
}

#[test]
fn model_attributes_emit_relation_and_columns() {
    let d = doc();
    // #[diesel(table_name = users)] + column_name 재명명.
    assert!(has_fact(&d, "users", Some("id"), false));
    assert!(has_fact(&d, "users", Some("nick_name"), false));
    assert!(
        !has_fact(&d, "users", Some("nick"), false),
        "renamed column must not leak"
    );
    // sea-orm 문자열 형태.
    assert!(has_fact(&d, "audit_log", Some("id"), false));
}

#[test]
fn diesel_dsl_paths_emit_relation_and_columns() {
    let d = doc();
    // `x::table` → 관계, `x::dsl::y`·`x::columns::y` → 관계+컬럼.
    // DSL 경로는 위치가 겹치는 별개 사실로 둘 다 낸다.
    assert!(has_fact(&d, "users", Some("id"), false));
    assert!(has_fact(&d, "audit_log", Some("message"), false));
}

#[test]
fn non_literal_args_become_dynamic_facts() {
    let d = doc();
    assert!(has_fact(&d, "text", None, true));
    assert!(has_fact(&d, "text.to_string()", None, true));
    // query_file!의 SQL은 파일에 있어 dynamic이다.
    assert!(has_fact(&d, "\"queries/top.sql\"", None, true));
    // format! 템플릿의 플레이스홀더 관계 자리도 동적 근거다.
    assert!(has_fact(&d, "DELETE FROM {} WHERE id = 1", None, true));
    let lim = d["limitations"].as_array().unwrap();
    assert!(lim.iter().any(|l| l
        .as_str()
        .is_some_and(|s| s.starts_with("unjoined-dynamic-relations: 6"))));
}

#[test]
fn unknown_diesel_dsl_paths_are_dynamic_not_static() {
    let d = doc();
    // 선언된 table! 이름이 아닌 `x::table` 경로는 정적 사실이 되지 않는다 —
    // 동적 근거로만 남고 limitation으로 센다. sea_orm이 선언한 이름도
    // diesel DSL 귀속 목록에 들지 않으므로 같은 규칙이다.
    for ch in ["config::table", "sea_only::table"] {
        assert!(has_fact(&d, ch, None, true), "{ch} should stay dynamic");
    }
    assert!(
        !has_fact(&d, "config", None, false),
        "undeclared diesel path must not become a relation fact"
    );
    let lim = d["limitations"].as_array().unwrap();
    assert!(lim.iter().any(|l| l
        .as_str()
        .is_some_and(|s| s.starts_with("unresolved-diesel-paths: 2"))));
}

#[test]
fn crate_and_macro_aliases_are_resolved() {
    let d = doc();
    // use sqlx as db → db::query("..")의 리터럴은 확정 SQL 자리다.
    assert!(has_fact(&d, "aliased_q", None, false));
    // use diesel::table as dt → dt! 선언과 metrics::table 경로.
    assert!(has_fact(&d, "metrics", None, false));
    assert!(has_fact(&d, "metrics", Some("id"), false));
    // 타입 위치의 `audit_log::table`도 읽힌다 — audit_log의
    // 컬럼 없는 관계 사실은 expr 컬럼 경로의 것과 별개 위치로 둘이다.
    let rel_only = d["facts"].as_array().unwrap().iter().filter(|f| {
        f["channel"] == "audit_log" && f.get("method").is_none() && !f["dynamic"].as_bool().unwrap()
    });
    assert!(
        rel_only.count() >= 2,
        "type-position diesel path must be read"
    );
}

#[test]
fn multi_statement_and_grant_literals_are_read() {
    let d = doc();
    // `;` 뒤 문장 머리의 UPDATE도 관계 키워드로 연다.
    assert!(has_fact(&d, "sessions", None, false));
    // GRANT .. ON t — 권한 문 안의 on만 관계 자리다.
    assert!(has_fact(&d, "grant_t", None, false));
    // GRANT의 TO/FROM은 권한 주체 자리다 — 관계로 읽지 않는다.
    assert!(!has_fact(&d, "app_role", None, false));
    // 알 수 없는 매크로의 SQL 리터럴도 스캔 대상이다(템플릿 보존).
    assert!(has_fact(&d, "flagged_t", None, false));
    // sea_orm만 선언한 이름은 관계 사실은 내지만 DSL 경로는 열지 않는다.
    assert!(has_fact(&d, "sea_only", None, false));
}

#[test]
fn prose_strings_are_not_scanned() {
    // 산문 속 키워드 모양("update the ..", "into main")은 사실이 되지 않는다.
    let d = doc();
    for ch in ["beginning", "the", "main", "config", "file", "report"] {
        assert!(
            !has_fact(&d, ch, None, false) && !has_fact(&d, ch, None, true),
            "prose leaked as fact: {ch}"
        );
    }
}

#[test]
fn measured_gaps_report_as_limitations() {
    let d = doc();
    let lim: Vec<&str> = d["limitations"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|l| l.as_str())
        .collect();
    // 문법이 맞지 않은 table!과 바인딩 없는 컬럼 어트리뷰트는 센다.
    assert!(lim
        .iter()
        .any(|s| s.starts_with("unparsed-table-macros: 1")));
    assert!(lim
        .iter()
        .any(|s| s.starts_with("unattributed-column-tags: 1")));
}

#[test]
fn empty_workspace_emits_null_target() {
    // 사실이 없는 문서는 계약상 target null이다 — 빈 프로젝트로 검증한다.
    let tmp = std::env::temp_dir().join(format!("rustograph-schema-empty-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("empty_app/src")).unwrap();
    std::fs::write(
        tmp.join("Cargo.toml"),
        "[workspace]\nmembers = [\"empty_app\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    std::fs::write(
        tmp.join("empty_app/Cargo.toml"),
        "[package]\nname = \"empty_app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(tmp.join("empty_app/src/lib.rs"), "pub fn f() {}\n").unwrap();

    let d = schema::facts(&tmp, "test").unwrap();
    let v = serde_json::to_value(&d).unwrap();
    assert_eq!(v["target"], serde_json::Value::Null);
    assert_eq!(v["facts"].as_array().unwrap().len(), 0);
    let _ = std::fs::remove_dir_all(&tmp);
}

/// 임시 워크스페이스(`app` 패키지의 src/lib.rs 하나)를 만든다 — fixture의 계수를
/// 건드리지 않고 입력 모양 하나만 검증할 때 쓴다.
fn temp_workspace(tag: &str, lib: &str) -> PathBuf {
    let tmp = std::env::temp_dir().join(format!("rustograph-schema-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(tmp.join("app/src")).unwrap();
    std::fs::write(
        tmp.join("Cargo.toml"),
        "[workspace]\nmembers = [\"app\"]\nresolver = \"2\"\n",
    )
    .unwrap();
    std::fs::write(
        tmp.join("app/Cargo.toml"),
        "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(tmp.join("app/src/lib.rs"), lib).unwrap();
    tmp
}

/// isthmus `controlCharacterPattern`(C0·DEL·C1·U+2028·U+2029)과 같은 판정이다.
fn has_control(text: &str) -> bool {
    text.chars()
        .any(|c| matches!(c, '\u{0}'..='\u{1f}' | '\u{7f}'..='\u{9f}' | '\u{2028}' | '\u{2029}'))
}

/// 여러 줄 raw SQL·rustfmt가 나눈 `&format!(..)` 인자처럼 원문이 여러 줄인 dynamic
/// 사실도 채널에는 공백을 접은 한 줄만 싣는다 — 줄바꿈·탭 하나로 isthmus가 문서
/// 전체를 거부한다(GRAPH-EXCHANGE: 채널에 제어 문자·NEL·U+2028/2029 금지).
/// 120자 절단은 정화한 뒤에 건다.
#[test]
fn dynamic_channels_are_sanitized_single_lines() {
    let padded = format!(
        "SELECT 1{}FROM unnest($1::int[]) t",
        "\n            ".repeat(12)
    );
    let columns: Vec<String> = (0..40).map(|i| format!("c{i:02}")).collect();
    let lib = r##"
pub fn by_ids(ids: &[i32]) {
    let _ = sqlx::query!(
        r#"
        SELECT u.id
          FROM unnest($1::int[]) AS t(id)
          JOIN users u ON u.id = t.id
        "#,
        ids
    );
}

pub fn templated(table: &str) -> String {
    format!(
        "SELECT id, name
         FROM {}
         WHERE id = 1",
        table
    )
}

pub async fn find(table: &str) {
    let _ = sqlx::query(&format!(
        "SELECT id FROM {} WHERE id = $1",
        table
    ));
}

pub fn escapes() {
    let _ = sqlx::query("SELECT *\tFROM {} \u{85}\u{2028} WHERE\u{1}x");
}

pub fn padded() {
    let _ = sqlx::query!(r#"@PADDED@"#);
}

pub fn wide() {
    let _ = sqlx::query!(r#"SELECT @COLUMNS@ FROM {}"#);
}
"##
    .replace("@PADDED@", &padded)
    .replace("@COLUMNS@", &columns.join(",\n      "));
    let dir = temp_workspace("dynamic-channels", &lib);
    let d = serde_json::to_value(schema::facts(&dir, "test").expect("schema facts")).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let facts = d["facts"].as_array().unwrap();
    for f in facts {
        let channel = f["channel"].as_str().unwrap();
        assert!(!has_control(channel), "control character in {channel:?}");
        assert_eq!(channel, channel.trim(), "untrimmed channel {channel:?}");
        assert!(!channel.contains("  "), "unfolded channel {channel:?}");
    }
    let folded_wide = format!("SELECT {} FROM {{}}", columns.join(", "));
    let cut_wide = format!(
        "{}...",
        folded_wide.chars().take(120).collect::<String>().trim_end()
    );
    let dynamic: std::collections::BTreeSet<&str> = facts
        .iter()
        .filter(|f| f["dynamic"] == true)
        .map(|f| f["channel"].as_str().unwrap())
        .collect();
    let want: std::collections::BTreeSet<&str> = [
        "SELECT u.id FROM unnest($1::int[]) AS t(id) JOIN users u ON u.id = t.id",
        "SELECT id, name FROM {} WHERE id = 1",
        "&format!( \"SELECT id FROM {} WHERE id = $1\", table )",
        "SELECT id FROM {} WHERE id = $1",
        "SELECT * FROM {} WHEREx",
        // 원문은 120자를 넘지만 접으면 짧다 — 자르지 않는다.
        "SELECT 1 FROM unnest($1::int[]) t",
        cut_wide.as_str(),
    ]
    .into();
    assert_eq!(dynamic, want);
    // 정적 사실은 그대로다.
    assert!(facts
        .iter()
        .any(|f| f["channel"] == "users" && f["dynamic"] == false));
}

/// 위치 열은 그 줄의 UTF-8 바이트 오프셋 + 1이다(GRAPH-EXCHANGE) — proc-macro2 열은
/// 문자 수라 앞에 한글·이모지가 있는 줄에서는 바이트로 환산해야 한다. ASCII 줄은
/// 전과 같다. dynamic 원문도 같은 열로 잘라야 식 앞 글자로 밀리지 않는다.
#[test]
fn locations_use_utf8_byte_columns() {
    let lib = r#"
pub fn korean() {
    let 이름 = 1; let _s = "SELECT * FROM users";
    let _e = "😀"; let _s = "SELECT * FROM emoji_t";
    let _ = 이름;
}

pub fn ascii() {
    let name = 1; let _s = "SELECT * FROM orders";
    let _ = name;
}

pub fn korean_dyn(sql_text: &str) {
    let 이름 = 1; let _r = sqlx::query(&sql_text);
    let _ = 이름;
}
"#;
    let dir = temp_workspace("utf8-columns", lib);
    let d = serde_json::to_value(schema::facts(&dir, "test").expect("schema facts")).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let at = |channel: &str| -> (u64, u64) {
        let f = d["facts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["channel"] == channel)
            .unwrap_or_else(|| panic!("no fact {channel}: {}", d["facts"]));
        (
            f["location"]["line"].as_u64().unwrap(),
            f["location"]["column"].as_u64().unwrap(),
        )
    };
    // 기대 열 = 원문 줄에서 그 토큰의 바이트 위치 + 1.
    let byte_column = |needle: &str| -> (u64, u64) {
        lib.lines()
            .enumerate()
            .find_map(|(i, l)| l.find(needle).map(|b| (i as u64 + 1, b as u64 + 1)))
            .unwrap()
    };
    // `    let 이름 = 1; let _s = ` = 25자·29바이트(UTF-16으로는 26).
    assert_eq!(at("users"), (3, 30));
    assert_eq!(at("users"), byte_column("\"SELECT * FROM users\""));
    assert_eq!(at("emoji_t"), byte_column("\"SELECT * FROM emoji_t\""));
    assert_eq!(at("orders"), byte_column("\"SELECT * FROM orders\""));
    assert_eq!(at("&sql_text"), byte_column("sqlx::query(&sql_text)"));
}

#[test]
fn cli_schema_prints_document_and_writes_out() {
    let mut out = Cursor::new(Vec::new());
    let mut err = Cursor::new(Vec::new());
    let dir = fixture().display().to_string();
    let code = cli::run(
        &["schema".into(), "--dir".into(), dir.clone()],
        &mut out,
        &mut err,
    );
    assert_eq!(
        code,
        0,
        "stderr: {}",
        String::from_utf8_lossy(err.get_ref())
    );
    let text = String::from_utf8(out.into_inner()).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["platform"], "rust");
    assert_eq!(v["target"], "persistence");

    // --out은 파일에 쓰고 확인 문구를 뱉는다.
    let out_path =
        std::env::temp_dir().join(format!("rustograph-schema-{}.json", std::process::id()));
    let mut out = Cursor::new(Vec::new());
    let mut err = Cursor::new(Vec::new());
    let code = cli::run(
        &[
            "schema".into(),
            "--dir".into(),
            dir,
            "--out".into(),
            out_path.display().to_string(),
        ],
        &mut out,
        &mut err,
    );
    assert_eq!(code, 0);
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&out_path).unwrap()).unwrap();
    assert_eq!(written["format"], "bridge-facts");
    let _ = std::fs::remove_file(&out_path);
}

#[test]
fn cli_schema_rejects_unsupported_flags() {
    // schema는 --dir/--out만 쓴다 — 그래프 옵션을 조용히 삼키면
    // 사용자는 --semantic·--graph 등이 적용됐다고 오해한다.
    let dir = fixture().display().to_string();
    for extra in [
        &["--semantic"][..],
        &["--graph", "saved.json"][..],
        &["--level", "symbol"][..],
        &["--strict"][..],
        &["--format", "json"][..],
        &["stray"][..],
    ] {
        let mut argv: Vec<String> = vec!["schema".into(), "--dir".into(), dir.clone()];
        argv.extend(extra.iter().map(|s| s.to_string()));
        let mut out = Cursor::new(Vec::new());
        let mut err = Cursor::new(Vec::new());
        let code = cli::run(&argv, &mut out, &mut err);
        let err = String::from_utf8_lossy(err.get_ref()).into_owned();
        assert_eq!(code, 2, "{extra:?}: {err}");
        assert!(err.contains("not supported"), "{extra:?}: {err}");
        assert!(out.get_ref().is_empty(), "{extra:?}: no document on error");
    }
}

/// 같은 워크스페이스의 심볼 그래프 정점 ID — impact·reach가 쓰는 id 공간이다.
fn graph_ids() -> std::collections::BTreeSet<String> {
    let g = rustograph::source::load(
        &fixture(),
        &rustograph::source::Options {
            symbol_level: true,
            ..Default::default()
        },
    )
    .expect("graph harvest failed");
    g.vertices.into_iter().map(|v| v.id).collect()
}

#[test]
fn every_usr_is_a_graph_vertex() {
    // usr는 isthmus가 순회 문서의 id와 문자열 그대로 잇는다 — 그래프에
    // 없는 문자열이면 trace가 사실을 핸들러 도달과 영영 잇지 못한다.
    let d = doc();
    let ids = graph_ids();
    let mut with_usr = 0;
    for f in d["facts"].as_array().unwrap() {
        let Some(sym) = f.get("symbol") else { continue };
        let usr = sym["usr"].as_str().expect("symbol carries usr");
        assert!(ids.contains(usr), "usr {usr} is not a graph vertex");
        assert_eq!(sym["qualifiedName"].as_str(), Some(usr));
        with_usr += 1;
    }
    assert!(with_usr > 30, "most facts must carry a usr, got {with_usr}");
}

/// (경로 접미사, 채널, 컬럼)의 첫 사실이 싣는 usr.
fn usr_of(
    d: &serde_json::Value,
    path: &str,
    channel: &str,
    method: Option<&str>,
) -> Option<String> {
    let f = d["facts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| {
            f["location"]["path"]
                .as_str()
                .is_some_and(|p| p.ends_with(path))
                && f["channel"].as_str() == Some(channel)
                && f["method"].as_str() == method
        })
        .unwrap_or_else(|| panic!("fact {channel} {method:?} not found"));
    f.get("symbol")
        .map(|s| s["usr"].as_str().unwrap().to_string())
}

#[test]
fn usr_follows_the_enclosing_graph_item() {
    let d = doc();
    let lib = "src/lib.rs";
    let cases: &[(&str, Option<&str>, Option<&str>)] = &[
        // 함수 본문.
        ("app.orders", None, Some("schema_app::find_user")),
        // 인라인 모듈 안 함수.
        ("flagged_t", None, Some("schema_app::unimported::f")),
        // 고유 impl 메서드와 트레이트 impl 메서드(`Type::<Trait>::m`).
        ("repo_items", None, Some("schema_app::Repo::list")),
        (
            "loaded_items",
            None,
            Some("schema_app::Repo::<Loader>::load"),
        ),
        // 트레이트 기본 메서드.
        (
            "loader_defaults",
            None,
            Some("schema_app::Loader::fallback"),
        ),
        // 메서드 밖 연관 상수는 self 타입.
        ("repo_consts", None, Some("schema_app::Repo")),
        // 모듈 수준 const 초기화식.
        ("public.members", None, Some("schema_app::LIST_SQL")),
        // 구조체 어트리뷰트·필드 컬럼은 구조체 정점.
        ("users", Some("nick_name"), Some("schema_app::User")),
        ("sea_only", Some("id"), Some("schema_app::SeaOnly")),
        // cfg(test) 모듈도 그래프 정점이다(조건은 정점의 cfg로 남는다).
        (
            "test_only_t",
            None,
            Some("schema_app::tests::reads_fixture_table"),
        ),
        // 최상위 table! 매크로 호출은 감싸는 정점이 없다.
        ("public.users", Some("name"), None),
    ];
    for (channel, method, want) in cases {
        assert_eq!(
            usr_of(&d, lib, channel, *method).as_deref(),
            *want,
            "usr of {channel} {method:?}"
        );
    }
    // mod 선언이 없는 orphan 파일은 그래프 밖이다.
    assert_eq!(usr_of(&d, "src/orphan.rs", "orphan_t", None), None);
}

#[test]
fn facts_without_usr_are_counted_as_missing() {
    let d = doc();
    let missing = d["facts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f.get("symbol").is_none())
        .count();
    assert!(missing > 0);
    let want = format!("missing-relation-usrs: {missing} relation-use fact(s)");
    assert!(
        d["limitations"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l.as_str().is_some_and(|s| s.starts_with(&want))),
        "limitations must count {missing} facts without usr: {}",
        d["limitations"]
    );
}

/// `generatedAt`만 뺀 문서 — 같은 입력이면 바이트까지 같아야 하는 나머지(사실·
/// limitation·`sourceModifiedAt`)로 두 실행을 맞댄다. `sourceModifiedAt`은 읽은
/// 파일들의 최신 mtime이라 읽은 파일 집합이 달라지면 함께 달라진다. 쓰는 테스트가
/// 링크·FIFO를 만드는 unix 전용이라 도우미도 unix에서만 컴파일한다(Windows CI의
/// `cargo test export::tests`도 이 파일을 컴파일한다 — 안 쓰는 함수 경고를 피한다).
#[cfg(unix)]
fn facts_without_generated_at(dir: &std::path::Path) -> serde_json::Value {
    let mut d = serde_json::to_value(schema::facts(dir, "test").expect("schema facts")).unwrap();
    d.as_object_mut().unwrap().remove("generatedAt");
    d
}

/// 이름 붙은 파이프를 만든다 — 쓰는 쪽이 없으면 여는 순간 영원히 막힌다.
#[cfg(unix)]
fn mkfifo(path: &std::path::Path) {
    let status = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .expect("mkfifo must run");
    assert!(status.success(), "mkfifo {}", path.display());
}

/// 패키지 안의 링크 순환(`src/a -> .`·`src/b -> .`·`src/up -> ..`)은 링크 없는 트리와
/// 같은 문서를 낸다. 링크를 따라가는 순회는 단계마다 경로가 두 배가 되어 OS 링크
/// 한도(macOS 32단계)까지 사실상 끝나지 않았다.
#[cfg(unix)]
#[test]
fn symlink_cycles_do_not_change_facts() {
    let dir = temp_workspace(
        "link-cycle",
        "pub fn q() -> &'static str { \"SELECT * FROM users\" }\n",
    );
    let plain = facts_without_generated_at(&dir);
    for (link, target) in [("a", "."), ("b", "."), ("up", "..")] {
        std::os::unix::fs::symlink(target, dir.join("app/src").join(link)).unwrap();
    }
    let linked = facts_without_generated_at(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(linked, plain);
    assert!(plain["facts"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["channel"] == "users"));
}

/// 워크스페이스 루트 안을 가리키는 링크는 전처럼 따른다 — 패키지 밖 공유 디렉터리·
/// 파일의 SQL도 사실이 되고, 위치는 링크가 아니라 실제 경로다.
#[cfg(unix)]
#[test]
fn links_inside_the_workspace_are_still_followed() {
    let dir = temp_workspace("link-inside", "pub fn f() {}\n");
    std::fs::create_dir_all(dir.join("shared/sql")).unwrap();
    std::fs::write(
        dir.join("shared/sql/q.rs"),
        "pub fn q() -> &'static str { \"SELECT * FROM shared_dir_t\" }\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("shared/one.rs"),
        "pub fn o() -> &'static str { \"SELECT * FROM shared_file_t\" }\n",
    )
    .unwrap();
    std::os::unix::fs::symlink("../../shared/sql", dir.join("app/src/sql")).unwrap();
    std::os::unix::fs::symlink("../../shared/one.rs", dir.join("app/src/one.rs")).unwrap();
    let d = facts_without_generated_at(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    let path_of = |channel: &str| {
        d["facts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|f| f["channel"] == channel)
            .and_then(|f| f["location"]["path"].as_str().map(str::to_string))
    };
    assert_eq!(path_of("shared_dir_t").as_deref(), Some("shared/sql/q.rs"));
    assert_eq!(path_of("shared_file_t").as_deref(), Some("shared/one.rs"));
}

/// 루트 밖을 가리키는 디렉터리·.rs 링크는 따르지 않는다 — 그 아래 SQL은 위치가 루트
/// 밖이라 사실이 될 수 없는데, 읽으면 `unlocated-references`·`unparsed-sources`와
/// `sourceModifiedAt`(루트 밖 파일의 mtime)만 흐렸다.
#[cfg(unix)]
#[test]
fn links_leaving_the_workspace_are_not_followed() {
    let dir = temp_workspace(
        "link-outside",
        "pub fn q() -> &'static str { \"SELECT * FROM users\" }\n",
    );
    let outside =
        std::env::temp_dir().join(format!("rustograph-schema-outside-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&outside);
    std::fs::create_dir_all(outside.join("deep")).unwrap();
    let leak = outside.join("deep/leak.rs");
    let broken = outside.join("broken.rs");
    std::fs::write(
        &leak,
        "pub fn leak() -> &'static str { \"SELECT * FROM secret_outside_table\" }\n",
    )
    .unwrap();
    std::fs::write(&broken, "pub fn broken( {\n").unwrap();
    // 루트 밖 파일이 읽히면 sourceModifiedAt이 이 미래 시각으로 끌려간다.
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(86_400);
    for file in [&leak, &broken] {
        std::fs::File::options()
            .write(true)
            .open(file)
            .and_then(|f| f.set_modified(later))
            .unwrap();
    }
    let plain = facts_without_generated_at(&dir);
    std::os::unix::fs::symlink(&outside, dir.join("app/src/ext")).unwrap();
    std::os::unix::fs::symlink(&broken, dir.join("app/src/broken.rs")).unwrap();
    let linked = facts_without_generated_at(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&outside);
    assert_eq!(linked, plain);
}

/// 정규 파일이 아닌 `.rs` 항목(FIFO, FIFO를 가리키는 링크, `/dev/zero` 링크)은 읽지
/// 않는다 — FIFO는 여는 순간 쓰는 쪽을 영원히 기다리고 `/dev/zero`는 끝없이 읽힌다.
#[cfg(unix)]
#[test]
fn special_rs_entries_are_not_read() {
    let dir = temp_workspace(
        "special-entries",
        "pub fn q() -> &'static str { \"SELECT * FROM users\" }\n",
    );
    let plain = facts_without_generated_at(&dir);
    mkfifo(&dir.join("app/src/pipe.rs"));
    std::os::unix::fs::symlink("pipe.rs", dir.join("app/src/pipe_link.rs")).unwrap();
    std::os::unix::fs::symlink("/dev/zero", dir.join("app/src/zero.rs")).unwrap();
    let special = facts_without_generated_at(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(special, plain);
}
