//! semantic 엔진 통합 테스트 — `cargo test --features semantic`에서만 돈다.
//! 계약: 그래프 스키마·정점 ID는 syn 수확과 같고, 정확도만 올라간다.
//! 같은 이름 메서드는 타입으로 구별하고, 매크로는 확장 트리를 걷는다.

#![cfg(feature = "semantic")]

use rustograph::graph::{Document, Edge, EdgeKind};
use rustograph::source::{self, Options};
use std::path::PathBuf;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixture")
}

fn sem_doc() -> Document {
    source::load(
        &fixture(),
        &Options {
            symbol_level: true,
            semantic: true,
            ..Default::default()
        },
    )
    .expect("semantic fixture load failed")
}

fn syn_doc() -> Document {
    source::load(
        &fixture(),
        &Options {
            symbol_level: true,
            ..Default::default()
        },
    )
    .expect("syntactic fixture load failed")
}

fn call<'a>(d: &'a Document, from: &str, to: &str) -> Option<&'a Edge> {
    d.edges
        .iter()
        .find(|e| e.from == from && e.to == to && e.kind == EdgeKind::Call)
}

fn edge_key(e: &Edge) -> (&str, &str, EdgeKind, bool) {
    (e.from.as_str(), e.to.as_str(), e.kind, e.tentative)
}

#[test]
fn method_call_on_concrete_type_is_firm() {
    let d = sem_doc();
    // u: Used — greet은 Greet impl 메서드로 정확히 해석되어 추정이 아니다.
    let e = call(
        &d,
        "fixture_app::main",
        "fixture_core::Used::<Greet>::greet",
    )
    .expect("semantic call edge");
    assert!(!e.tentative, "concrete receiver must resolve firmly");
    // syn 팬아웃이 만들던 무관한 같은-이름 메서드는 없어야 한다.
    assert!(call(&d, "fixture_app::main", "fixture_core::Other::greet").is_none());
}

#[test]
fn inherent_and_self_calls_are_firm() {
    let d = sem_doc();
    let e = call(&d, "fixture_app::main", "fixture_core::Used::quadrupled")
        .expect("inherent method call edge");
    assert!(!e.tentative);
    let e = call(
        &d,
        "fixture_core::Used::quadrupled",
        "fixture_core::Used::doubled",
    )
    .expect("self method call edge");
    assert!(!e.tentative);
}

#[test]
fn trait_dispatch_expands_to_impl_matrix() {
    let d = sem_doc();
    // dyn 수신자 — 실제 impl을 모르니 후보는 추정 간선이다.
    let decl = call(
        &d,
        "fixture_core::dyn_dispatch",
        "fixture_core::Greet::greet",
    )
    .expect("trait decl candidate");
    assert!(decl.tentative);
    let imp = call(
        &d,
        "fixture_core::dyn_dispatch",
        "fixture_core::Used::<Greet>::greet",
    )
    .expect("impl candidate");
    assert!(imp.tentative);
    // Greet을 구현하지 않은 Other::greet은 후보가 아니다.
    assert!(call(
        &d,
        "fixture_core::dyn_dispatch",
        "fixture_core::Other::greet"
    )
    .is_none());
    // 제네릭 수신자도 같은 행렬이다.
    assert!(call(
        &d,
        "fixture_core::generic_dispatch",
        "fixture_core::Used::<Greet>::greet"
    )
    .is_some_and(|e| e.tentative));
}

#[test]
fn macro_expansion_yields_inner_call() {
    let d = sem_doc();
    // emit_helper!() 확장 안의 helper 호출 — syn은 빈 토큰만 보고 놓친다.
    let e =
        call(&d, "fixture_app::main", "fixture_core::util::helper").expect("expanded call edge");
    assert!(!e.tentative);
    // 매크로 정의 자체로의 간선도 있다.
    assert!(call(&d, "fixture_app::main", "fixture_core::emit_helper").is_some());
    // syn 모드에서는 이 간선이 없다 — 개선의 증거.
    let s = syn_doc();
    assert!(call(&s, "fixture_app::main", "fixture_core::util::helper").is_none());
}

#[test]
fn graph_schema_and_ids_unchanged() {
    let s = syn_doc();
    let d = sem_doc();
    // 정점 집합은 엔진과 무관하게 동일 — 정점은 syn이 만든다.
    let mut sv: Vec<_> = s.vertices.iter().map(|v| &v.id).collect();
    let mut dv: Vec<_> = d.vertices.iter().map(|v| &v.id).collect();
    sv.sort();
    dv.sort();
    assert_eq!(sv, dv);
    // 구조 간선(contains/uses/implements/signature)도 동일해야 한다.
    // Edge는 PartialEq가 없으니 키 튜플로 비교한다.
    for kind in [
        EdgeKind::Contains,
        EdgeKind::Uses,
        EdgeKind::Implements,
        EdgeKind::Signature,
    ] {
        let se: Vec<_> = s
            .edges
            .iter()
            .filter(|e| e.kind == kind)
            .map(edge_key)
            .collect();
        let de: Vec<_> = d
            .edges
            .iter()
            .filter(|e| e.kind == kind)
            .map(edge_key)
            .collect();
        assert_eq!(se, de, "structural {kind:?} edges differ");
    }
    // 한계는 여전히 실측이다 — semantic 통계 문장이 있어야 한다.
    assert!(d
        .limitations
        .iter()
        .any(|l| l.contains("semantic analysis:")));
}

#[test]
fn default_trait_method_on_concrete_type_is_firm() {
    let d = sem_doc();
    // u: Used는 Named::name을 오버라이드하지 않는다 — 선언점 디폴트가
    // 확정 타깃이지 후보 행렬이 아니다.
    let e = call(&d, "fixture_app::main", "fixture_core::Named::name")
        .expect("default trait method edge");
    assert!(!e.tentative, "concrete receiver on default method is firm");
}

#[test]
fn block_local_defs_do_not_leak_calls() {
    let d = sem_doc();
    // local_scope 안의 블록 지역 inner 본문 호출은 소유자에게 귀속되지 않는다.
    assert!(call(&d, "fixture_app::local_scope", "fixture_core::util::helper").is_none());
    // 블록 지역 fn은 정점이 없다 — 같은 이름의 정점으로의 간선도 없어야 한다.
    assert!(call(&d, "fixture_app::local_scope", "fixture_app::inner").is_none());
}

#[test]
fn local_fn_item_call_resolves() {
    let d = sem_doc();
    // let f = fixture_core::ffi_entry; f() — 지역 바인딩이지만 callable 해석이
    // fn 아이템까지 따라간다. syn은 이름 `f`를 못 잡아 간선이 없다.
    let e = call(&d, "fixture_app::main", "fixture_core::ffi_entry").expect("fn-item call edge");
    assert!(!e.tentative);
    let s = syn_doc();
    assert!(call(&s, "fixture_app::main", "fixture_core::ffi_entry").is_none());
}

#[test]
fn pattern_paths_reference_targets() {
    let d = sem_doc();
    // match 갈래의 `fixture_core::BASE` — 패턴 위치의 경로도 참조로 잡힌다.
    let e = d.edges.iter().find(|e| {
        e.from == "fixture_app::main"
            && e.to == "fixture_core::BASE"
            && e.kind == EdgeKind::References
    });
    assert!(e.is_some_and(|e| !e.tentative), "pattern path must resolve");
}

#[test]
fn boxed_dyn_receiver_stays_open() {
    let d = sem_doc();
    // Box<dyn Greet> — 조정 전 타입(Box)은 ADT지만 역참조 후 수신 타입은
    // dyn이라 디스패치가 열려 있다. impl 후보로 펼쳐지되 tentative여야 한다.
    let e = call(
        &d,
        "fixture_core::boxed_dispatch",
        "fixture_core::Used::<Greet>::greet",
    )
    .expect("trait-matrix candidate edge");
    assert!(
        e.tentative,
        "Box<dyn> receiver must keep dispatch tentative"
    );
    let decl = call(
        &d,
        "fixture_core::boxed_dispatch",
        "fixture_core::Greet::greet",
    )
    .expect("trait decl edge");
    assert!(decl.tentative);
}

#[test]
fn boxed_self_receiver_stays_open() {
    let d = sem_doc();
    // `consume(self: Box<Self>)`는 역참조 없이 Box<dyn> 그대로 받는다 —
    // 조정 후 타입이 Box(ADT)여도 인자 안이 dyn이면 디스패치는 열려 있다.
    let e = call(
        &d,
        "fixture_core::consume_dispatch",
        "fixture_core::Used::<Consume>::consume",
    )
    .expect("impl candidate edge");
    assert!(e.tentative, "self: Box<Self> on dyn must stay tentative");
    let decl = call(
        &d,
        "fixture_core::consume_dispatch",
        "fixture_core::Consume::consume",
    )
    .expect("trait decl edge");
    assert!(decl.tentative);
}

#[test]
fn concrete_generic_instance_stays_firm() {
    let d = sem_doc();
    // Wrap<()> — 제네릭 ADT라도 인자가 전부 구체적이면 디스패치는 닫힌다.
    // 기본 구현 상속은 선언점이 확정 타깃 — tentative로 떨어지면 회귀.
    let e = call(&d, "fixture_core::wrap_call", "fixture_core::Named::name")
        .expect("default method on concrete generic instance");
    assert!(!e.tentative, "Wrap<()> receiver is concrete");
}

#[test]
fn auto_trait_only_object_stays_open() {
    let d = sem_doc();
    // `&dyn Send` — principal trait이 없어 as_dyn_trait가 못 잡는다.
    // blanket impl 기본 메서드 호출은 열린 디스패치여야 한다.
    let e = call(
        &d,
        "fixture_core::poke_on_dyn_send",
        "fixture_core::Poke::poke",
    )
    .expect("default method on principal-less dyn object");
    assert!(e.tentative, "dyn Send receiver is not concrete");
}

#[test]
fn derived_method_call_targets_type() {
    let d = sem_doc();
    // `u.clone()` — Clone impl은 #[derive]가 만든다 — 생성 메서드는
    // 정점이 없으니 호출은 impl 대상 타입으로 귀속된다.
    let e = call(&d, "fixture_core::clone_used", "fixture_core::Used")
        .expect("derived method call resolves to the impl'd type");
    assert!(!e.tentative, "concrete receiver stays firm");
    // 생성 메서드 정점 이름으로 가는 간선은 없어야 한다(유령 정점 금지).
    assert!(call(
        &d,
        "fixture_core::clone_used",
        "fixture_core::Used::<Clone>::clone"
    )
    .is_none());
}

#[test]
fn proc_macro_call_keeps_crate_use() {
    let d = sem_doc();
    // proc 매크로 크레이트는 정점이 없는 타깃 종류라 선언 대신
    // 크레이트(루트 모듈) 정점으로 귀속한다.
    let e = call(&d, "fixture_core::proc_call", "fixture_macros")
        .expect("proc-macro use edge to the crate vertex");
    assert!(!e.tentative);
    // 확장은 proc 매크로 서버가 있을 때만 — 서버가 붙으면 확장 안의
    // 호출이 확정 간선으로, 없으면 proc-macro 전용 limitation이 실측한다.
    let expanded = call(&d, "fixture_core::proc_call", "fixture_core::util::helper")
        .is_some_and(|e| !e.tentative);
    // 서버 부재 전용 실측 문구만 매칭한다 — 일반 확장 실패 문구("…or
    // expansion failure")에 "proc-macro"가 들어있어 넓게 매칭하면
    // 무관한 실패가 이 단언을 통과시킨다.
    let srv_down = d
        .limitations
        .iter()
        .any(|l| l.contains("proc-macro invocations could not be expanded"));
    assert!(
        expanded || srv_down,
        "firm expansion edge or the proc-macro limitation"
    );
}

/// 속성 매크로가 감싼 impl — 메서드 정점이 실재하므로 호출은 타입이
/// 아니라 그 정점으로 간다.
#[test]
fn attr_macro_impl_keeps_method_vertex() {
    let d = sem_doc();
    let e = call(&d, "fixture_core::kept_ping", "fixture_core::Kept::ping")
        .expect("call edge to the real method vertex");
    assert!(!e.tentative);
    // 생성 impl 타입 귀속이면 생기는 잘못된 call 간선 — `Kept`는
    // 생성자 참조(References)로만 가야 한다.
    assert!(
        call(&d, "fixture_core::kept_ping", "fixture_core::Kept").is_none(),
        "call edge must not collapse to the type vertex"
    );
}

/// 블록 지역 `#[derive]` 타입의 생성 메서드 호출은 같은 이름의 모듈
/// 정점으로 귀속되면 안 된다 — 정규 ID 충돌이다.
#[test]
fn block_local_derive_does_not_collide() {
    let d = sem_doc();
    assert!(call(&d, "fixture_core::local_derived", "fixture_core::Local").is_none());
}

#[test]
fn out_dir_defs_resolve_to_module() {
    let d = sem_doc();
    // OUT_DIR 산출물 안의 정의들 — 정점은 없지만 소속 모듈 정점으로
    // 귀속돼야 한다(syn이 모듈 경로로 잡던 것과 같은 표면).
    // out_dirs 로드가 꺼져 있으면 이 간선은 만들어지지 않는다.
    let refs = |to: &str, kind: EdgeKind| {
        d.edges
            .iter()
            .any(|e| e.from == "fixture_core::uses_built" && e.to == to && e.kind == kind)
    };
    assert!(
        refs("fixture_core::built", EdgeKind::References),
        "const ref to module"
    );
    assert!(
        refs("fixture_core::built", EdgeKind::Call),
        "generated fn call to module"
    );
    // 크레이트 루트에 include!된 상수 — 소속 모듈이 크레이트 루트다.
    assert!(
        refs("fixture_core", EdgeKind::References),
        "root-included const"
    );
}

#[test]
fn merged_bin_root_body_uses_its_own_file() {
    let d = sem_doc();
    // lib와 같은 이름의 bin — 루트 합본에서 bin 본문의 파일은
    // src/bin/fixture_core.rs다. 대표 파일(lib.rs)을 쓰면 소스 정체 검사에
    // 걸려 syn 폴백이 되고 메서드 호출은 확정될 수 없다.
    let e = call(
        &d,
        "fixture_core::main",
        "fixture_core::Used::<Greet>::greet",
    )
    .expect("bin main body must be analyzed semantically");
    assert!(!e.tentative, "merged bin body resolved via types");
}

#[test]
fn block_local_ctor_does_not_collide() {
    let d = sem_doc();
    // local_ctor 안의 `struct local_scope` — 이름이 같아도 모듈 아이템과
    // 다른 정의다. 생성자 호출이 같은 이름의 정점으로 가면 안 된다.
    assert!(call(&d, "fixture_app::local_ctor", "fixture_app::local_scope").is_none());
}

/// 같은 ID로 충돌하는 두 트레이트 impl — 매크로가 만든 `b::Tr` impl의
/// 메서드는 `S::<Tr>::m`이라는 같은 문자열 ID를 갖지만 provenance가
/// 없다. syn이 수확한 `a::Tr` 쪽 정점으로 귀속되면 안 된다.
#[test]
fn generated_impl_does_not_steal_sibling_vertex() {
    let d = sem_doc();
    // a::Tr 디스패치 — syn이 수확한 진짜 메서드 정점으로 간다.
    assert!(
        call(&d, "fixture_core::dispatch_a", "fixture_core::S::<Tr>::m")
            .is_some_and(|e| e.tentative)
    );
    // b::Tr 디스패치 — 확장 안의 메서드는 정점이 없으니 impl 대상 타입으로.
    assert!(call(&d, "fixture_core::dispatch_b", "fixture_core::S").is_some_and(|e| e.tentative));
    // 충돌하는 a::Tr 정점으로 가는 간선은 만들어지면 안 된다.
    assert!(call(&d, "fixture_core::dispatch_b", "fixture_core::S::<Tr>::m").is_none());
}

/// span 보존 생성 메서드 — proc 매크로가 입력 토큰의 위치를 재사용해
/// 만든 `c::Tr` impl 메서드는 이름 앵커까지 `a::Tr`의 진짜 선언과 겹친다.
/// 위치만으로는 구분이 안 되므로 트레이트 정체(`a::Tr` vs `c::Tr`)로
/// 가려야 한다 — `S::<Tr>::m`이 아니라 `S`로 귀속돼야 한다.
#[test]
fn span_preserved_generated_method_does_not_steal() {
    let d = sem_doc();
    assert!(call(&d, "fixture_core::dispatch_c", "fixture_core::S").is_some_and(|e| e.tentative));
    assert!(call(&d, "fixture_core::dispatch_c", "fixture_core::S::<Tr>::m").is_none());
}

/// 속성 매크로가 익명 const로 감싼 진짜 impl — 확장이 블록을 추가해도
/// 메서드 정점은 syn provenance가 확인되므로 확정 간선을 유지한다.
/// 타입 정점으로 떨어지면 지역성 검사가 진짜 선언을 거절한 것이다.
#[test]
fn const_wrapped_real_impl_keeps_method_vertex() {
    let d = sem_doc();
    let e = call(&d, "fixture_core::wrap_ping", "fixture_core::Cloaked::ping")
        .expect("call edge to the real method vertex");
    assert!(!e.tentative);
    assert!(call(&d, "fixture_core::wrap_ping", "fixture_core::Cloaked").is_none());
}

/// fn 안 `#[path]` 모듈 — 같은 파일을 가리키는 지역 모듈의 정의는
/// 원본 위치가 모듈 선언과 같다. 지역 정의가 `fixture_core::shared::*`
/// 정점으로 귀속되면 모듈 선언을 훔치는 것이다.
/// 대조군: 모듈 레벨 `shared`의 같은 호출은 정점으로 확정 해석된다 —
/// 정점이 실재함과 해석이 동작함을 보인다.
#[test]
fn fn_local_path_module_does_not_steal() {
    let d = sem_doc();
    // 대조군 — 모듈 레벨 shared의 같은 정의들은 정점으로 해석된다.
    assert!(call(
        &d,
        "fixture_core::use_shared",
        "fixture_core::shared::Shared::val"
    )
    .is_some_and(|e| !e.tentative));
    assert!(call(
        &d,
        "fixture_core::use_shared",
        "fixture_core::shared::helper"
    )
    .is_some_and(|e| !e.tentative));
    // 지역 모듈의 같은 호출은 그 정점으로 가면 안 된다.
    assert!(!d.edges.iter().any(|e| {
        e.from == "fixture_core::local_shadowed" && e.to.starts_with("fixture_core::shared")
    }));
}

/// `impl Gen<u8>`/`impl Gen<u16>` — 정점 ID는 같고 본문은 다르다.
/// 인덱스가 한 항목만 저장하면 다른 쪽 본문의 간선이 빠진다.
/// 메서드 호출의 *확정* 간선으로 검증한다 — syn 폴백은 메서드 호출을
/// 추정으로만 만들어 같은 단언을 위조할 수 없다.
#[test]
fn generic_impls_sharing_id_keep_both_bodies() {
    let d = sem_doc();
    assert!(
        call(&d, "fixture_core::Gen::pick", "fixture_core::Used::doubled")
            .is_some_and(|e| !e.tentative)
    );
    assert!(call(
        &d,
        "fixture_core::Gen::pick",
        "fixture_core::Used::quadrupled"
    )
    .is_some_and(|e| !e.tentative));
}

/// 익명 const 안의 생성 impl(serde_derive 패턴) — self 타입이 모듈
/// 레벨이면 디스패치 후보로 유효해 impl 대상 타입으로 귀속된다.
#[test]
fn const_wrapped_generated_impl_keeps_owner() {
    let d = sem_doc();
    assert!(
        call(&d, "fixture_core::dyn_dispatch", "fixture_core::IntOrFloat")
            .is_some_and(|e| e.tentative)
    );
}

/// 매크로가 함수 안에서 만든 지역 타입 — 확장 구문만으로는 감싼 함수가
/// 안 보이지만, 호출의 impl owner가 모듈 레벨 같은-이름 정점으로 귀속되면
/// 안 된다.
#[test]
fn macro_generated_local_type_does_not_collide() {
    let d = sem_doc();
    // 생성자 참조와 생성 메서드 호출 둘 다 모듈 정점으로 새면 안 된다.
    assert!(!d
        .edges
        .iter()
        .any(|e| e.from == "fixture_core::gen_local" && e.to.starts_with("fixture_core::Shadow")));
}

/// 속성 매크로가 직접 붙은 fn — 확장은 소비된 속성을 빼므로 ra의 아이템
/// 범위가 syn보다 짧다. provenance 대조는 이름 토큰 앵커여야 한다.
#[test]
fn attr_macro_fn_keeps_provenance() {
    let d = sem_doc();
    assert!(
        call(&d, "fixture_core::call_kept", "fixture_core::kept_fn").is_some_and(|e| !e.tentative)
    );
}

/// 메서드 정점도 ADT owner도 없는 impl — 후보를 조용히 버리면
/// limitation이 거짓말을 하므로 디스패치 후보 전용 카운터로 세어져야 한다.
/// fixture에서 표현 불가 후보는 정확히 하나다: `impl Primitive for u8`.
/// (`impl<T: ?Sized> Poke for T` 같은 빈 blanket impl은 디스패치가
/// 트레이트 기본 구현 정점으로 해석되므로 후보가 표현 가능하다.)
#[test]
fn unrepresentable_candidate_is_counted() {
    let d = sem_doc();
    // 트레이트 선언점에는 추정 간선이 간다.
    assert!(call(
        &d,
        "fixture_core::prim_dispatch",
        "fixture_core::Primitive::hit"
    )
    .is_some_and(|e| e.tentative));
    // 표현 불가 후보는 간선이 아니라 전용 limitation으로 실측된다 —
    // 무관한 external 경로가 이 단언을 통과시키지 않게 수치를 고정한다.
    let n = d
        .limitations
        .iter()
        .find_map(|l| {
            l.split_once(" trait-dispatch candidates have no graph vertex")
                .and_then(|(n, _)| n.parse::<usize>().ok())
        })
        .expect("unrepresentable-candidate limitation");
    assert_eq!(n, 1, "u8 primitive impl only");
}

#[test]
fn syn_mode_still_fans_out() {
    // 기본 모드 계약은 그대로 — 같은-이름 팬아웃이 추정 간선으로 남는다.
    let s = syn_doc();
    let e = call(
        &s,
        "fixture_app::main",
        "fixture_core::Used::<Greet>::greet",
    )
    .expect("syntactic fan-out edge");
    assert!(e.tentative);
    assert!(
        call(&s, "fixture_app::main", "fixture_core::Other::greet").is_some_and(|e| e.tentative)
    );
}
