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
        .is_some_and(|s| s.starts_with("unjoined-dynamic-relations: 5"))));
}

#[test]
fn unknown_diesel_dsl_paths_are_dynamic_not_static() {
    let d = doc();
    // 선언된 table! 이름이 아닌 `x::table` 경로는 정적 사실이 되지 않는다 —
    // 동적 근거로만 남고 limitation으로 센다.
    assert!(has_fact(&d, "config::table", None, true));
    assert!(
        !has_fact(&d, "config", None, false),
        "undeclared diesel path must not become a relation fact"
    );
    let lim = d["limitations"].as_array().unwrap();
    assert!(lim.iter().any(|l| l
        .as_str()
        .is_some_and(|s| s.starts_with("unresolved-diesel-paths: 1"))));
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
