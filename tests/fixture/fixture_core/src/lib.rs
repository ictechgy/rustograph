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
