mod extra;

use fixture_core::Greet;
use fixture_core::Named;
use fixture_core::Used as Renamed;
use fixture_core::inline::*;
#[cfg(feature = "never")]
use fixture_core::cfg_gated;

fn main() {
    let u = Renamed { v: 1 };
    u.greet();
    let _quad = u.quadrupled();
    extra::run();
    let _c = fixture_core::Color::Red;
    println!("{}", fixture_core::entry());
    println!("{}", inline_fn());
    let _via_macro = fixture_core::emit_helper!();
    let _d = fixture_core::dyn_dispatch(&u);
    let _g = fixture_core::generic_dispatch(&u);
    // Box<dyn> 수신자 — 조정 후 타입이 dyn이라 후보 행렬이어야 한다.
    let _b = fixture_core::boxed_dispatch(Box::new(Renamed { v: 2 }));
    // self: Box<Self> 수신자 — 조정 후에도 Box<dyn>이지만 역시 열린다.
    let _cd = fixture_core::consume_dispatch(Box::new(Renamed { v: 4 }));
    // 기본 구현 상속 — 구체 수신자라 선언점 디폴트가 확정 타깃이다.
    let _n = u.name();
    // fn 아이템을 담은 지역 바인딩 — callable 해석이 ffi_entry까지 따라간다.
    let f = fixture_core::ffi_entry;
    let _fv = f();
    // 패턴 위치의 경로 — 상수 패턴 참조다(BASE는 여기서만 참조된다).
    match _n {
        fixture_core::BASE => {}
        _ => {}
    }
    local_scope();
    local_ctor();
}

/// 블록 지역 생성자 — 이름이 모듈 아이템과 같아도 다른 정의다.
/// callable 해석이 지역 생성자를 찾아도 `fixture_app::local_scope`
/// 정점으로의 간선으로 오인하면 안 된다.
fn local_ctor() {
    struct local_scope(u8);
    let mk = local_scope;
    let _ = mk(1);
}

/// 블록 지역 정의 — `inner` 본문의 호출은 `local_scope`의 것이 아니다.
fn local_scope() {
    fn inner() -> u32 {
        fixture_core::util::helper()
    }
    let _ = inner();
}
