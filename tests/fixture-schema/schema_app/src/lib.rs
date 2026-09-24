//! schema 명령 fixture — sqlx·diesel·sea-orm·동적 인자의 사실 수확을 검증한다.
//! 실제 sqlx/diesel 의존은 없다 — 스캐너는 이름만 읽고 확장하지 않는다.

use sqlx::query_scalar;
use sqlx::query as sqlx_query;
use diesel::table;
use diesel::table as dt;
use sqlx as db;

diesel::table! {
    use diesel::sql_types::*;
    public.users (id) {
        id -> Int4,
        name -> Text,
        nick_name -> Nullable<Text>,
    }
}

diesel::table! {
    audit_log (id) {
        id -> Int4,
        message -> Text,
    }
}

/// sqlx 매크로 — 한정·비한정·컬럼 있는 리터럴.
pub fn find_user() {
    let _ = sqlx::query!("SELECT id, name FROM users WHERE active = $1");
    let _ = sqlx::query_as::<(), ()>("SELECT * FROM app.orders");
    let _ = sqlx::query_scalar!("SELECT count(*) FROM reporting.events");
}

/// 비한정 매크로는 파일이 sqlx에서 import할 때만 인정된다.
pub fn imported_macro() {
    let _ = query_scalar!("DELETE FROM sessions");
    let _ = sqlx_query!("UPDATE users SET seen = 1");
}

/// 비리터럴 인자는 dynamic 사실이다.
pub fn dynamic_sql(text: &str) {
    let _ = sqlx::query!(text);
    let _ = sqlx::query_as::<(), ()>(text.to_string());
    let _ = sqlx::query_file!("queries/top.sql");
}

/// 별칭으로 import된 table! 매크로 — 선언 이름은 DSL 귀속 목록에 든다.
dt! {
    metrics (id) {
        id -> Int4,
    }
}

/// 형태가 다른 table! — 문법이 맞지 않으면 limitation으로 센다.
table! {
    broken_macro_body
}

/// diesel 모델 어트리뷰트 — 관계 + 필드 컬럼.
#[diesel(table_name = users)]
pub struct User {
    pub id: i32,
    #[diesel(column_name = nick_name)]
    pub nick: Option<String>,
}

/// sea-orm 엔티티 어트리뷰트.
#[sea_orm(table_name = "audit_log")]
pub struct AuditLog {
    pub id: i32,
    pub message: String,
}

/// sea-orm만 선언하는 이름 — diesel DSL 귀속 목록에는 들지 않는다.
#[sea_orm(table_name = "sea_only")]
pub struct SeaOnly {
    pub id: i32,
}

/// 바인딩 없는 컬럼 어트리뷰트 — unattributed로 센다.
pub struct Orphan {
    #[diesel(column_name = loner)]
    pub x: i32,
}

/// diesel DSL 경로 — x::table과 x::dsl::y.
pub fn dsl_refs() {
    let _ = insert_into(users::table);
    let _ = users::dsl::id;
    let _ = audit_log::columns::message;
    // 타입 위치의 DSL 경로도 같은 규칙으로 읽힌다.
    let _: audit_log::table;
    // 선언된 table! 이름과 맞지 않는 같은 모양의 경로는 동적 근거다.
    let _ = config::table;
    // sea_orm이 선언한 이름은 diesel DSL 귀속 목록에 들지 않는다.
    let _ = sea_only::table;
    // 별칭 table!이 선언한 이름은 DSL 경로를 연다.
    let _ = metrics::table;
}

/// 크레이트 별칭 경로의 호출도 sqlx 규칙을 따른다.
pub fn crate_alias() {
    let _ = db::query("SELECT * FROM aliased_q");
    let _ = db::raw_sql("SET statement_timeout = 0");
}

/// 다중 문장과 GRANT — 문장 경계 뒤의 동사도 읽고 ON은 권한 문 안에서만.
pub const MULTI: &str = "SELECT 1; UPDATE sessions SET seen = 1";
pub const GRANT: &str = "GRANT SELECT ON grant_t TO app_role";

/// format! 템플릿의 관계 자리 플레이스홀더는 동적 사실로 남는다.
pub fn templated(name: &str) {
    let _ = format!("DELETE FROM {} WHERE id = 1", name);
}

/// 변수에 담긴 리터럴도 관계 참조다.
pub const LIST_SQL: &str = "SELECT * FROM public.members";

/// SQL이 아닌 문자열은 걸리지 않는다 — 산문 속 키워드 모양 포함.
pub const PROSE: &str = "from the beginning of the report";
pub const PROSE_UPDATE: &str = "please update the config file";
pub const PROSE_INTO: &str = "merged the branch into main";

/// sqlx를 import하지 않은 비한정 매크로는 다른 크레이트의 것일 수 있다.
/// 이름 규칙은 걸리지 않지만, 알 수 없는 매크로의 리터럴은 여전히
/// SQL 스캔 대상이다 — 템플릿 속 SQL을 놓치지 않기 위한 의도다.
pub mod unimported {
    pub fn f() {
        let _ = other_query!("SELECT 1");
        let _ = other_query!("SELECT * FROM flagged_t");
    }
}
