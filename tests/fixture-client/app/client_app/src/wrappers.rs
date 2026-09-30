//! 선언된 래퍼(`tests/fixture-client/http-wrappers.json`)와 선언되지 않은 싱크.

use crate::api::{ApiClient, Endpoint, Verb};
use crate::{Res, HOST};
use reqwest::blocking::Client;

/// 함수 래퍼 base.
const WRAP_BASE: &str = "http://api.example.com/wb";

/// 선언된 함수 래퍼 — 기본 동사 GET, 경로는 첫 인자.
pub fn fetch_json(path: &str) -> Res {
    Client::new().get(format!("{WRAP_BASE}{path}")).send()?;
    Ok(())
}

/// 선언되지 않은 싱크 — 매개변수를 URL로 흘려보낸다(`http-wrapper-undeclared:`).
pub fn raw_get(path: &str) -> Res {
    Client::new().get(format!("{HOST}{path}")).send()?;
    Ok(())
}

pub fn wrapper_function() -> Res {
    fetch_json("/w/items")
}

pub fn wrapper_method() -> Res {
    let api = ApiClient::new();
    api.send(Verb::Post, "/w/orders")
}

pub fn wrapper_constructor(id: u32) -> Res {
    let api = ApiClient::new();
    api.execute(Endpoint {
        verb: Verb::Delete,
        path: format!("/e/items/{id}"),
    })
}

pub fn undeclared_sink() -> Res {
    raw_get("/v1/undeclared")
}
