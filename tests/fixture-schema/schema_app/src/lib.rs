//! schema 명령 fixture — sqlx·diesel·sea-orm·동적 인자의 사실 수확을 검증한다.
//! 실제 sqlx/diesel 의존은 없다 — 스캐너는 이름만 읽고 확장하지 않는다.

use sqlx::query_scalar;
use sqlx::query as sqlx_query;

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
}

/// 변수에 담긴 리터럴도 관계 참조다.
pub const LIST_SQL: &str = "SELECT * FROM public.members";

/// SQL이 아닌 문자열은 걸리지 않는다.
pub const PROSE: &str = "from the beginning of the report";

/// sqlx를 import하지 않은 비한정 매크로는 다른 크레이트의 것일 수 있다.
pub mod unimported {
    pub fn f() {
        // `other_query!`는 인정되지 않는다 — 이름 표에 없다.
        let _ = other_query!("SELECT 1");
    }
}
