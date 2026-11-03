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

/// 서로를 필드로 담는 재귀 데이터 정의는 심볼 레벨 순환이 아니다 — 직접 재귀처럼 고칠 대상이 아닌
/// 한 타입 묶음이다. 필드 간선은 그대로 있어 type 레벨 순환과 dead에는 쓰인다. 같은 타입 쌍이
/// 메서드로 서로를 부르면 그것은 호출 재귀라 계속 순환이다.
#[test]
fn recursive_data_types_are_not_symbol_level_cycles() {
    let root = std::env::temp_dir().join(format!("rustograph-data-rec-{}", std::process::id()));
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname='datarec'\nversion='0.1.0'\nedition='2021'\n[workspace]\n",
    )
    .unwrap();
    std::fs::write(
        root.join("src/main.rs"),
        "enum Expr { Lit(u8), Block(Box<Stmt>) }\n\
         struct Stmt { e: Expr }\n\
         fn walk_expr(e: &Expr) { if let Expr::Block(s) = e { walk_stmt(s) } }\n\
         fn walk_stmt(s: &Stmt) { walk_expr(&s.e) }\n\
         fn main() { walk_expr(&Expr::Lit(1)); }\n",
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
    let field = |from: &str, to: &str| {
        doc.edges.iter().any(|e| {
            e.from == from && e.to == to && e.kind == rustograph::graph::EdgeKind::References
        })
    };
    assert!(field("datarec::Expr", "datarec::Stmt") && field("datarec::Stmt", "datarec::Expr"));
    let members: Vec<Vec<String>> = analysis::cycles(&doc)
        .cycles
        .into_iter()
        .map(|c| c.members)
        .collect();
    assert_eq!(members, [["datarec::walk_expr", "datarec::walk_stmt"]]);
    let types = doc.view(rustograph::graph::Level::Type).unwrap();
    assert!(
        analysis::cycles(&types)
            .cycles
            .iter()
            .any(|c| c.members == ["datarec::Expr", "datarec::Stmt"]),
        "type-level cycles keep field recursion"
    );
}
