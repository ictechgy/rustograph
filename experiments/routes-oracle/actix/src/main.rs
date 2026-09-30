//! actix-web 4.15.0 오라클 — fixture `actix_app::app()`을 `test::init_service`로 띄워 요청한다.
//! 모든 요청에 `x-api: 1`을 실어 헤더 가드 스코프도 닿게 한다(가드 없는 음성 요청은 따로 본다).

use actix_web::test;
use oracle_common::{evaluate, io_paths, load_facts, plan, write, Probe};

/// 기대 디스패치 — 등록 순서(정수 id가 슬러그보다 먼저), 스코프, 설정 함수, 꼬리 파라미터.
const EXPECTED: &[(&str, &str, &str)] = &[
    ("GET", "/", "actix_app::handlers::index"),
    ("GET", "/api/items", "actix_app::handlers::items::list"),
    ("GET", "/api/items/", "actix_app::handlers::items::list"),
    ("POST", "/api/items", "actix_app::handlers::items::create"),
    ("GET", "/api/items/42", "actix_app::handlers::items::show"),
    (
        "GET",
        "/api/items/abc",
        "actix_app::handlers::items::by_slug",
    ),
    ("GET", "/api/ping", "actix_app::handlers::ping"),
    ("GET", "/api/users/3", "actix_app::handlers::users::get"),
    (
        "DELETE",
        "/api/users/3",
        "actix_app::handlers::users::delete",
    ),
    ("GET", "/api/a", "actix_app::handlers::multi"),
    ("PATCH", "/api/b", "actix_app::handlers::multi"),
    ("POST", "/api/cfg", "actix_app::handlers::cfg_post"),
    ("GET", "/files/a/b.txt", "actix_app::handlers::files"),
    ("GET", "/r1", "actix_app::handlers::routes_multi"),
    ("POST", "/r2", "actix_app::handlers::routes_multi"),
    ("GET", "/m", "actix_app::handlers::route_macro"),
    ("PUT", "/m", "actix_app::handlers::route_macro"),
    ("GET", "/v1/status", "actix_app::handlers::v1_status"),
];

/// 닿지 않아야 하는 요청 — method 불일치, 스코프 밖, 트림된 빈 꼬리.
const NEGATIVE: &[(&str, &str)] = &[
    ("POST", "/api/users/3"),
    ("DELETE", "/r1"),
    ("GET", "/r2"),
    ("DELETE", "/m"),
    ("GET", "/files/"),
    ("GET", "/files"),
    ("GET", "/lonely"),
    ("GET", "/api/cfg"),
];

/// 요청 하나를 보낸다(가드 헤더 포함 여부 선택).
async fn send<S, B>(app: &S, p: &Probe, header: bool) -> (u16, String)
where
    S: actix_web::dev::Service<
        actix_http::Request,
        Response = actix_web::dev::ServiceResponse<B>,
        Error = actix_web::Error,
    >,
    B: actix_web::body::MessageBody,
{
    let method = actix_web::http::Method::from_bytes(p.method.as_bytes()).expect("method");
    let mut req = test::TestRequest::default().method(method).uri(&p.path);
    if header {
        req = req.insert_header(("x-api", "1"));
    }
    let resp = test::call_service(app, req.to_request()).await;
    let status = resp.status().as_u16();
    let body = test::read_body(resp).await;
    (status, String::from_utf8_lossy(&body).into_owned())
}

#[actix_web::main]
async fn main() -> std::process::ExitCode {
    let (input, output) = io_paths();
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(input).expect("routes document"))
            .expect("json");
    let (dispatch, facts) = load_facts(&doc);
    let app = test::init_service(actix_app::app()).await;
    let mut probes = plan(&facts, EXPECTED, NEGATIVE);
    let mut results = Vec::new();
    for p in probes.drain(..) {
        let (status, body) = send(&app, &p, true).await;
        results.push((p, status, body));
    }
    // 헤더 가드: 헤더 없는 요청은 v1 스코프를 건너뛰어 닿지 않아야 한다(narrowed 확인).
    let unguarded = Probe {
        kind: "negative",
        method: "GET".into(),
        path: "/v1/status".into(),
        truth: Some(None),
        fact: None,
    };
    let (status, body) = send(&app, &unguarded, false).await;
    let mut rec = evaluate(
        "actix",
        "actix-web 4.15.0 (actix-router 0.5.4)",
        &dispatch,
        &facts,
        results,
    );
    // narrowed 사실은 조건부라 사실 예측으로는 닿는다 — 실제로 닿지 않는지만 본다.
    rec.negatives.total += 1;
    let pass = status == 404;
    if pass {
        rec.negatives.passed += 1;
    }
    rec.probes.push(oracle_common::ProbeResult {
        probe: unguarded,
        predicted: oracle_common::Predicted::None,
        status,
        actual: if pass {
            oracle_common::Actual::NoMatch
        } else {
            oracle_common::Actual::Other(body)
        },
        pass,
    });
    write(&rec, &output)
}
