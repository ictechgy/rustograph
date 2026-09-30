//! isthmus 정규 경로 템플릿 — 문법 검사·URI 경로 정규화·세그먼트 조립.
//!
//! 정본은 isthmus docs/GRAPH-EXCHANGE.md "정규 경로 템플릿"과 공유 벡터
//! `http-template`(`template.grammar`·`template.normalize`)이다. isthmus는 문법만
//! 검증하고 다시 정규화하지 않으므로, 생산자가 소비자와 같은 판정을 내야 문서가
//! 거부되지 않는다. 이 모듈은 syn을 모른다 — 프레임워크 경로 문법은 각 추출기가
//! 여기의 [`Seg`]로 바꿔 넘긴다.

/// 템플릿 최대 길이(UTF-16 코드 단위) — 소비자의 `too-long` 기준이다.
pub const MAX_TEMPLATE_LENGTH: usize = 2048;

/// 정규 템플릿이면 None, 아니면 소비자와 같은 거부 사유 코드를 돌려준다.
pub fn template_problem(template: &str) -> Option<&'static str> {
    if template.encode_utf16().count() > MAX_TEMPLATE_LENGTH {
        return Some("too-long");
    }
    let Some(rest) = template.strip_prefix('/') else {
        return Some("not-rooted");
    };
    let raw: Vec<&str> = rest.split('/').collect();
    for (i, seg) in raw.iter().enumerate() {
        if *seg == "{**}" {
            if i != raw.len() - 1 {
                return Some("catch-all-not-last");
            }
            continue;
        }
        if let Some(problem) = segment_problem(seg) {
            return Some(problem);
        }
    }
    None
}

/// 세그먼트 하나의 거부 사유 — isthmus `parseSegment`와 같은 순서로 판정한다.
fn segment_problem(seg: &str) -> Option<&'static str> {
    let bytes = seg.as_bytes();
    let mut param = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'{' => {
                if seg[i..].starts_with("{**}") {
                    return Some("catch-all-partial");
                }
                if bytes.get(i + 1) != Some(&b'}') {
                    return Some("stray-brace");
                }
                if param {
                    return Some("multiple-parameters");
                }
                param = true;
                i += 2;
            }
            b'}' => return Some("stray-brace"),
            b'%' => {
                if let Some(problem) = percent_problem(bytes, i) {
                    return Some(problem);
                }
                i += 3;
            }
            c if is_pchar_literal(c) => i += 1,
            _ => return Some("invalid-character"),
        }
    }
    None
}

/// `%XX` 하나의 거부 사유 — 대문자 hex여야 하고 unreserved를 인코딩하면 안 된다.
fn percent_problem(bytes: &[u8], at: usize) -> Option<&'static str> {
    let (Some(&h), Some(&l)) = (bytes.get(at + 1), bytes.get(at + 2)) else {
        return Some("malformed-percent");
    };
    if !h.is_ascii_hexdigit() || !l.is_ascii_hexdigit() {
        return Some("malformed-percent");
    }
    if h.is_ascii_lowercase() || l.is_ascii_lowercase() {
        return Some("lowercase-percent-hex");
    }
    if is_unreserved(hex_value(h) * 16 + hex_value(l)) {
        return Some("encoded-unreserved");
    }
    None
}

/// RFC 3986 unreserved 문자다 — 인코딩하지 않는다.
fn is_unreserved(c: u8) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, b'-' | b'.' | b'_' | b'~')
}

/// `%`를 뺀 pchar 리터럴 문자다(unreserved·sub-delims·`:`·`@`).
fn is_pchar_literal(c: u8) -> bool {
    is_unreserved(c)
        || matches!(
            c,
            b'!' | b'$'
                | b'&'
                | b'\''
                | b'('
                | b')'
                | b'*'
                | b'+'
                | b','
                | b';'
                | b'='
                | b':'
                | b'@'
        )
}

/// hex 숫자 하나의 값 — 호출자가 `is_ascii_hexdigit`를 먼저 확인한다.
fn hex_value(c: u8) -> u8 {
    match c {
        b'0'..=b'9' => c - b'0',
        b'a'..=b'f' => c - b'a' + 10,
        _ => c - b'A' + 10,
    }
}

/// URI 경로 문자열을 정규 템플릿 리터럴로 정규화한다(`template.normalize`).
///
/// `%XX`는 대문자 hex로 쓰고 unreserved면 디코드한다. pchar도 `/`도 아닌 문자
/// (공백·중괄호·ASCII 밖 문자 등)는 UTF-8 바이트마다 `%XX`로 인코딩한다. 중복
/// 슬래시·끝 슬래시·인코딩된 `/`(`%2F`)는 보존한다. 형식이 깨진 `%`는 글자
/// 그대로의 `%`로 보고 `%25`로 쓴다.
pub fn normalize_uri_path(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = String::with_capacity(path.len());
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'%' {
            let pair = (bytes.get(i + 1), bytes.get(i + 2));
            if let (Some(&h), Some(&l)) = pair {
                if h.is_ascii_hexdigit() && l.is_ascii_hexdigit() {
                    let value = hex_value(h) * 16 + hex_value(l);
                    push_byte(&mut out, value, true);
                    i += 3;
                    continue;
                }
            }
            out.push_str("%25");
            i += 1;
            continue;
        }
        push_byte(&mut out, c, false);
        i += 1;
    }
    out
}

/// 바이트 하나를 정규형으로 쓴다. `encoded`면 원문이 `%XX`였다는 뜻이라
/// unreserved만 디코드하고 나머지(`%2F` 포함)는 인코딩을 유지한다.
fn push_byte(out: &mut String, c: u8, encoded: bool) {
    let literal = if encoded {
        is_unreserved(c)
    } else {
        c == b'/' || is_pchar_literal(c)
    };
    if literal {
        out.push(c as char);
    } else {
        out.push_str(&format!("%{c:02X}"));
    }
}

/// 프레임워크 경로를 해석한 세그먼트 하나다. 리터럴 조각은 프레임워크가 요청
/// 경로와 비교하는 원문 그대로 담고, 렌더링할 때 [`normalize_uri_path`]를 거친다.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seg {
    /// 파라미터 없는 세그먼트.
    Lit(String),
    /// 파라미터 하나를 품은 세그먼트 — 앞뒤 리터럴이 비면 세그먼트 전체 `{}`.
    Param {
        prefix: String,
        suffix: String,
        constraint: Option<Constraint>,
    },
    /// 마지막 세그먼트 전체의 끝 catch-all(세그먼트 1개 이상).
    CatchAll,
}

/// `paramConstraints` 항목의 종류다. 닫힌 종류(int·uuid·slug)만 소비자가 평가한다.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Constraint {
    Int,
    Uuid,
    Slug,
    /// 평가하지 않는 정규식 — 원문은 정보용 `pattern`이다.
    Regex(String),
}

impl Constraint {
    /// 계약의 kind 문자열이다.
    pub fn kind(&self) -> &'static str {
        match self {
            Constraint::Int => "int",
            Constraint::Uuid => "uuid",
            Constraint::Slug => "slug",
            Constraint::Regex(_) => "regex",
        }
    }
}

/// 세그먼트 목록을 정규 템플릿 문자열로 쓴다. 세그먼트가 없으면 루트 `/`다.
pub fn render(segs: &[Seg]) -> String {
    if segs.is_empty() {
        return "/".to_string();
    }
    let mut out = String::new();
    for seg in segs {
        out.push('/');
        match seg {
            Seg::Lit(text) => out.push_str(&encode_segment_text(text)),
            Seg::Param { prefix, suffix, .. } => {
                out.push_str(&encode_segment_text(prefix));
                out.push_str("{}");
                out.push_str(&encode_segment_text(suffix));
            }
            Seg::CatchAll => out.push_str("{**}"),
        }
    }
    out
}

/// 세그먼트 안 리터럴 조각을 정규화한다. `/`는 세그먼트 경계라 이 자리에 올 수
/// 없으므로 `%2F`로 쓴다(프레임워크가 한 세그먼트로 비교한 문자다).
fn encode_segment_text(text: &str) -> String {
    normalize_uri_path(text).replace('/', "%2F")
}

/// `/a/b/` 같은 원문 경로를 리터럴 세그먼트로 나눈다(앞 `/` 하나는 뗀다).
/// 빈 경로와 `/`는 빈 세그먼트 하나(루트)다.
pub fn literal_segments(path: &str) -> Vec<Seg> {
    let body = path.strip_prefix('/').unwrap_or(path);
    body.split('/').map(|s| Seg::Lit(s.to_string())).collect()
}

/// 세그먼트 목록의 `paramConstraints`(세그먼트 인덱스, 제약)다.
pub fn constraints(segs: &[Seg]) -> Vec<(usize, Constraint)> {
    segs.iter()
        .enumerate()
        .filter_map(|(i, s)| match s {
            Seg::Param {
                constraint: Some(c),
                ..
            } => Some((i, c.clone())),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_normalizes_literals_and_params() {
        let segs = vec![
            Seg::Lit("caf\u{e9}".into()),
            Seg::Param {
                prefix: "v".into(),
                suffix: String::new(),
                constraint: None,
            },
            Seg::Lit("{x}".into()),
            Seg::CatchAll,
        ];
        assert_eq!(render(&segs), "/caf%C3%A9/v{}/%7Bx%7D/{**}");
        assert_eq!(render(&[]), "/");
        assert_eq!(render(&literal_segments("/a//b/")), "/a//b/");
        assert_eq!(template_problem(&render(&segs)), None);
    }

    #[test]
    fn malformed_percent_is_a_literal_percent() {
        assert_eq!(normalize_uri_path("/a%2"), "/a%252");
        assert_eq!(normalize_uri_path("/%zz"), "/%25zz");
    }
}
