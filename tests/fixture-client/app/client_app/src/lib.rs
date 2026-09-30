//! 합성 HTTP 클라이언트 fixture — rustograph `routes --role client`가 읽고,
//! experiments/client-oracle이 같은 소스를 진짜 reqwest·ureq·url로 컴파일해 로컬
//! 서버로 요청을 보낸다. 시나리오 함수 하나가 요청 하나를 보낸다.

pub mod api;
pub mod scenarios;
pub mod ureq_calls;
pub mod wrappers;

/// 시나리오 결과 — 오라클은 오류도 기록한다(상대 URL은 보내기 전에 실패한다).
pub type Res = Result<(), Box<dyn std::error::Error>>;

/// 모든 리터럴 URL의 host.
pub const HOST: &str = "http://api.example.com";
