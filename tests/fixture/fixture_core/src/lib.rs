//! fixture core 크레이트 — 수확기 계약을 검증하는 최소 입력.

pub mod util;
#[path = "gen.rs"]
pub mod gen;
pub mod nested;

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
