//! `#[cfg(...)]` 조건 평가기 — `--target`·`--exclude-tests` 필터의 순수 부분.
//!
//! 조건은 `cfg_of`가 저장한 토큰 문자열(`target_os = "windows"`,
//! `all(unix , feature = "x")`). 평가는 삼값 논리다 — 모르는 조건은
//! keep으로 처리하고, 거짓으로 판명된 것만 문서에서 지운다.
//!
//! 팩트는 두 경로로 온다: `rustc --print cfg --target`의 권위 있는
//! 출력(complete=true — bare 플래그까지 확정)과, rustc를 못 쓸 때의
//! 트리플 문자열 추정(complete=false — 확실한 키만 채운다).
//! 트리플 구성 요소를 cfg 값으로 직역하면 안 된다 — `i686`의
//! target_arch는 `"x86"`이고 `gnueabihf`의 env는 `"gnu"`이며
//! emscripten은 `unix`와 `wasm` 두 family를 갖는다.

use std::collections::{BTreeMap, BTreeSet};

/// cfg 평가의 사실 집합. `pairs`는 `name = "value"` 술어의 값 집합
/// (target_family·target_feature처럼 같은 키가 여러 번 나올 수 있다),
/// `flags`는 `unix`·`windows`·`debug_assertions` 같은 bare 조건이다.
/// 모르는 키는 맵에 없음으로 표현한다 — 거짓이 아니라 미지다.
#[derive(Debug, Default)]
pub struct Facts {
    pub pairs: BTreeMap<String, BTreeSet<String>>,
    pub flags: BTreeSet<String>,
    /// rustc 출력이면 true — flags/pairs가 완전 목록이라 부재=거짓.
    /// 트리플 추정이면 false — 부재는 미지다.
    pub complete: bool,
    /// `cfg(test)` 전용 팩트 — 타깃은 이를 모른다. without_tests만
    /// Some(false)로 세팅해 "test 없이는 성립 불가" 조건을 거른다.
    pub test: Option<bool>,
}

impl Facts {
    /// `rustc --print cfg --target` 출력 한 줄씩을 팩트로 적재한다.
    /// `name = "value"`와 bare 조건 두 형태다. complete=true —
    /// rustc가 주지 않은 조건은 그 타깃에서 거짓이라고 봐도 된다.
    pub fn from_cfg_lines<'a>(lines: impl Iterator<Item = &'a str>) -> Facts {
        let mut f = Facts {
            complete: true,
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
                f.flags.insert(line.to_string());
            }
        }
        f
    }

    /// rustc를 못 쓸 때의 트리플 추정 — 부분 팩트만 채운다.
    /// 알려진 매핑만 번역하고 모르는 것은 비운다 — 잘못된 팩트보다
    /// 미지가 낫다(미지는 keep이고 잘못은 오삭제다).
    pub fn from_triple(triple: &str) -> Facts {
        let mut f = Facts::default();
        let parts: Vec<&str> = triple.split('-').collect();
        if parts.len() < 2 {
            return f;
        }
        if let Some(arch) = norm_arch(parts[0]) {
            f.set("target_arch", arch);
            let (w, e) = arch_facts(parts[0]);
            if let Some(w) = w {
                f.set("target_pointer_width", w);
            }
            if let Some(e) = e {
                f.set("target_endian", e);
            }
        }
        if parts[1] != "unknown" {
            f.set("target_vendor", parts[1]);
        }
        if let Some(os) = parts.get(2).filter(|o| **o != "unknown") {
            f.set("target_os", norm_os(os));
            for fam in families_of(norm_os(os)) {
                f.add_family(fam);
            }
        }
        // 4번째 부분은 env — gnueabihf는 gnu, gnux32는 gnu+32비트다.
        if let Some(env) = parts.get(3).filter(|e| **e != "unknown") {
            let (env, width32) = norm_env(env);
            f.set("target_env", env);
            if width32 {
                f.set("target_pointer_width", "32");
            }
        } else {
            f.set("target_env", "");
        }
        f
    }

    /// `--exclude-tests` 전용 팩트 — test=false 외에는 아무것도 모른다.
    /// 이 팩트로 Some(false)가 나오는 조건만 "test 없이는 성립 불가"다:
    /// `test`·`all(test, unix)`는 걸리고 `not(test)`·`any(test, unix)`는
    /// 남는다 — 후자는 test 없이도 성립하므로 테스트 전용이 아니다.
    pub fn test_off() -> Facts {
        Facts {
            test: Some(false),
            ..Default::default()
        }
    }

    fn set(&mut self, key: &str, val: &str) {
        self.pairs
            .entry(key.to_string())
            .or_default()
            .insert(val.to_string());
    }

    fn add_family(&mut self, fam: &'static str) {
        self.pairs
            .entry("target_family".to_string())
            .or_default()
            .insert(fam.to_string());
    }
}

/// 트리플 아키텍처 → rustc target_arch 값. 트리플 토큰을 그대로 쓰면
/// i686·armv7·thumbv* 같은 별칭이 틀린 팩트가 된다 — 모르면 None.
fn norm_arch(arch: &str) -> Option<&'static str> {
    Some(match arch {
        "i386" | "i486" | "i586" | "i686" => "x86",
        "x86_64" => "x86_64",
        "arm" | "armv4t" | "armv5te" | "armv6" | "armv7" | "armv7s" | "armeb" | "armebv7r" => "arm",
        "aarch64" | "aarch64_be" => "aarch64",
        "riscv32" | "riscv32im" | "riscv32imc" | "riscv32imac" => "riscv32",
        "riscv64" | "riscv64imac" | "riscv64gc" => "riscv64",
        "mips" | "mipsel" => "mips",
        "mips32r6" | "mips32r6el" => "mips32r6",
        "mips64" | "mips64el" => "mips64",
        "mips64r6" | "mips64r6el" => "mips64r6",
        "arm64_32" => "arm64_32",
        "arm64ec" => "arm64ec",
        "powerpc" => "powerpc",
        "powerpc64" => "powerpc64",
        "powerpc64le" => "powerpc64le",
        "s390x" => "s390x",
        "sparc" => "sparc",
        "sparc64" => "sparc64",
        "sparcv9" => "sparcv9",
        "wasm32" => "wasm32",
        "wasm64" => "wasm64",
        "loongarch64" => "loongarch64",
        "nvptx64" => "nvptx64",
        "m68k" => "m68k",
        "hexagon" => "hexagon",
        "xtensa" => "xtensa",
        "bpf" | "bpfeb" | "bpfel" => "bpf",
        // thumbv6m/thumbv7em 등은 모두 arm 아키텍처다.
        a if a.starts_with("thumbv") => "arm",
        _ => return None,
    })
}

/// 아키텍처 토큰 → (포인터 폭, 엔디언). 모르는 것은 None.
fn arch_facts(arch: &str) -> (Option<&'static str>, Option<&'static str>) {
    let (w, e) = match arch {
        "x86_64" | "aarch64" | "riscv64" | "riscv64imac" | "riscv64gc" | "loongarch64"
        | "wasm64" | "powerpc64le" | "mips64el" | "mips64r6el" | "nvptx64" | "arm64_32"
        | "arm64ec" => ("64", "little"),
        "s390x" | "powerpc64" | "sparc64" | "sparcv9" | "mips64" | "mips64r6" | "aarch64_be" => {
            ("64", "big")
        }
        "i386" | "i486" | "i586" | "i686" | "riscv32" | "riscv32im" | "riscv32imc"
        | "riscv32imac" | "wasm32" | "mipsel" | "mips32r6" | "mips32r6el" | "hexagon"
        | "xtensa" | "bpfel" => ("32", "little"),
        "arm" | "armv4t" | "armv5te" | "armv6" | "armv7" | "armv7s" => ("32", "little"),
        "powerpc" | "sparc" | "mips" | "m68k" | "armeb" | "armebv7r" | "bpfeb" => ("32", "big"),
        a if a.starts_with("thumbv") => ("32", "little"),
        _ => return (None, None),
    };
    (Some(w), Some(e))
}

/// 트리플 OS 토큰 → rustc target_os 값.
fn norm_os(os: &str) -> &str {
    match os {
        "darwin" => "macos",
        other => other,
    }
}

/// OS → target_family 집합. emscripten처럼 family가 여러 개인 타깃이
/// 있다 — rustc 실측에서 `unix`와 `wasm`이 함께 나온다.
fn families_of(os: &str) -> &'static [&'static str] {
    match os {
        "macos" | "ios" | "tvos" | "watchos" | "visionos" | "linux" | "android" | "freebsd"
        | "netbsd" | "openbsd" | "dragonfly" | "solaris" | "illumos" | "fuchsia" | "redox"
        | "haiku" | "vxworks" | "espidf" | "hurd" | "nto" | "aix" | "l4re" => &["unix"],
        // emscripten은 unix+wasm, wasi 계열은 wasm 단독.
        "emscripten" => &["unix", "wasm"],
        "wasi" | "p1" | "p2" => &["wasm"],
        "windows" => &["windows"],
        // hermit·xous·uefi·none 같은 비-유닉스는 family가 없다 —
        // 집합이 비어 있는 것과 모르는 것은 다르니 빈 슬라이스를 돌린다.
        _ => &[],
    }
}

/// env 토큰 → (rustc target_env, 포인터 폭 32 강제).
/// `gnueabihf`는 env가 gnu이고 `gnux32`는 env가 gnu면서 ILP32 ABI다.
fn norm_env(env: &str) -> (&str, bool) {
    match env {
        "gnux32" => ("gnu", true),
        "gnu_ilp32" => ("gnu", true),
        "gnueabi" | "gnueabihf" | "gnuabi64" | "gnuspe" => ("gnu", false),
        "musleabi" | "musleabihf" => ("musl", false),
        "uclibceabi" | "uclibceabihf" => ("uclibc", false),
        other => (other, false),
    }
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
                                let v = u8::from_str_radix(&format!("{h1}{h2}"), 16).ok()?;
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
            c if c.is_alphanumeric() || c == '_' => {
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
                } else {
                    Some(true).filter(|_| vals.iter().all(|v| v.is_some()))
                }
            }
            "any" => {
                if vals.contains(&Some(true)) {
                    Some(true)
                } else {
                    Some(false).filter(|_| vals.iter().all(|v| v.is_some()))
                }
            }
            "not" if vals.len() == 1 => vals[0].map(|v| !v),
            _ => return None,
        };
        return Some((v, rest));
    }
    Some((bare_name(name, facts), rest))
}

/// `name = "value"` 술어 — 팩트에 없는 키는 None(미지).
/// complete 팩트에서는 부재=거짓이 아니라 그래도 None이다 — 키 자체를
/// rustc가 모르는 이름이면 비교할 수 없기 때문이다.
fn fact_eq(name: &str, val: &str, f: &Facts) -> Option<bool> {
    f.pairs.get(name).map(|vals| vals.contains(val))
}

/// `unix`·`windows`·`test` 같은 bare-name 술어.
/// unix/windows는 target_family 멤버십으로 평가한다 — family가 여러 개
/// 될 수 있어 집합이다. test는 전용 팩트로만. 그 외(debug_assertions·
/// doc·miri·proc_macro 등)는 complete 팩트(rustc 실측)에서만 확정된다.
fn bare_name(name: &str, f: &Facts) -> Option<bool> {
    match name {
        "test" => f.test,
        "unix" | "windows" => f.pairs.get("target_family").map(|s| s.contains(name)),
        _ if f.complete => Some(f.flags.contains(name)),
        _ => None,
    }
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
        // gnux32는 env gnu + ILP32.
        let f = Facts::from_triple("x86_64-unknown-linux-gnux32");
        assert!(f.pairs["target_env"].contains("gnu"));
        assert!(f.pairs["target_pointer_width"].contains("32"));
        // emscripten은 unix와 wasm 두 family를 갖는다.
        let f = Facts::from_triple("wasm32-unknown-emscripten");
        assert!(f.pairs["target_family"].contains("unix"));
        assert!(f.pairs["target_family"].contains("wasm"));
        // windows 타깃의 unix는 확정 거짓.
        assert_eq!(eval("unix", &windows()), Some(false));
    }

    #[test]
    fn rustc_lines_are_authoritative() {
        let f = macos_rustc();
        assert!(f.complete);
        assert_eq!(eval("unix", &f), Some(true));
        assert_eq!(eval("windows", &f), Some(false));
        assert_eq!(eval("target_vendor = \"apple\"", &f), Some(true));
        // complete 팩트는 bare 플래그도 확정한다 — debug_assertions가
        // 출력에 있으니 참, 없는 miri는 거짓으로 잡힌다.
        assert_eq!(eval("debug_assertions", &f), Some(true));
        assert_eq!(eval("miri", &f), Some(false));
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
        // feature·test·debug_assertions는 트리플 추정이 모른다.
        assert_eq!(eval("feature = \"x\"", &f), None);
        assert_eq!(eval("test", &f), None);
        assert_eq!(eval("debug_assertions", &f), None);
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
    }
}
