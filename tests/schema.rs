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
    assert!(d["generatedAt"].as_str().is_some_and(|s| s.ends_with('Z')));
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
