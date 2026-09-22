//! rustograph — Rust 코드베이스의 질의 가능 의존성 그래프.
//! 그래프가 산출물이고 나머지는 전부 그 위의 질의다.
//!
//! 수확(cargo_meta/modtree/harvest/source) → 문서(graph) → 질의
//! (analysis/rules) → 출력(export/sarif/cli) 순서로만 의존한다.

pub mod analysis;
pub mod cargo_meta;
pub mod cli;
pub mod config;
pub mod export;
pub mod graph;
pub mod harvest;
pub mod mcp;
pub mod modtree;
pub mod rules;
pub mod sarif;
pub mod source;
