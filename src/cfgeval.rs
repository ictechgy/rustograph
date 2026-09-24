//! `#[cfg(...)]` 조건 평가기 — `--target`·`--exclude-tests` 필터의 순수 부분.
//!
//! 조건은 `cfg_of`가 저장한 토큰 문자열(`target_os = "windows"`,
//! `all(unix , feature = "x")`). 평가는 삼값 논리다 — 모르는 조건은
//! keep으로 처리하고, 거짓으로 판명된 것만 문서에서 지운다.
//!
//! 팩트는 두 경로로 온다: `rustc --print cfg --target`의 권위 있는
//! 출력(rustc=true — 잘 알려진 플래그와 target_* 키의 부재=거짓)과,
//! rustc를 못 쓸 때의 트리플 문자열 추정(이름 기반 분류 — 모르는
//! 토큰은 비운다). 두 경우 모두 프로필·Cargo가 정하는 조건
//! (feature·debug_assertions·panic·커스텀 --cfg)은 미지로 남는다.
//! 트리플 구성 요소를 cfg 값으로 직역하면 안 된다 — `i686`의
//! target_arch는 `"x86"`이고 `gnueabihf`의 env는 `"gnu"`이며
//! emscripten은 `unix`와 `wasm` 두 family를 갖는다.

use std::collections::{BTreeMap, BTreeSet};

/// cfg 평가의 사실 집합. `pairs`는 `name = "value"` 술어의 값 집합
/// (target_family처럼 같은 키가 여러 번 나올 수 있다), `flags`는
/// `unix`·`windows`·`test` 같은 bare 조건이다. 모르는 키는 맵에
/// 없음으로 표현한다 — 거짓이 아니라 미지다.
///
/// rustc --print cfg 출력은 *기본* 빌드 설정이다 — Cargo feature·
/// build.rs --cfg·프로필(debug_assertions·panic)은 거기 없거나 기본값만
/// 있다. 그래서 부재를 곧바로 거짓으로 읽으면 커스텀 cfg가 오삭제된다.
/// 부재=거짓은 두 경우로만 제한한다: platform이 확정된 팩트에서 잘 알려진
/// 빌트인 플래그(KNOWN_FLAGS), rustc 출력이 닫힌 키 우주인 target_* 키.
/// 프로필·RUSTFLAGS가 정하는 조건(VOLATILE)은 팩트에 있어도 미지다.
#[derive(Debug, Default)]
pub struct Facts {
    pub pairs: BTreeMap<String, BTreeSet<String>>,
    /// bare 플래그 팩트 — 값 false는 "확실히 끔"(test_off의 test=false).
    pub flags: BTreeMap<String, bool>,
    /// 실제 타깃을 서술하는 팩트 — KNOWN_FLAGS의 부재=거짓이 허용된다.
    pub platform: bool,
    /// rustc --print cfg 실측 — target_* 키의 부재=거짓이 허용된다.
    pub rustc: bool,
}

/// Cargo 프로필·RUSTFLAGS·도구 호출이 정하는 조건 — rustc 기본 출력에
/// 있어도(없어도) 참/거짓을 증명할 수 없다. feature는 Cargo가 주는
/// 것이라 rustc 출력이 아예 모른다.
const VOLATILE: &[&str] = &[
    "feature",
    "panic",
    "debug_assertions",
    "target_feature",
    "relocation_model",
    "sanitize",
    "sanitizer_cfi",
    "coverage",
    "instrument_coverage",
    "ub_checks",
    "contract_checks",
    "miri",
    "clippy",
    "rustfmt",
];

/// rustc/cargo가 정의하는 닫힌 bare 플래그 — platform 팩트에서 부재면
/// 거짓으로 증명할 수 있다. 커스텀 --cfg 플래그는 여기 없으니 미지로 남는다.
const KNOWN_FLAGS: &[&str] = &["unix", "windows", "test", "doc", "doctest", "proc_macro"];

impl Facts {
    /// `rustc --print cfg --target` 출력 한 줄씩을 팩트로 적재한다.
    /// `name = "value"`와 bare 조건 두 형태다. platform+rustc=true —
    /// 잘 알려진 플래그와 target_* 키의 부재는 거짓으로 읽어도 된다.
    /// 단 VOLATILE 키는 적재하되 평가하지 않는다 — 기본값이 실제 빌드의
    /// 값이 아닐 수 있기 때문이다.
    pub fn from_cfg_lines<'a>(lines: impl Iterator<Item = &'a str>) -> Facts {
        let mut f = Facts {
            platform: true,
            rustc: true,
            ..Default::default()
        };
        for line in lines {
            let line = line.trim();
            if let Some((k, v)) = line.split_once('=') {
                let v = v.trim().trim_matches('"');
                f.pairs
                    .entry(k.trim().to_string())
                    .or_default()
                    .insert(v.to_string());
            } else if !line.is_empty() {
                f.flags.insert(line.to_string(), true);
            }
        }
        f
    }

    /// rustc를 못 쓸 때의 트리플 추정 — 알려진 매핑만 번역하고 모르는
    /// 것은 비운다 — 잘못된 팩트보다 미지가 낫다(미지는 keep이고
    /// 잘못은 오삭제다). 구성 요소를 위치로 읽지 않고 이름으로 분류한다:
    /// `thumbv7em-none-eabihf`의 eabihf는 env가 아니라 ABI 표시이고,
    /// `wasm32-wasip1`의 wasip1은 os=wasi+env=p1이며, `muslabi64`는
    /// env=musl이다 — 위치로 직역하면 셋 다 틀린다.
    pub fn from_triple(triple: &str) -> Facts {
        let mut f = Facts {
            platform: true,
            ..Default::default()
        };
        let parts: Vec<&str> = triple.split('-').collect();
        if parts.len() < 2 {
            return f;
        }
        if let Some((arch, width, endian)) = arch_facts(parts[0]) {
            f.set("target_arch", arch);
            f.set("target_pointer_width", width);
            f.set("target_endian", endian);
        }
        for tok in &parts[1..] {
            // 합성 토큰을 먼저 — androideabi는 os+env를 같이 정한다.
            match *tok {
                "androideabi" => {
                    f.set("target_os", "android");
                    f.set("target_env", "");
                    f.set("target_abi", "eabi");
                    continue;
                }
                "wasip1" => {
                    f.set("target_os", "wasi");
                    f.set("target_env", "p1");
                    continue;
                }
                "wasip2" => {
                    f.set("target_os", "wasi");
                    f.set("target_env", "p2");
                    continue;
                }
                _ => {}
            }
            if let Some(v) = vendor_of(tok) {
                f.set("target_vendor", v);
            } else if let Some(os) = os_of(tok) {
                f.set("target_os", os);
                for fam in families_of(os) {
                    f.add_family(fam);
                }
            } else if let Some((env, abi, width32)) = env_of(tok) {
                f.set("target_env", env);
                if let Some(a) = abi {
                    f.set("target_abi", a);
                }
                if width32 {
                    f.set("target_pointer_width", "32");
                }
            }
            // 모르는 토큰(abi64·eabihf 단독·freestanding 등)은 넘긴다 —
            // 추측으로 채운 팩트는 없는 것보다 나쁘다.
        }
        // rustc는 env를 항상 출력한다(없으면 "") — OS를 알고 env 토큰이
        // 없었으면 빈 env로 둔다. `none` 위치에 env를 직역하면 틀린다.
        if f.pairs.contains_key("target_os") && !f.pairs.contains_key("target_env") {
            f.set("target_env", "");
        }
        f
    }

    /// `--exclude-tests` 전용 팩트 — test=false 외에는 아무것도 모른다
    /// (platform=false라 unix 같은 플래그 부재도 거짓으로 읽지 않는다).
    /// 이 팩트로 Some(false)가 나오는 조건만 "test 없이는 성립 불가"다:
    /// `test`·`all(test, unix)`는 걸리고 `not(test)`·`any(test, unix)`는
    /// 남는다 — 후자는 test 없이도 성립하므로 테스트 전용이 아니다.
    pub fn test_off() -> Facts {
        let mut f = Facts::default();
        f.flags.insert("test".to_string(), false);
        f
    }

    /// 단일값 팩트 — 같은 키의 나중 지정이 이전을 덮는다
    /// (gnux32의 폭 32가 x86_64의 64를 대체하는 식).
    fn set(&mut self, key: &str, val: &str) {
        self.pairs
            .insert(key.to_string(), BTreeSet::from([val.to_string()]));
    }

    fn add_family(&mut self, fam: &'static str) {
        self.pairs
            .entry("target_family".to_string())
            .or_default()
            .insert(fam.to_string());
    }
}

/// 아키텍처 토큰 → (target_arch, 포인터 폭, 엔디언). rustc 실측 기준:
/// i686은 x86, powerpc64le는 powerpc64, sparcv9은 sparc64, arm64_32는
/// aarch64+32비트, bpfel/bpfeb은 64비트다 — 트리플 토큰을 그대로 쓰면
/// 이 별칭들이 틀린 팩트가 된다. 모르면 None.
fn arch_facts(arch: &str) -> Option<(&'static str, &'static str, &'static str)> {
    Some(match arch {
        "i386" | "i486" | "i586" | "i686" => ("x86", "32", "little"),
        "x86_64" => ("x86_64", "64", "little"),
        "arm" | "armv4t" | "armv5te" | "armv6" | "armv7" | "armv7s" => ("arm", "32", "little"),
        "armeb" | "armebv7r" => ("arm", "32", "big"),
        "aarch64" => ("aarch64", "64", "little"),
        "aarch64_be" => ("aarch64", "64", "big"),
        "arm64_32" => ("aarch64", "32", "little"),
        "arm64ec" => ("arm64ec", "64", "little"),
        "riscv32" | "riscv32im" | "riscv32imc" | "riscv32imac" => ("riscv32", "32", "little"),
        "riscv64" | "riscv64imac" | "riscv64gc" => ("riscv64", "64", "little"),
        "mips" => ("mips", "32", "big"),
        "mipsel" => ("mips", "32", "little"),
        "mips32r6" => ("mips32r6", "32", "big"),
        "mips32r6el" => ("mips32r6", "32", "little"),
        "mips64" => ("mips64", "64", "big"),
        "mips64el" => ("mips64", "64", "little"),
        "mips64r6" => ("mips64r6", "64", "big"),
        "mips64r6el" => ("mips64r6", "64", "little"),
        "powerpc" => ("powerpc", "32", "big"),
        "powerpcle" => ("powerpc", "32", "little"),
        "powerpc64" => ("powerpc64", "64", "big"),
        "powerpc64le" => ("powerpc64", "64", "little"),
        "s390x" => ("s390x", "64", "big"),
        "sparc" => ("sparc", "32", "big"),
        "sparc64" | "sparcv9" => ("sparc64", "64", "big"),
        "wasm32" => ("wasm32", "32", "little"),
        "wasm64" => ("wasm64", "64", "little"),
        "loongarch64" => ("loongarch64", "64", "little"),
        "nvptx64" => ("nvptx64", "64", "little"),
        "m68k" => ("m68k", "32", "big"),
        "hexagon" => ("hexagon", "32", "little"),
        "xtensa" => ("xtensa", "32", "little"),
        "bpfel" => ("bpf", "64", "little"),
        "bpfeb" => ("bpf", "64", "big"),
        // thumbv6m/thumbv7em 등은 모두 arm 아키텍처다.
        a if a.starts_with("thumbv") => ("arm", "32", "little"),
        _ => return None,
    })
}

/// 트리플 토큰 → rustc target_vendor 값. 모르는 토큰은 None —
/// 벤더 칸에 OS가 오는 트리플(`foo-bar-linux`)도 있어 위치로 못 읽는다.
fn vendor_of(tok: &str) -> Option<&'static str> {
    Some(match tok {
        "apple" => "apple",
        "pc" => "pc",
        "unknown" => "unknown",
        "nvidia" => "nvidia",
        "sun" => "sun",
        "ibm" => "ibm",
        "amd" => "amd",
        "esp" => "espressif",
        "nintendo" => "nintendo",
        "fortanix" => "fortanix",
        "sony" => "sony",
        "uwp" => "uwp",
        "wrs" => "wrs",
        _ => return None,
    })
}

/// 트리플 토큰 → rustc target_os 값. `none`은 bare-metal os다.
fn os_of(tok: &str) -> Option<&'static str> {
    Some(match tok {
        "darwin" | "macos" => "macos",
        "ios" => "ios",
        "tvos" => "tvos",
        "watchos" => "watchos",
        "visionos" => "visionos",
        "linux" => "linux",
        "windows" | "win32" => "windows",
        "android" => "android",
        "emscripten" => "emscripten",
        "wasi" => "wasi",
        "freebsd" => "freebsd",
        "netbsd" => "netbsd",
        "openbsd" => "openbsd",
        "dragonfly" => "dragonfly",
        "solaris" => "solaris",
        "illumos" => "illumos",
        "haiku" => "haiku",
        "redox" => "redox",
        "fuchsia" => "fuchsia",
        "hurd" => "hurd",
        "vxworks" => "vxworks",
        "espidf" => "espidf",
        "horizon" => "horizon",
        "nto" => "nto",
        "aix" => "aix",
        "l4re" => "l4re",
        "uefi" => "uefi",
        "hermit" => "hermit",
        "xous" => "xous",
        "cygwin" => "cygwin",
        "ohos" => "ohos",
        "none" => "none",
        _ => return None,
    })
}

/// OS → target_family 집합. emscripten처럼 family가 여러 개인 타깃이
/// 있다 — rustc 실측에서 `unix`와 `wasm`이 함께 나온다.
fn families_of(os: &str) -> &'static [&'static str] {
    match os {
        "macos" | "ios" | "tvos" | "watchos" | "visionos" | "linux" | "android" | "freebsd"
        | "netbsd" | "openbsd" | "dragonfly" | "solaris" | "illumos" | "fuchsia" | "redox"
        | "haiku" | "vxworks" | "espidf" | "hurd" | "nto" | "aix" | "l4re" | "cygwin" | "ohos" => {
            &["unix"]
        }
        // emscripten은 unix+wasm, wasi 계열은 wasm 단독.
        "emscripten" => &["unix", "wasm"],
        "wasi" => &["wasm"],
        "windows" => &["windows"],
        // hermit·xous·uefi·none·horizon 같은 비-유닉스는 family가 없다 —
        // 집합이 비어 있는 것과 모르는 것은 다르니 빈 슬라이스를 돌린다.
        _ => &[],
    }
}

/// env 토큰 → (target_env, target_abi, 포인터 폭 32 강제). rustc 실측:
/// `gnueabihf`·`gnuabi64`는 env=gnu, `gnux32`는 env=gnu+abi=x32+ILP32,
/// `muslabi64`는 env=musl+abi=abi64, `eabi`/`eabihf`는 env=""+abi에만.
fn env_of(tok: &str) -> Option<(&'static str, Option<&'static str>, bool)> {
    Some(match tok {
        "gnu" => ("gnu", None, false),
        "gnueabi" => ("gnu", Some("eabi"), false),
        "gnueabihf" => ("gnu", Some("eabihf"), false),
        "gnuabi64" => ("gnu", Some("abi64"), false),
        "gnux32" | "gnu_ilp32" => ("gnu", Some("x32"), true),
        "gnuspe" => ("gnu", Some("spe"), false),
        "gnusoftfloat" => ("gnu", Some("softfloat"), false),
        "musl" => ("musl", None, false),
        "musleabi" => ("musl", Some("eabi"), false),
        "musleabihf" => ("musl", Some("eabihf"), false),
        "muslabi64" => ("musl", Some("abi64"), false),
        "uclibc" => ("uclibc", None, false),
        "uclibceabi" => ("uclibc", Some("eabi"), false),
        "uclibceabihf" => ("uclibc", Some("eabihf"), false),
        "msvc" => ("msvc", None, false),
        "sgx" => ("sgx", None, false),
        "newlib" => ("newlib", None, false),
        "sim" => ("sim", Some("sim"), false),
        "macabi" => ("macabi", Some("macabi"), false),
        // 순수 ABI 토큰은 env가 비어 있고 abi만 간다.
        "eabi" => ("", Some("eabi"), false),
        "eabihf" => ("", Some("eabihf"), false),
        "abi64" => ("", Some("abi64"), false),
        "ilp32" => ("", Some("ilp32"), false),
        _ => return None,
    })
}

/// cfg 조건 문자열을 팩트로 평가한다.
/// `Some(false)`는 "이 팩트에서 확실히 거짓" — 지워도 안전한 유일한 결과다.
/// `Some(true)`는 확실히 참. `None`은 판정 불가(feature·미지의 키·
/// 문법 오류) — keep. 문법이 깨진 조건도 None이다 — 파싱을 속이면
/// 진짜 조건을 거짓으로 오판할 수 있다.
pub fn eval(cond: &str, facts: &Facts) -> Option<bool> {
    let toks = lex(cond)?;
    let (v, rest) = pred(&toks, facts)?;
    // 파싱이 안 끝나면 조건을 다 못 읽은 것 — 폐쇄적으로 판정 불가.
    rest.is_empty().then_some(v).flatten()
}

/// 조건이 없는 선언은 무조건 성립이다 — `#[cfg]`가 없는 아이템은
/// 어떤 타깃에서도 존재한다.
pub fn eval_opt(cond: Option<&str>, facts: &Facts) -> Option<bool> {
    match cond {
        Some(c) => eval(c, facts),
        None => Some(true),
    }
}

/// 토큰 하나 — 식별자/문자열/구두점. cfg 문법만 허용한다.
#[derive(Debug, PartialEq)]
enum Tok {
    Ident(String),
    Str(String),
    Eq,
    LParen,
    RParen,
    Comma,
}

/// 토큰 문자열을 렉싱한다. cfg 조건 밖의 문자·끊긴 문자열·지원하지
/// 않는 이스케이프는 전부 렉스 오류 — None이면 eval도 None이다.
fn lex(src: &str) -> Option<Vec<Tok>> {
    let mut out = Vec::new();
    let mut chars = src.chars().peekable();
    while let Some(&c) = chars.peek() {
        match c {
            ' ' | '\t' | '\n' => {
                chars.next();
            }
            '=' => {
                chars.next();
                out.push(Tok::Eq);
            }
            '(' => {
                chars.next();
                out.push(Tok::LParen);
            }
            ')' => {
                chars.next();
                out.push(Tok::RParen);
            }
            ',' => {
                chars.next();
                out.push(Tok::Comma);
            }
            '"' => {
                chars.next();
                let mut s = String::new();
                let mut closed = false;
                while let Some(c) = chars.next() {
                    match c {
                        '"' => {
                            closed = true;
                            break;
                        }
                        // 이스케이프는 지원 범위만 디코드한다 — `\x6f` 같은
                        // 것을 미디코드한 채 비교하면 참 조건을 거짓으로
                        // 지운다. 모르는 이스케이프는 렉스 오류로 미지 처리.
                        // \xHH는 Rust 문자열 규칙대로 ASCII(≤0x7F)만 —
                        // 그 이상은 유효한 cfg가 아니라 문법 오류다.
                        '\\' => match chars.next()? {
                            '"' => s.push('"'),
                            '\\' => s.push('\\'),
                            'n' => s.push('\n'),
                            'r' => s.push('\r'),
                            't' => s.push('\t'),
                            '0' => s.push('\0'),
                            'x' => {
                                let h1 = chars.next()?;
                                let h2 = chars.next()?;
                                if !(h1.is_ascii_hexdigit() && h2.is_ascii_hexdigit()) {
                                    return None;
                                }
                                let v = u8::from_str_radix(&format!("{h1}{h2}"), 16).ok()?;
                                if v > 0x7F {
                                    return None;
                                }
                                s.push(v as char);
                            }
                            _ => return None,
                        },
                        _ => s.push(c),
                    }
                }
                if !closed {
                    return None;
                }
                out.push(Tok::Str(s));
            }
            // 식별자 시작은 문자·밑줄만 — `123` 같은 숫자 토큰은 cfg
            // 문법이 아니라 렉스 오류다(미지로 폐쇄).
            c if c.is_alphabetic() || c == '_' => {
                let mut s = String::new();
                while let Some(&c) = chars.peek() {
                    if !(c.is_alphanumeric() || c == '_') {
                        break;
                    }
                    s.push(c);
                    chars.next();
                }
                out.push(Tok::Ident(s));
            }
            _ => return None,
        }
    }
    Some(out)
}

/// 술어 하나를 파싱·평가한다 — (결과, 남은 토큰).
fn pred<'a>(toks: &'a [Tok], facts: &Facts) -> Option<(Option<bool>, &'a [Tok])> {
    let (Tok::Ident(name), rest) = toks.split_first()? else {
        return None;
    };
    // name = "value" 형태.
    if let Some((Tok::Eq, rest)) = rest.split_first() {
        let (Tok::Str(val), rest) = rest.split_first()? else {
            return None;
        };
        return Some((fact_eq(name, val, facts), rest));
    }
    if let Some((Tok::LParen, rest)) = rest.split_first() {
        // all(...)/any(...)/not(...) — 술어 사이에는 쉼표가 필수다.
        // `all(unix windows)`를 쉼표 없는 두 술어로 넘기면 임의 조건이
        // 참이 될 수 있다 — 문법 위반은 None으로 폐쇄한다.
        let mut vals = Vec::new();
        let mut rest = rest;
        loop {
            match rest.split_first() {
                Some((Tok::RParen, r)) => {
                    rest = r;
                    break;
                }
                _ => {
                    let (v, r) = pred(rest, facts)?;
                    vals.push(v);
                    rest = match r.split_first() {
                        Some((Tok::Comma, r)) => r,
                        Some((Tok::RParen, r)) => {
                            // 마지막 술어 — 닫는 괄호까지 소비하고 끝낸다.
                            rest = r;
                            break;
                        }
                        _ => return None,
                    };
                }
            }
        }
        let _ = rest;
        let v = match name.as_str() {
            // 삼값 논리: all은 거짓 하나면 거짓, any는 참 하나면 참.
            // 그 외에 미지가 섞이면 전체가 미지다.
            "all" => {
                if vals.contains(&Some(false)) {
                    Some(false)
                } else if vals.iter().all(Option::is_some) {
                    Some(true)
                } else {
                    None
                }
            }
            "any" => {
                if vals.contains(&Some(true)) {
                    Some(true)
                } else if vals.iter().all(Option::is_some) {
                    Some(false)
                } else {
                    None
                }
            }
            "not" if vals.len() == 1 => vals[0].map(|v| !v),
            _ => return None,
        };
        return Some((v, rest));
    }
    Some((bare_name(name, facts), rest))
}

/// `name = "value"` 술어 — 팩트에 없는 키는 기본적으로 None(미지).
/// 예외는 rustc 실측의 target_* 키뿐: rustc가 출력하는 키 우주는
/// 닫혀 있어 부재가 곧 "그 타깃에 그 키는 없다"는 뜻이다.
/// feature·panic 같은 VOLATILE 키는 있어도 미지다 — 프로필이 값을
/// 바꾸기 때문이다.
fn fact_eq(name: &str, val: &str, f: &Facts) -> Option<bool> {
    if VOLATILE.contains(&name) {
        return None;
    }
    if let Some(vals) = f.pairs.get(name) {
        return Some(vals.contains(val));
    }
    if f.rustc && name.starts_with("target_") {
        return Some(false);
    }
    None
}

/// `unix`·`windows`·`test` 같은 bare-name 술어.
/// 평가 순서: cfg 리터럴(true/false는 항상 확정) → VOLATILE(프로필이
/// 정하는 것은 미지) → 명시 플래그 팩트(test_off의 test=false 포함) →
/// unix/windows의 target_family 멤버십(트리플 추정이 여기 답한다) →
/// platform 팩트에서 잘 알려진 빌트인의 부재=거짓 → 그 외는 미지.
fn bare_name(name: &str, f: &Facts) -> Option<bool> {
    match name {
        "true" => return Some(true),
        "false" => return Some(false),
        _ => {}
    }
    if VOLATILE.contains(&name) {
        return None;
    }
    if let Some(&v) = f.flags.get(name) {
        return Some(v);
    }
    if matches!(name, "unix" | "windows") {
        if let Some(fams) = f.pairs.get("target_family") {
            return Some(fams.contains(name));
        }
    }
    if f.platform && KNOWN_FLAGS.contains(&name) {
        return Some(false);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn macos() -> Facts {
        Facts::from_triple("aarch64-apple-darwin")
    }

    fn windows() -> Facts {
        Facts::from_triple("x86_64-pc-windows-msvc")
    }

    /// rustc --print cfg --target aarch64-apple-darwin의 실제 출력 축약본.
    fn macos_rustc() -> Facts {
        Facts::from_cfg_lines(
            [
                "debug_assertions",
                "panic=\"unwind\"",
                "target_abi=\"\"",
                "target_arch=\"aarch64\"",
                "target_endian=\"little\"",
                "target_env=\"\"",
                "target_family=\"unix\"",
                "target_os=\"macos\"",
                "target_pointer_width=\"64\"",
                "target_vendor=\"apple\"",
                "unix",
            ]
            .iter()
            .copied(),
        )
    }

    #[test]
    fn triple_facts() {
        let f = macos();
        assert_eq!(f.pairs["target_arch"].iter().next().unwrap(), "aarch64");
        assert_eq!(f.pairs["target_os"].iter().next().unwrap(), "macos");
        assert!(f.pairs["target_family"].contains("unix"));
        assert!(f.pairs["target_env"].contains(""));
        let w = windows();
        assert!(w.pairs["target_os"].contains("windows"));
        assert!(w.pairs["target_family"].contains("windows"));
        assert!(w.pairs["target_env"].contains("msvc"));
    }

    #[test]
    fn triple_aliases_are_normalized() {
        // i686은 target_arch="x86"다 — 트리플 토큰을 그대로 쓰면 틀린다.
        let f = Facts::from_triple("i686-pc-windows-msvc");
        assert!(f.pairs["target_arch"].contains("x86"));
        assert!(f.pairs["target_pointer_width"].contains("32"));
        assert!(f.pairs["target_family"].contains("windows"));
        // armv7 + gnueabihf → arch arm, env gnu.
        let f = Facts::from_triple("armv7-unknown-linux-gnueabihf");
        assert!(f.pairs["target_arch"].contains("arm"));
        assert!(f.pairs["target_env"].contains("gnu"));
        // gnux32는 env gnu + ILP32 — 폭은 64가 아니라 32 하나다.
        let f = Facts::from_triple("x86_64-unknown-linux-gnux32");
        assert!(f.pairs["target_env"].contains("gnu"));
        assert_eq!(
            f.pairs["target_pointer_width"],
            BTreeSet::from(["32".to_string()])
        );
        assert!(f.pairs["target_abi"].contains("x32"));
        assert_eq!(eval("not(target_pointer_width = \"64\")", &f), Some(true));
        // emscripten은 unix와 wasm 두 family를 갖는다.
        let f = Facts::from_triple("wasm32-unknown-emscripten");
        assert!(f.pairs["target_family"].contains("unix"));
        assert!(f.pairs["target_family"].contains("wasm"));
        // windows 타깃의 unix는 확정 거짓.
        assert_eq!(eval("unix", &windows()), Some(false));
    }

    /// rustc --print cfg 실측과 대조해 검증한 트리플 매핑들.
    #[test]
    fn triple_mappings_match_rustc() {
        // powerpc64le는 arch가 powerpc64다(le는 엔디언).
        let f = Facts::from_triple("powerpc64le-unknown-linux-gnu");
        assert!(f.pairs["target_arch"].contains("powerpc64"));
        assert_eq!(
            f.pairs["target_pointer_width"],
            BTreeSet::from(["64".to_string()])
        );
        // sparcv9은 sparc64다.
        let f = Facts::from_triple("sparcv9-sun-solaris");
        assert!(f.pairs["target_arch"].contains("sparc64"));
        assert!(f.pairs["target_vendor"].contains("sun"));
        // arm64_32는 aarch64 + ILP32.
        let f = Facts::from_triple("arm64_32-apple-watchos");
        assert!(f.pairs["target_arch"].contains("aarch64"));
        assert!(f.pairs["target_pointer_width"].contains("32"));
        // bpf는 양쪽 엔디언 모두 64비트다.
        let f = Facts::from_triple("bpfel-unknown-none");
        assert!(f.pairs["target_pointer_width"].contains("64"));
        assert!(f.pairs["target_os"].contains("none"));
        // eabihf는 env가 아니라 ABI 표시 — os는 none, env는 ""다.
        let f = Facts::from_triple("thumbv7em-none-eabihf");
        assert!(f.pairs["target_os"].contains("none"));
        assert_eq!(f.pairs["target_env"], BTreeSet::from(["".to_string()]));
        assert!(f.pairs["target_abi"].contains("eabihf"));
        // wasip1은 os=wasi + env=p1으로 쪼개진다.
        let f = Facts::from_triple("wasm32-wasip1");
        assert!(f.pairs["target_os"].contains("wasi"));
        assert!(f.pairs["target_env"].contains("p1"));
        // muslabi64는 env=musl + abi=abi64.
        let f = Facts::from_triple("mips64-unknown-linux-muslabi64");
        assert!(f.pairs["target_env"].contains("musl"));
        // androideabi는 os=android + env="".
        let f = Facts::from_triple("armv7-linux-androideabi");
        assert!(f.pairs["target_os"].contains("android"));
        assert_eq!(f.pairs["target_env"], BTreeSet::from(["".to_string()]));
        // apple abi 계열 — sim/macabi는 env이자 abi다.
        let f = Facts::from_triple("aarch64-apple-ios-sim");
        assert!(f.pairs["target_env"].contains("sim"));
        assert!(f.pairs["target_os"].contains("ios"));
    }

    #[test]
    fn rustc_lines_are_authoritative() {
        let f = macos_rustc();
        assert!(f.rustc && f.platform);
        assert_eq!(eval("unix", &f), Some(true));
        assert_eq!(eval("windows", &f), Some(false));
        assert_eq!(eval("target_vendor = \"apple\"", &f), Some(true));
        // rustc 출력이 닫힌 target_* 키 우주 — 없는 키는 거짓이다.
        assert_eq!(eval("target_has_atomic = \"128\"", &f), Some(false));
        // cargo build --target에는 test가 없다 — 부재는 거짓이다.
        assert_eq!(eval("test", &f), Some(false));
        // 프로필·Cargo가 정하는 조건은 출력에 있어도 미지다 —
        // debug_assertions는 기본값이 나와도 릴리스에서 다르고,
        // feature·커스텀 --cfg는 rustc 출력이 아예 모른다.
        assert_eq!(eval("debug_assertions", &f), None);
        assert_eq!(eval("panic = \"abort\"", &f), None);
        assert_eq!(eval("feature = \"serde\"", &f), None);
        assert_eq!(eval("my_custom_flag", &f), None);
        // cfg 리터럴은 항상 확정된다.
        assert_eq!(eval("true", &f), Some(true));
        assert_eq!(eval("false", &f), Some(false));
        assert_eq!(eval("all(true , unix)", &f), Some(true));
        assert_eq!(eval("not(true)", &f), Some(false));
    }

    #[test]
    fn test_off_marks_test_only_conditions() {
        let f = Facts::test_off();
        // test가 필요한 조건은 거짓으로 확정된다.
        assert_eq!(eval("test", &f), Some(false));
        assert_eq!(eval("all(test , unix)", &f), Some(false));
        // test 없이도 성립하는 조건은 미지로 남는다 — 지우면 안 된다.
        assert_eq!(eval("not(test)", &f), Some(true));
        assert_eq!(eval("any(test , unix)", &f), None);
        assert_eq!(eval("feature = \"test-utils\"", &f), None);
        assert_eq!(eval("unix", &f), None);
    }

    #[test]
    fn eval_simple_conditions() {
        assert_eq!(eval("unix", &macos()), Some(true));
        assert_eq!(eval("windows", &macos()), Some(false));
        assert_eq!(eval("target_os = \"linux\"", &macos()), Some(false));
        assert_eq!(eval("target_os = \"macos\"", &macos()), Some(true));
        assert_eq!(eval("target_pointer_width = \"64\"", &macos()), Some(true));
        assert_eq!(eval("target_endian = \"big\"", &macos()), Some(false));
    }

    #[test]
    fn eval_nested_all_any_not() {
        let f = macos();
        assert_eq!(eval("all(unix , target_os = \"macos\")", &f), Some(true));
        assert_eq!(eval("all(unix , windows)", &f), Some(false));
        assert_eq!(eval("any(windows , unix)", &f), Some(true));
        assert_eq!(eval("not(unix)", &f), Some(false));
        assert_eq!(eval("not(windows)", &f), Some(true));
    }

    #[test]
    fn unknown_conditions_stay_unknown() {
        let f = macos();
        // feature·debug_assertions는 프로필이 정한다 — 미지다.
        assert_eq!(eval("feature = \"x\"", &f), None);
        assert_eq!(eval("debug_assertions", &f), None);
        // test는 빌드에 정의되지 않는 빌트인 — platform 팩트에서
        // 부재는 거짓이다(cargo build --target은 test를 켜지 않는다).
        assert_eq!(eval("test", &f), Some(false));
        // 모르는 키도 마찬가지.
        assert_eq!(eval("target_feature = \"sse4.2\"", &f), None);
        // all 안에 미지 섞임 — 참 항목만으론 전체를 못 정한다.
        assert_eq!(eval("all(unix , feature = \"x\")", &f), None);
        assert_eq!(eval("any(feature = \"x\" , unix)", &f), Some(true));
    }

    #[test]
    fn garbage_is_unknown() {
        assert_eq!(eval("=", &macos()), None);
        assert_eq!(eval("all(unix", &macos()), None);
        assert_eq!(eval("unix extra", &macos()), None);
        // 쉼표 없는 두 술어는 문법 위반 — 미지로 폐쇄한다.
        assert_eq!(eval("all(unix windows)", &macos()), None);
        // 끊긴 문자열·미지원 이스케이프·조건 밖 문자도 미지다.
        assert_eq!(eval("target_os = \"macos", &macos()), None);
        assert_eq!(eval("target_os = \"mac\\yos\"", &macos()), None);
        assert_eq!(eval("unix;windows", &macos()), None);
        // 지원하는 이스케이프는 디코드된다 — "mac\tos"는 macos가 아니다.
        assert_eq!(eval("target_os = \"mac\\tos\"", &macos()), Some(false));
        // \x 이스케이프는 디코드한다 — "mac\x6fs"는 "macos"다.
        assert_eq!(eval("target_os = \"mac\\x6fs\"", &macos()), Some(true));
        // \x80 이상은 Rust 문자열 규칙 위반 — 디코드하지 않고 미지다.
        assert_eq!(eval("target_os = \"mac\\x80s\"", &macos()), None);
        // 숫자 시작 토큰은 cfg 식별자가 아니다.
        assert_eq!(eval("123", &macos()), None);
        assert_eq!(eval("all(123 , unix)", &macos()), None);
    }
}
