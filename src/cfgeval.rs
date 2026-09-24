//! `#[cfg(...)]` 조건 평가기 — `--target` 필터의 순수 부분.
//!
//! 조건은 `cfg_of`가 저장한 토큰 문자열(`target_os = "windows"`,
//! `all(unix , feature = "x")`). 평가는 삼값 논리다 — 모르는 조건은
//! keep으로 처리하고, 거짓으로 판명된 것만 문서에서 지운다.
//! feature·test·debug_assertions처럼 타깃 트리플로는 알 수 없는 키는
//! 전부 `None`으로 돌아간다.

/// 타깃 트리플에서 유도 가능한 cfg 팩트. 모르는 필드는 None —
/// 모르는 조건을 거짓으로 처리하면 실제로는 참인 코드를 지우게 된다.
#[derive(Debug, Default)]
pub struct Facts {
    pub target_arch: Option<String>,
    pub target_os: Option<String>,
    pub target_family: Option<String>,
    pub target_env: Option<String>,
    pub target_endian: Option<String>,
    pub target_pointer_width: Option<String>,
    pub target_vendor: Option<String>,
}

impl Facts {
    /// `aarch64-apple-darwin` 같은 트리플을 팩트로 펼친다.
    /// 트리플이 알려진 형태가 아니어도 부분 팩트만 채운 채 돌아간다 —
    /// 평가 불가 조건은 keep되니 부분 팩트도 안전하다.
    pub fn from_triple(triple: &str) -> Facts {
        let parts: Vec<&str> = triple.split('-').collect();
        let mut f = Facts::default();
        if parts.len() < 2 {
            return f;
        }
        f.target_arch = Some(parts[0].to_string());
        let (width, endian) = arch_facts(parts[0]);
        f.target_pointer_width = width.map(|s| s.to_string());
        f.target_endian = endian.map(|s| s.to_string());
        f.target_vendor = (parts[1] != "unknown").then(|| parts[1].to_string());
        // 3번째 부분은 OS. darwin은 rustc가 "macos"로 보고한다.
        if let Some(os) = parts.get(2) {
            if *os != "unknown" {
                let os = match *os {
                    "darwin" => "macos".to_string(),
                    other => other.to_string(),
                };
                f.target_family = family_of(&os).map(|s| s.to_string());
                f.target_os = Some(os);
            }
        }
        // 4번째 부분은 env(gnu|msvc|musl|...). 없으면 rustc 기본값 "".
        f.target_env = Some(match parts.get(3) {
            Some(env) if *env != "unknown" => env.to_string(),
            _ => String::new(),
        });
        f
    }
}

/// 아키텍처 이름 → (포인터 폭, 엔디언). 모르는 아키텍처는 (None, None) —
/// 이름 팩트만으로 cfg(target_arch) 평가는 계속 가능하다.
fn arch_facts(arch: &str) -> (Option<&'static str>, Option<&'static str>) {
    let (w, e) = match arch {
        "x86_64" | "aarch64" | "riscv64" | "loongarch64" | "wasm64" | "powerpc64le"
        | "mips64el" | "mipsel64" => ("64", "little"),
        "s390x" | "powerpc64" | "sparc64" | "mips64" => ("64", "big"),
        "i586" | "i686" | "arm" | "riscv32" | "wasm32" | "mipsel" | "x86" => ("32", "little"),
        "powerpc" | "sparc" | "mips" | "m68k" | "armeb" => ("32", "big"),
        a if a.starts_with("armv") || a.starts_with("thumbv") => ("32", "little"),
        a if a.starts_with("armebv") => ("32", "big"),
        _ => return (None, None),
    };
    (Some(w), Some(e))
}

/// OS 이름 → target_family. unix/windows 외에는 family가 없다(rustc 규칙).
fn family_of(os: &str) -> Option<&'static str> {
    match os {
        "macos" | "ios" | "tvos" | "watchos" | "visionos" | "linux" | "android" | "freebsd"
        | "netbsd" | "openbsd" | "dragonfly" | "solaris" | "illumos" | "fuchsia" | "redox"
        | "haiku" | "emscripten" | "wasi" | "vxworks" | "espidf" | "hermit" | "hurd" | "nto"
        | "aix" | "l4re" | "xous" => Some("unix"),
        "windows" => Some("windows"),
        _ => None,
    }
}

/// cfg 조건 문자열을 팩트로 평가한다.
/// `Some(false)`는 "이 타깃에서 확실히 거짓" — 지워도 안전한 유일한 결과다.
/// `Some(true)`는 확실히 참. `None`은 판정 불가(feature·test·미지의 키) — keep.
pub fn eval(cond: &str, facts: &Facts) -> Option<bool> {
    let toks = lex(cond);
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

/// 토큰 문자열을 렉싱한다. cfg 조건 밖의 토큰은 렉스 오류 — None.
fn lex(src: &str) -> Vec<Tok> {
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
                while let Some(&c) = chars.peek() {
                    chars.next();
                    if c == '"' {
                        break;
                    }
                    s.push(c);
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
            _ => {
                chars.next();
            }
        }
    }
    out
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
        // all(...)/any(...)/not(...) — 리스트는 뒤에 쉼표가 올 수 있다.
        let mut vals = Vec::new();
        let mut rest = rest;
        loop {
            match rest.split_first() {
                Some((Tok::RParen, r)) => {
                    rest = r;
                    break;
                }
                Some((Tok::Comma, r)) => rest = r,
                _ => {
                    let (v, r) = pred(rest, facts)?;
                    vals.push(v);
                    rest = r;
                }
            }
        }
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

/// `name = "value"` 술어 — 팩트가 없거나 모르는 키면 None.
fn fact_eq(name: &str, val: &str, f: &Facts) -> Option<bool> {
    let fact = match name {
        "target_arch" => &f.target_arch,
        "target_os" => &f.target_os,
        "target_family" => &f.target_family,
        "target_env" => &f.target_env,
        "target_endian" => &f.target_endian,
        "target_pointer_width" => &f.target_pointer_width,
        "target_vendor" => &f.target_vendor,
        _ => return None,
    };
    fact.as_ref().map(|s| s == val)
}

/// `unix`·`windows` 같은 bare-name 술어 — family로 평가한다.
/// test·debug_assertions·doc·miri·feature 등은 타깃이 모른다.
fn bare_name(name: &str, f: &Facts) -> Option<bool> {
    match name {
        "unix" => f.target_family.as_ref().map(|s| s == "unix"),
        "windows" => f.target_family.as_ref().map(|s| s == "windows"),
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

    #[test]
    fn triple_facts() {
        let f = macos();
        assert_eq!(f.target_arch.as_deref(), Some("aarch64"));
        assert_eq!(f.target_os.as_deref(), Some("macos"));
        assert_eq!(f.target_family.as_deref(), Some("unix"));
        assert_eq!(f.target_env.as_deref(), Some(""));
        let w = windows();
        assert_eq!(w.target_os.as_deref(), Some("windows"));
        assert_eq!(w.target_family.as_deref(), Some("windows"));
        assert_eq!(w.target_env.as_deref(), Some("msvc"));
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
        // feature·test·debug_assertions는 타깃이 모른다.
        assert_eq!(eval("feature = \"x\"", &f), None);
        assert_eq!(eval("test", &f), None);
        assert_eq!(eval("debug_assertions", &f), None);
        // 모르는 키도 마찬가지.
        assert_eq!(eval("target_feature = \"sse4.2\"", &f), None);
        // all 안에 미지 섞임 — 참 항목만으론 전체를 못 정한다.
        assert_eq!(eval("all(unix , feature = \"x\")", &f), None);
        // 미지라도 거짓이 있는 any/all은 결과를 정할 수 있다/없다:
        // all(unix, unknown) → unknown, but any(false, unknown) → unknown.
        assert_eq!(eval("any(feature = \"x\" , unix)", &f), Some(true));
    }

    #[test]
    fn garbage_is_unknown() {
        assert_eq!(eval("=", &macos()), None);
        assert_eq!(eval("all(unix", &macos()), None);
        assert_eq!(eval("unix extra", &macos()), None);
    }
}
