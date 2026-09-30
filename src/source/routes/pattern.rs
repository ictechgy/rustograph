//! actix-router `ResourceDef` 패턴을 정규 템플릿 세그먼트로 바꾼다.
//!
//! 근거(actix-router 0.5.4 `src/resource.rs`): `{name}`은 `[^/]+`(907),
//! `{name:regex}`는 이름 붙은 그룹에 그대로 들어가 여러 세그먼트와 맞을 수 있고
//! (956, 문서 140-142), 끝의 `{name}*`는 `.*`(908, 935)다. 한 세그먼트에 정적 글자와
//! 파라미터를 섞을 수 있고 파라미터 여러 개도 된다(문서 78-80, 1214). 정규식은
//! 실행하지 않고 흔한 모양(`X+`·`X{n,m}`과 그 연결)만 읽는다 — 읽지 못한 정규식이
//! `/`와 맞을 수 있는지 증명하지 못하면 템플릿을 포기한다(dynamic).

use super::template::{Constraint, Seg};

/// 해석 결과 — 세그먼트와, 끝 catch-all이 빈 나머지도 받는지.
pub(super) struct Parsed {
    pub segs: Vec<Seg>,
    pub empty_tail: bool,
}

/// 패턴 조각 — 리터럴 글자열 또는 파라미터.
enum Piece {
    Lit(String),
    Param { regex: Option<String>, tail: bool },
}

/// actix 패턴 전체(앞 `/` 포함)를 해석한다. Err는 템플릿으로 쓸 수 없는 사유다.
pub(super) fn parse_actix(raw: &str) -> Result<Parsed, String> {
    let pieces = tokenize(raw)?;
    // 조각을 세그먼트로 모은다. 리터럴 안의 `/`가 세그먼트 경계다.
    let mut segs: Vec<Vec<Piece>> = vec![Vec::new()];
    for piece in pieces {
        match piece {
            Piece::Lit(text) => {
                let mut parts = text.split('/');
                if let Some(first) = parts.next() {
                    push_lit(segs.last_mut().expect("at least one segment"), first);
                }
                for part in parts {
                    segs.push(Vec::new());
                    push_lit(segs.last_mut().expect("just pushed"), part);
                }
            }
            p => segs.last_mut().expect("at least one segment").push(p),
        }
    }
    // 앞 `/`가 만든 빈 첫 세그먼트를 뗀다.
    if raw.starts_with('/') {
        segs.remove(0);
    } else {
        return Err("does not start with `/`".to_string());
    }
    let n = segs.len();
    let mut out = Vec::with_capacity(n);
    let mut empty_tail = false;
    for (i, pieces) in segs.into_iter().enumerate() {
        let (seg, empty) = segment(pieces, i == n - 1)?;
        empty_tail |= empty;
        out.push(seg);
    }
    Ok(Parsed {
        segs: out,
        empty_tail,
    })
}

/// 빈 리터럴은 넣지 않는다.
fn push_lit(seg: &mut Vec<Piece>, text: &str) {
    if !text.is_empty() {
        seg.push(Piece::Lit(text.to_string()));
    }
}

/// 세그먼트 하나를 템플릿 세그먼트로 — (세그먼트, 빈 끝 변형 필요).
fn segment(pieces: Vec<Piece>, last: bool) -> Result<(Seg, bool), String> {
    let params = pieces
        .iter()
        .filter(|p| matches!(p, Piece::Param { .. }))
        .count();
    if params == 0 {
        let text: String = pieces
            .into_iter()
            .map(|p| match p {
                Piece::Lit(t) => t,
                Piece::Param { .. } => String::new(),
            })
            .collect();
        return Ok((Seg::Lit(text), false));
    }
    if params > 1 {
        return Err("has more than one parameter in a segment".to_string());
    }
    let mut prefix = String::new();
    let mut suffix = String::new();
    let mut param = None;
    for p in pieces {
        match p {
            Piece::Lit(t) if param.is_none() => prefix.push_str(&t),
            Piece::Lit(t) => suffix.push_str(&t),
            Piece::Param { regex, tail } => param = Some((regex, tail)),
        }
    }
    let (regex, tail) = param.expect("one parameter counted");
    let class = match (tail, regex.as_deref()) {
        (true, _) => Class::Multi { empty: true },
        (false, None) => Class::Segment(None),
        (false, Some(re)) => classify(re),
    };
    let whole = prefix.is_empty() && suffix.is_empty();
    match class {
        Class::Segment(constraint) => Ok((
            Seg::Param {
                prefix,
                suffix,
                constraint,
            },
            false,
        )),
        Class::Multi { empty } if whole && last => Ok((Seg::CatchAll, empty)),
        Class::Multi { .. } => Err(
            "has a parameter that can span segments but is not a whole last segment".to_string(),
        ),
        Class::Unknown => Err("has a parameter regex the analyzer cannot bound".to_string()),
    }
}

/// 패턴을 리터럴·파라미터 조각으로 나눈다. 파라미터 안 정규식의 중첩 중괄호
/// (`\d{3}`)를 센다(resource.rs:910-922). 끝의 `}*`는 꼬리 파라미터다(935).
fn tokenize(raw: &str) -> Result<Vec<Piece>, String> {
    let mut out = Vec::new();
    let mut lit = String::new();
    let chars: Vec<char> = raw.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '{' {
            lit.push(chars[i]);
            i += 1;
            continue;
        }
        let mut depth = 1;
        let mut j = i + 1;
        while j < chars.len() && depth > 0 {
            match chars[j] {
                '{' => depth += 1,
                '}' => depth -= 1,
                _ => {}
            }
            j += 1;
        }
        if depth != 0 {
            return Err("has an unclosed `{`".to_string());
        }
        let inner: String = chars[i + 1..j - 1].iter().collect();
        let (name, regex) = match inner.split_once(':') {
            Some((n, r)) => (n.to_string(), Some(r.to_string())),
            None => (inner.clone(), None),
        };
        if name.is_empty() {
            return Err("has an unnamed parameter".to_string());
        }
        let tail = j == chars.len() - 1 && chars[j] == '*';
        if tail && regex.is_some() {
            return Err("has a tail parameter with a custom regex (actix panics)".to_string());
        }
        if !lit.is_empty() {
            out.push(Piece::Lit(std::mem::take(&mut lit)));
        }
        out.push(Piece::Param { regex, tail });
        i = if tail { j + 1 } else { j };
    }
    if !lit.is_empty() {
        out.push(Piece::Lit(lit));
    }
    Ok(out)
}

/// 정규식 분류.
#[derive(Debug, PartialEq, Eq)]
enum Class {
    /// 한 세그먼트 안에서만 맞는다(빈 값 없음) — 제약(없으면 제약 없음).
    Segment(Option<Constraint>),
    /// `/`까지 맞을 수 있다 — `empty`면 빈 값도.
    Multi { empty: bool },
    /// 읽지 못함.
    Unknown,
}

/// 128비트 ASCII 문자 집합.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Set(u128);

impl Set {
    const EMPTY: Set = Set(0);
    const ALL: Set = Set(u128::MAX);
    fn range(a: u8, b: u8) -> Set {
        (a..=b).fold(Set::EMPTY, |s, c| s.with(c))
    }
    fn with(self, c: u8) -> Set {
        if c < 128 {
            Set(self.0 | (1u128 << c))
        } else {
            self
        }
    }
    fn has(self, c: u8) -> bool {
        c < 128 && self.0 & (1u128 << c) != 0
    }
    fn union(self, o: Set) -> Set {
        Set(self.0 | o.0)
    }
    fn not(self) -> Set {
        Set(!self.0)
    }
    fn subset(self, o: Set) -> bool {
        self.0 & !o.0 == 0
    }
    fn digits() -> Set {
        Set::range(b'0', b'9')
    }
    fn word() -> Set {
        Set::range(b'a', b'z')
            .union(Set::range(b'A', b'Z'))
            .union(Set::digits())
            .with(b'_')
    }
    fn space() -> Set {
        [b' ', b'\t', b'\n', b'\r', 0x0b, 0x0c]
            .iter()
            .fold(Set::EMPTY, |s, &c| s.with(c))
    }
    /// `/`를 뺀 전체 — 기본 `[^/]+`와 같은 집합.
    fn not_slash(self) -> Set {
        Set(self.0 & !(1u128 << b'/'))
    }
    fn hex() -> Set {
        Set::digits()
            .union(Set::range(b'a', b'f'))
            .union(Set::range(b'A', b'F'))
    }
}

/// 연결 원소 하나 — 문자 집합과 반복 범위(최대 None = 무한).
struct Atom {
    set: Set,
    min: u32,
    max: Option<u32>,
}

/// 정규식을 분류한다. 읽는 문법: 문자 클래스(`[..]`·`[^..]`), `\d \w \s \D \W \S`,
/// `.`, 이스케이프·일반 리터럴과 수량자 `+ * ? {n} {n,} {n,m}`의 연결. 그룹·대안·
/// 앵커·플래그가 있으면 Unknown이다.
fn classify(re: &str) -> Class {
    let Some(atoms) = atoms(re) else {
        return Class::Unknown;
    };
    if atoms.is_empty() {
        return Class::Unknown;
    }
    let slash = atoms.iter().any(|a| a.set.has(b'/'));
    let min: u32 = atoms.iter().map(|a| a.min).sum();
    if slash {
        // `.*`·`.+`처럼 원소 하나가 전부일 때만 끝 catch-all로 읽는다.
        return match atoms.as_slice() {
            [a] if a.max.is_none() && a.set == Set::ALL && a.min <= 1 => {
                Class::Multi { empty: a.min == 0 }
            }
            _ => Class::Unknown,
        };
    }
    if min == 0 {
        // 빈 값과 맞는 파라미터는 `{}`(비어 있지 않은 세그먼트)로 쓸 수 없다.
        return Class::Unknown;
    }
    let all: Set = atoms.iter().fold(Set::EMPTY, |s, a| s.union(a.set));
    let slug = Set::word().with(b'-');
    let constraint = if is_uuid(&atoms) {
        Some(Constraint::Uuid)
    } else if all.subset(Set::digits()) {
        Some(Constraint::Int)
    } else if all.subset(slug) {
        Some(Constraint::Slug)
    } else if atoms.len() == 1 && all == Set::ALL.not_slash() {
        None
    } else {
        Some(Constraint::Regex(re.to_string()))
    };
    Class::Segment(constraint)
}

/// 하이픈 있는 8-4-4-4-12 hex 모양인가.
fn is_uuid(atoms: &[Atom]) -> bool {
    let want = [8, 0, 4, 0, 4, 0, 4, 0, 12];
    atoms.len() == want.len()
        && atoms.iter().zip(want).all(|(a, n)| {
            if n == 0 {
                a.set == Set::EMPTY.with(b'-') && a.min == 1 && a.max == Some(1)
            } else {
                a.set.subset(Set::hex()) && a.min == n && a.max == Some(n)
            }
        })
}

/// 정규식을 원소 연결로 읽는다. 읽지 못하면 None.
fn atoms(re: &str) -> Option<Vec<Atom>> {
    let b = re.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let (set, next) = match b[i] {
            b'[' => class(b, i)?,
            b'\\' => (escape(*b.get(i + 1)?)?, i + 2),
            b'.' => (Set::ALL, i + 1),
            b'(' | b')' | b'|' | b'^' | b'$' | b'*' | b'+' | b'?' | b'{' | b'}' => return None,
            c if c.is_ascii() => (Set::EMPTY.with(c), i + 1),
            _ => return None,
        };
        i = next;
        let (min, max, after) = quantifier(b, i)?;
        i = after;
        out.push(Atom { set, min, max });
    }
    Some(out)
}

/// 이스케이프 하나의 집합.
fn escape(c: u8) -> Option<Set> {
    Some(match c {
        b'd' => Set::digits(),
        b'w' => Set::word(),
        b's' => Set::space(),
        b'D' => Set::digits().not(),
        b'W' => Set::word().not(),
        b'S' => Set::space().not(),
        c if c.is_ascii_punctuation() => Set::EMPTY.with(c),
        _ => return None,
    })
}

/// `[..]` 클래스 — (집합, 다음 위치).
fn class(b: &[u8], start: usize) -> Option<(Set, usize)> {
    let mut i = start + 1;
    let negate = b.get(i) == Some(&b'^');
    if negate {
        i += 1;
    }
    let mut set = Set::EMPTY;
    let mut first = true;
    loop {
        let c = *b.get(i)?;
        if c == b']' && !first {
            i += 1;
            break;
        }
        first = false;
        let (lo, next) = match c {
            b'\\' => {
                let e = *b.get(i + 1)?;
                if e.is_ascii_alphabetic() {
                    set = set.union(escape(e)?);
                    i += 2;
                    continue;
                }
                (e, i + 2)
            }
            b'[' => return None,
            c if c.is_ascii() => (c, i + 1),
            _ => return None,
        };
        i = next;
        if b.get(i) == Some(&b'-') && b.get(i + 1).is_some_and(|&c| c != b']') {
            let hi = *b.get(i + 1)?;
            if hi == b'\\' || !hi.is_ascii() || hi < lo {
                return None;
            }
            set = set.union(Set::range(lo, hi));
            i += 2;
        } else {
            set = set.with(lo);
        }
    }
    Some((if negate { set.not() } else { set }, i))
}

/// 수량자 — (최소, 최대, 다음 위치). 게으른 `?` 접미사는 같은 범위다.
fn quantifier(b: &[u8], i: usize) -> Option<(u32, Option<u32>, usize)> {
    let (min, max, mut next) = match b.get(i) {
        Some(b'+') => (1, None, i + 1),
        Some(b'*') => (0, None, i + 1),
        Some(b'?') => (0, Some(1), i + 1),
        Some(b'{') => {
            let close = i + b[i..].iter().position(|&c| c == b'}')?;
            let body = std::str::from_utf8(&b[i + 1..close]).ok()?;
            let (lo, hi) = match body.split_once(',') {
                None => {
                    let n = body.parse().ok()?;
                    (n, Some(n))
                }
                Some((a, "")) => (a.parse().ok()?, None),
                Some((a, c)) => (a.parse().ok()?, Some(c.parse().ok()?)),
            };
            (lo, hi, close + 1)
        }
        _ => (1, Some(1), i),
    };
    if next > i && b.get(next) == Some(&b'?') {
        next += 1;
    }
    Some((min, max, next))
}

#[cfg(test)]
mod tests {
    use super::super::template::render;
    use super::*;

    fn t(raw: &str) -> String {
        match parse_actix(raw) {
            Ok(p) => format!(
                "{}{}",
                render(&p.segs),
                if p.empty_tail { " +empty" } else { "" }
            ),
            Err(e) => format!("ERR {e}"),
        }
    }

    #[test]
    fn actix_patterns() {
        assert_eq!(t("/items/{id}"), "/items/{}");
        assert_eq!(t("/files/{name}.json"), "/files/{}.json");
        assert_eq!(t("/files/{tail}*"), "/files/{**} +empty");
        assert_eq!(t("/files/{tail:.*}"), "/files/{**} +empty");
        assert_eq!(t("/files/{tail:.+}"), "/files/{**}");
        assert!(t("/v{a}.{b}").starts_with("ERR"));
        assert!(t("/user{tail}*").starts_with("ERR"));
        assert!(t("/a/{x:(foo|bar)}").starts_with("ERR"));
        assert_eq!(t("/"), "/");
        assert_eq!(t("/a/"), "/a/");
    }

    #[test]
    fn regex_constraints() {
        assert_eq!(classify(r"\d+"), Class::Segment(Some(Constraint::Int)));
        assert_eq!(
            classify(r"[0-9]{1,6}"),
            Class::Segment(Some(Constraint::Int))
        );
        assert_eq!(
            classify(r"[a-z0-9-]+"),
            Class::Segment(Some(Constraint::Slug))
        );
        assert_eq!(classify(r"\w+"), Class::Segment(Some(Constraint::Slug)));
        assert_eq!(classify(r"[^/]+"), Class::Segment(None));
        assert_eq!(
            classify(r"[^/.]+"),
            Class::Segment(Some(Constraint::Regex(r"[^/.]+".into())))
        );
        assert_eq!(
            classify(r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}"),
            Class::Segment(Some(Constraint::Uuid))
        );
        assert_eq!(classify(r"\d*"), Class::Unknown);
        assert_eq!(classify(r"[a-z/]+"), Class::Unknown);
        assert_eq!(classify(r"\S+"), Class::Unknown);
    }
}
