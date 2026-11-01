//! 직접 재귀의 자기 간선 — 수확은 소유자에서 자기 자신으로 가는 간선을 만들지 않는다(문서화된 동작).

use rustograph::{analysis, source};

/// `fn rec() { rec() }`는 자기 간선이 없어 `cycles --level symbol`에 순환으로 나오지 않는다. 서로
/// 부르는 두 함수는 지금처럼 순환이다. 도달성에는 영향이 없다 — 재귀 함수도 호출자에게서 닿는다.
#[test]
fn direct_recursion_is_not_a_cycle_but_mutual_recursion_is() {
    let root = std::env::temp_dir().join(format!("rustograph-recursion-{}", std::process::id()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='rec'\nversion='0.1.0'\nedition='2021'\n[workspace]\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/main.rs"),
        "fn rec(n: u8) { if n > 0 { rec(n - 1) } }\n\
         fn ping(n: u8) { if n > 0 { pong(n - 1) } }\n\
         fn pong(n: u8) { if n > 0 { ping(n - 1) } }\n\
         fn main() { rec(3); ping(3); }\n",
    )
    .unwrap();
    let doc = source::load(
        &root,
        &source::Options {
            symbol_level: true,
            ..Default::default()
        },
    );
    std::fs::remove_dir_all(&root).unwrap();
    let doc = doc.unwrap();
    assert!(!doc.edges.iter().any(|e| e.from == e.to), "{:?}", doc.edges);
    let cycles = analysis::cycles(&doc);
    let members: Vec<Vec<&str>> = cycles
        .cycles
        .iter()
        .map(|c| c.members.iter().map(String::as_str).collect())
        .collect();
    assert_eq!(members, [["rec::ping", "rec::pong"]]);
    assert!(analysis::dead(&doc).unwrap().findings.is_empty());
}
