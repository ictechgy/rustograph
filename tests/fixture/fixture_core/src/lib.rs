//! fixture core 크레이트 — 수확기 계약을 검증하는 최소 입력.

pub mod util;
#[path = "gen.rs"]
pub mod gen;
pub mod nested;

#[derive(Clone)]
pub struct Used {
    pub v: u32,
}

pub struct Unused {
    pub v: u32,
}

pub enum Color {
    Red,
    Green,
}

pub union IntOrFloat {
    pub i: u32,
    pub f: f32,
}

pub static GLOBAL_SEED: u32 = 7;

pub type Score = u32;

pub const BASE: u32 = 40;

macro_rules! local_shout {
    ($x:expr) => {
        $x + 1
    };
}

pub trait Greet {
    fn greet(&self) -> u32;
}

/// 같은 이름의 메서드를 가진 무관한 타입 — syn 이름 팬아웃은 여기에도
/// 간선을 만들지만, 의미 해석은 만들지 않아야 한다(정밀도 검증용).
pub struct Other;

impl Other {
    pub fn greet(&self) -> u32 {
        0
    }
}

pub trait Named {
    fn name(&self) -> u32 {
        9
    }
}

impl Greet for Used {
    fn greet(&self) -> u32 {
        self.v
    }
}

impl Named for Used {}

impl std::fmt::Display for Used {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.v)
    }
}

impl Used {
    pub fn doubled(&self) -> u32 {
        self.v * 2
    }

    pub fn quadrupled(&self) -> u32 {
        self.doubled() * 2
    }
}

/// 표현식 매크로 — 확장하면 crate 안 helper 호출이 나온다.
/// syn은 빈 토큰만 보지만, 의미 해석은 확장 트리 안의 호출을 잡는다.
#[macro_export]
macro_rules! emit_helper {
    () => {
        $crate::util::helper()
    };
}

/// dyn 수신자의 트레이트 디스패치 — 실제 impl은 런타임에 정해지므로
/// 의미 해석도 워크스페이스 impl 후보 행렬로 펼친다(추정 간선).
pub fn dyn_dispatch(x: &dyn Greet) -> u32 {
    x.greet()
}

/// 제네릭 수신자 — 구체 impl을 모르니 같은 후보 행렬이다.
pub fn generic_dispatch<T: Greet>(t: &T) -> u32 {
    t.greet()
}

/// Box<dyn> 수신자 — 조정 전 타입(Box)은 ADT지만 역참조 후 수신 타입은
/// dyn이라 디스패치가 열려 있다. 조정 후 타입으로 판정해야 한다.
pub fn boxed_dispatch(x: Box<dyn Greet>) -> u32 {
    x.greet()
}

/// Box<Self> 수신자 트레이트 — 역참조 없이 Box<dyn> 그대로 받는 메서드는
/// 조정 후 타입도 Box(ADT)라 겉 타입만 보면 닫힌 디스패치로 오인된다.
pub trait Consume {
    fn consume(self: Box<Self>) -> u32;
}

impl Consume for Used {
    fn consume(self: Box<Self>) -> u32 {
        self.v
    }
}

/// Box<dyn>의 self: Box<Self> 호출 — 실제 impl은 런타임에 정해진다.
pub fn consume_dispatch(x: Box<dyn Consume>) -> u32 {
    x.consume()
}

/// 제네릭 ADT — 인자가 튜플이어도 구체 인스턴스면 디스패치는 닫혀 있다.
pub struct Wrap<T>(pub T);

/// 구체 인스턴스의 기본 구현 상속 — 선언점 디폴트가 확정 타깃이다.
impl Named for Wrap<()> {}

/// Wrap<()>의 기본 메서드 호출 — 구체라 `Named::name`이 확정이다.
pub fn wrap_call() -> u32 {
    Wrap(()).name()
}

/// blanket impl 트레이트 — `dyn Send`처럼 principal trait이 없는 객체도
/// 수신자가 될 수 있다. as_dyn_trait는 principal만 돌려주므로 겉 kind가
/// Dynamic인지 별도로 잡아야 한다.
pub trait Poke {
    fn poke(&self) -> u32 {
        3
    }
}

impl<T: ?Sized> Poke for T {}

/// `&dyn Send` 수신자 — auto trait만 가진 객체. 디스패치는 열려 있다.
pub fn poke_on_dyn_send(x: &dyn Send) -> u32 {
    x.poke()
}

/// derive가 만든 impl 메서드 호출 — 생성 메서드는 정점이 없으므로
/// 간선은 impl 대상 타입(Used)으로 귀속돼야 한다.
pub fn clone_used(u: &Used) -> Used {
    u.clone()
}

/// proc 매크로 호출 — 서버가 붙으면 확장 안의 `util::helper` 호출이
/// 확정 간선으로 잡힌다. 서버가 없으면 unexpanded로 계측된다.
pub fn proc_call() -> u32 {
    fixture_macros::emit_helper_call!()
}

/// 속성 매크로가 그대로 돌려주는 타입 — impl은 매크로 확장 안에 있지만
/// `Kept::ping` 정점은 syn이 소스에서 만들었다.
pub struct Kept;

#[fixture_macros::keep]
impl Kept {
    pub fn ping(&self) -> u32 {
        7
    }
}

/// 속성 매크로 impl 메서드 호출 — 생성 impl 귀속이 아니라 실제
/// 메서드 정점 `Kept::ping`으로 가야 한다.
pub fn kept_ping() -> u32 {
    Kept.ping()
}

/// 속성 매크로가 직접 붙은 fn — 확장에서 소비된 속성만큼 ra의 아이템
/// 범위가 syn보다 짧다. 아이템 범위 동등 비교는 이 선언을 거절한다.
#[fixture_macros::keep]
pub fn kept_fn() -> u32 {
    11
}

/// `#[keep]` fn 호출 — provenance는 이름 토큰으로 대조해야 통과한다.
pub fn call_kept() -> u32 {
    kept_fn()
}

/// 모듈 레벨 `Local` — 아래 블록 지역 타입과 이름이 같다.
pub struct Local;

/// 블록 지역 `#[derive]` 타입 — `l.clone()`이 모듈 레벨 `Local`
/// 정점으로의 확정 간선을 만들면 안 된다.
pub fn local_derived() -> u32 {
    #[derive(Clone)]
    struct Local(u32);
    let l = Local(1);
    l.clone().0
}

/// 마지막 세그먼트가 같은 두 트레이트 — `a::Tr`/`b::Tr` 둘 다 메서드
/// 정점 ID `S::<Tr>::m`을 만든다. syn이 수확하는 것은 `a::Tr` impl뿐이다.
pub mod a {
    pub trait Tr {
        fn m(&self) -> u32;
    }

    /// `forge_sibling` 입력 메서드의 본문 호출 — 진짜 사본의 간선.
    pub fn probe() -> u32 {
        7
    }
}
pub mod b {
    pub trait Tr {
        fn m(&self) -> u32;
    }
}
pub mod c {
    pub trait Tr {
        fn m(&self) -> u32;
    }
}

/// 속성 매크로가 `crate::passthrough! { .. }` 안에 입력을 숨길 때
/// 쓰는 전달 매크로 — 토큰 트리 안의 사본은 확장 없이는 보이지 않는다.
macro_rules! passthrough {
    ($($t:tt)*) => {
        $($t)*
    };
}
pub(crate) use passthrough;

/// `forge_sibling`이 만드는 위조 형제 impl의 본문 호출 — 잘못 귀속되면
/// 이 정점으로의 간선이 생긴다.
pub fn forged_target() -> u32 {
    0
}

pub struct S4;

// 입력은 `passthrough!` 안에 숨겨지고, 형제 impl은 `c::Tr` 내용의
// 토큰에 `a::Tr`의 span을 위조해 단다 — 헤더 원본 범위 대조를 통과하면서
// 다른 트레이트로 해석되는 사본이다. 원본이 함수형 매크로 안에 숨어
// 있으므로 확장을 재귀하지 않으면 위조 사본이 단독 후보로 채택된다.
#[fixture_macros::forge_sibling]
impl a::Tr for S4 {
    fn m(&self) -> u32 {
        a::probe()
    }
}

/// 같은 물리 파일을 가리키는 두 모듈 — `super::` 해석이 문맥에 따라
/// 다르다. ra가 임의의 문맥으로 def를 묶으면 잘못된 본문이 귀속된다.
pub mod outer_a;
pub mod outer_b;

/// 비-mod.rs 파일 모듈 — 안의 `#[path]`는 `src/`(파일 디렉터리)가
/// 기준이지 `src/single/`(module_dir)이 아니다.
pub mod single;

pub struct S5;

// `forge_via_attr` — 원본 impl이 `#[emit_args(..)]`의 인자 토큰 안에
// 숨는다. 확장 트리의 아이템 속성 매크로까지 재귀 확장하지 않으면
// 위조 형제가 단독 후보가 된다.
#[fixture_macros::forge_via_attr]
impl a::Tr for S5 {
    fn m(&self) -> u32 {
        a::probe()
    }
}

pub struct S6;

// `forge_via_fn` — `forge_via_attr`와 같은 구조인데 인자를 숨기는
// 캐리어가 fn이다. 확장 트리에서 fn 아이템의 속성을 건너뛰면
// (자기 자신을 조상으로 오인) 원본 사본이 안 보인다.
#[fixture_macros::forge_via_fn]
impl a::Tr for S6 {
    fn m(&self) -> u32 {
        a::probe()
    }
}

pub struct S7;

// `forge_via_derive` — 위조 형제가 보이고, derive 출력의 `mod dup_hid`
// 안 `fn m`이 숨은 후보다. derive 확장을 걷지 않으면 위조가 단독이다.
#[fixture_macros::forge_via_derive]
impl a::Tr for S7 {
    fn m(&self) -> u32 {
        a::probe()
    }
}

pub struct S9;

// `forge_via_cfg` — 죽은 `cfg_attr` 안에 원본 사본이 들어 있다.
// 미평가 cfg_attr의 안쪽은 검증할 수 없으므로 애매로 빠져야 한다.
#[fixture_macros::forge_via_cfg]
impl a::Tr for S9 {
    fn m(&self) -> u32 {
        a::probe()
    }
}

/// 인라인 조상의 `#[path]`가 자식 모듈의 기준 디렉터리 세그먼트를
/// 덮어쓴다 — `inner` 대신 `deep`이 들어가 `src/nest/deep/leaf.rs`다.
pub mod nest {
    #[path = "deep"]
    pub mod inner {
        #[path = "leaf.rs"]
        pub mod leaf;
    }
}

/// `nest::inner::leaf`의 함수를 호출한다 — `#[path]` 오버라이드가
/// 풀리지 않으면 leaf 모듈 자체가 없어 이 간선도 없다.
pub fn call_leaf() -> u32 {
    nest::inner::leaf::leaf_probe()
}

pub struct S;

// 속성 매크로가 입력을 재emit하면서 `impl c::Tr for S`를 span 보존으로
// 추가한다 — 생성 메서드의 이름 위치가 이 impl의 `fn m`과 겹친다.
#[fixture_macros::spawn_sibling]
impl a::Tr for S {
    fn m(&self) -> u32 {
        1
    }
}

macro_rules! emit_impl {
    ($i:item) => {
        $i
    };
}

// 확장 안의 impl — syn은 매크로를 펼치지 않으므로 정점·provenance가 없다.
// fn_id 문자열은 a::Tr 쪽 정점 `S::<Tr>::m`과 충돌한다.
emit_impl! {
    impl b::Tr for S {
        fn m(&self) -> u32 {
            2
        }
    }
}

/// `a::Tr` 디스패치 — 진짜 메서드 정점으로 가야 한다.
pub fn dispatch_a(x: &dyn a::Tr) -> u32 {
    x.m()
}

/// `b::Tr` 디스패치 — 확장 안의 메서드는 정점이 없으니 impl 대상 타입으로
/// 귀속한다. 같은 문자열 ID의 `a::Tr` 정점으로 가면 안 된다.
pub fn dispatch_b(x: &dyn b::Tr) -> u32 {
    x.m()
}

// 루트 스코프의 `use` — 아래 `impl Tr for S2`의 `Tr`을 `a::Tr`로
// 해석시킨다. 생성 impl이 익명 const 안의 `use crate::b::Tr`로 같은
// 철자를 다른 트레이트에 가린다.
use a::Tr;

pub struct S2;

// 생성된 형제 impl은 소스 표기가 진짜와 같다(`impl Tr for S2`) — 하지만
// 확장 안의 `use`로 `Tr`이 `b::Tr`을 가리키므로 해석된 정체가 다르다.
#[fixture_macros::spawn_shadowed]
impl Tr for S2 {
    fn m(&self) -> u32 {
        1
    }
}

/// 진짜 `Tr`(=`a::Tr`) 메서드 — `S2::<Tr>::m` 정점으로 가야 한다.
pub fn dispatch_shadow_a(x: &S2) -> u32 {
    x.m()
}

/// 생성 `Tr`(=`b::Tr`) 메서드 — 정점이 없으니 impl 대상 `S2`로 가야
/// 하고, 소스 표기가 같아도 `S2::<Tr>::m`을 훔치면 안 된다.
pub fn dispatch_shadow_b(x: &S2) -> u32 {
    <S2 as b::Tr>::m(x)
}

pub mod d {
    /// 제네릭 트레이트 — 인자만 다른 impl(`G<u8>`/`G<u16>`)은 메서드
    /// 정점 ID가 같다(`S3::<G>::m`).
    pub trait G<T> {
        fn m(&self) -> u32;
    }
}

pub struct S3;

// 생성된 형제 impl은 `d::G<u16>` — 해석된 트레이트는 진짜 `d::G<u8>`와
// 같으므로 소스 표기의 인자 부분으로 가려야 한다.
#[fixture_macros::spawn_generic_sibling]
impl d::G<u8> for S3 {
    fn m(&self) -> u32 {
        1
    }
}

/// 진짜 `G<u8>` 메서드 — `S3::<G>::m` 정점으로.
pub fn dispatch_g8(x: &S3) -> u32 {
    <S3 as d::G<u8>>::m(x)
}

/// 생성 `G<u16>` 메서드 — impl 대상 `S3`으로, `S3::<G>::m`이 아니라.
pub fn dispatch_g16(x: &S3) -> u32 {
    <S3 as d::G<u16>>::m(x)
}

/// `c::Tr` 디스패치 — proc 매크로가 span을 보존해 만든 impl의 메서드는
/// 이름 위치까지 `a::Tr`의 진짜 선언과 겹친다. 위치만으로는 구분이 안
/// 되므로 트레이트 정체로 가려야 한다 — `S::<Tr>::m`이 아니라 `S`로.
pub fn dispatch_c(x: &dyn c::Tr) -> u32 {
    x.m()
}

/// 제네릭 타입의 서로 다른 구체 impl — 정점 ID는 둘 다 `Gen::pick`이고
/// 본문은 각각 다르다. 한 항목만 저장하면 다른 쪽 본문 간선이 빠진다.
/// 메서드 호출로 검증한다 — 경로 호출·상수 참조는 syn 폴백도 만들므로
/// 의미 해석이 실제로 그 본문을 걸었는지 구분이 안 된다.
pub struct Gen<T>(pub T);

impl Gen<u8> {
    pub fn pick(&self) -> u32 {
        Used { v: 1 }.doubled()
    }
}

impl Gen<u16> {
    pub fn pick(&self) -> u32 {
        Used { v: 1 }.quadrupled()
    }
}

macro_rules! emit_wrapped_impl {
    ($t:ty) => {
        const _: () = {
            impl Greet for $t {
                fn greet(&self) -> u32 {
                    5
                }
            }
        };
    };
}

// serde_derive 패턴 — 익명 const 안의 생성 impl. impl 소스는 확장
// 안에 있어 블록 조상을 가지지만, self 타입은 모듈 레벨이라 후보다.
emit_wrapped_impl!(IntOrFloat);

/// 모듈 레벨 `Shadow` — 아래 매크로 생성 지역 타입과 이름이 같다.
pub struct Shadow;

macro_rules! emit_local_ty {
    () => {
        struct Shadow(u32);
        impl Shadow {
            fn val(&self) -> u32 {
                self.0
            }
        }
    };
}

/// 매크로가 함수 안에서 만든 지역 `Shadow` — `s.val()`의 impl owner가
/// 모듈 레벨 `Shadow` 정점으로 귀속되면 안 된다. 확장 구문의 조상만
/// 보면 감싼 함수가 안 보이므로 확장 인지 조상 검사가 필요하다.
pub fn gen_local() -> u32 {
    emit_local_ty!();
    Shadow(5).val()
}

/// 원시 타입 impl — 메서드 정점도 ADT owner도 없어 그래프로 표현
/// 불가다. 디스패치 지점에서 external로 세어져야 한다.
pub trait Primitive {
    fn hit(&self) -> u32;
}

impl Primitive for u8 {
    fn hit(&self) -> u32 {
        *self as u32
    }
}

/// `dyn Primitive` 디스패치 — 유일한 impl 후보가 표현 불가다.
pub fn prim_dispatch(x: &dyn Primitive) -> u32 {
    x.hit()
}

/// build.rs가 OUT_DIR에 쓴 파일 — `load_out_dirs_from_check` 없이는
/// 해석되지 않는다.
pub mod built {
    include!(concat!(env!("OUT_DIR"), "/built_defs.rs"));
}

// 크레이트 루트에 직접 include!된 정의 — 소속 모듈이 크레이트 루트다.
include!(concat!(env!("OUT_DIR"), "/root_defs.rs"));

/// OUT_DIR 생성 정의 참조 — `built` 안의 상수·함수와 루트 상수.
/// out_dirs가 로드되면 모듈 정점으로 귀속되고, 아니면 미해석으로 센다.
pub fn uses_built() -> u32 {
    built::BUILT_ANSWER + built::built_answer() + ROOT_ANSWER
}

/// 속성 매크로가 익명 const로 감싼 진짜 impl — syn이 만든
/// `Cloaked::ping` 정점이 실재하므로 확장이 const 블록을 추가해도 호출은
/// 그 메서드 정점으로 가야 한다(타입 정점으로 떨어지면 안 된다).
pub struct Cloaked;

#[fixture_macros::wrap_in_const]
impl Cloaked {
    pub fn ping(&self) -> u32 {
        7
    }
}

/// const 래퍼 안의 진짜 메서드 호출 — `fixture_core::Cloaked::ping`으로.
pub fn wrap_ping() -> u32 {
    Cloaked.ping()
}

/// 같은 파일을 두 모듈이 가리킨다 — 모듈 레벨 `shared`는 정점을 만들고,
/// fn 안 지역 모듈은 그 정점을 훔치면 안 된다.
#[path = "shared.rs"]
pub mod shared;

/// fn 안의 `#[path]` 모듈 — shared.rs를 다시 로드하지만 지역 스코프다.
/// 여기서 해석된 `Shared::val`/`helper`가 `fixture_core::shared::*`
/// 정점으로 귀속되면 같은 이름의 모듈 선언을 훔치는 셈이다.
pub fn local_shadowed() -> u32 {
    #[path = "shared.rs"]
    mod local_shared;
    local_shared::Shared::val() + local_shared::helper()
}

/// fn 안의 지역 모듈이 모듈 선언과 *같은 이름*일 때 — 정규 ID까지
/// 완전히 겹치므로 지역성 판정이 없으면 정점을 통째로 훔친다.
pub fn local_shadowed_same_name() -> u32 {
    #[path = "shared.rs"]
    mod shared;
    shared::Shared::val() + shared::helper()
}

/// 모듈 레벨 대조군 — 같은 파일을 가리키는 모듈 레벨 `shared`의 정의는
/// 진짜 정점으로 해석되어 확정 간선이 생겨야 한다.
pub fn use_shared() -> u32 {
    shared::Shared::val() + shared::helper()
}

pub fn entry() -> u32 {
    util::helper() + local_shout!(0)
}

#[no_mangle]
pub extern "C" fn ffi_entry() -> u32 {
    0
}

#[cfg(feature = "never")]
pub fn cfg_gated() {}

#[cfg(unix)]
pub mod unix_only;

/// unsafe fn — 경계의 안쪽 정점.
pub unsafe fn raw_read(p: *const u32) -> u32 {
    unsafe { *p }
}

/// 본문에 unsafe 블록이 있는 안전한 fn — 안쪽 표시 + 진입 간선.
pub fn safe_wrapper(p: &u32) -> u32 {
    unsafe { raw_read(p as *const u32) }
}

/// unsafe trait — 구현이 경계를 넘는다.
pub unsafe trait RawBytes {}

unsafe impl RawBytes for Used {}

/// 트레이트 안의 unsafe fn 선언.
pub trait PtrMath {
    unsafe fn deref_raw(&self) -> u32;
}

fn dead_private() -> u32 {
    0
}

pub mod inline {
    pub fn inline_fn() -> u32 {
        7
    }
}

#[cfg(test)]
mod t {
    #[test]
    fn t1() {
        assert_eq!(crate::util::helper(), 41);
    }
}
