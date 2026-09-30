//! 클라이언트 URL 조립 — isthmus `url-compose` 규칙(HTTP-WRAPPERS "공통 해석 규칙")의
//! Rust 구현.
//!
//! syn을 모르는 순수 계층이다. 추출기(`client`)가 문자열 식을 [`Piece`] 목록으로
//! 바꿔 넘기면, 이 모듈이 라이브러리 의미대로 base와 경로를 결합하고 정규 템플릿·
//! `pathAnchor`·`authority`·`queryTailStripped`·`maskedSegments`·`channelPrefix`를
//! 확정한다. 공유 벡터(`conformance/url-compose.json`)가 같은 함수로 실행된다.
//!
//! Rust 클라이언트의 결합 방식은 세 갈래다(근거와 오라클 기록은 docs/HTTP-ROUTES.md).
//!
//! - [`Join::WhatwgConcat`]: 문자열을 이어 붙인 뒤 `url::Url::parse`(WHATWG URL
//!   Standard)로 해석한다 — reqwest의 `IntoUrl for &str/String`, ureq 2.x. 점
//!   세그먼트를 지우고 `//`는 보존한다. base 리터럴이면 실제 결과, 미상 base 뒤
//!   `/x`는 base, 상대 경로는 dynamic + `ambiguous-base-join:`(dio 행과 같은 결과).
//! - [`Join::WhatwgJoin`]: `url::Url::join` — WHATWG 상대 해석. http(s)에서는
//!   RFC 3986 병합과 같다(`/x`는 root, `x`는 base 마지막 세그먼트를 바꾼다).
//! - [`Join::HttpUriConcat`]: 문자열을 이어 붙인 뒤 `http::Uri`로 해석한다 —
//!   ureq 3.x. 점 세그먼트를 지우지 않는다.

use super::template::{normalize_uri_path, render, template_problem, Seg};

/// 값 조각의 출처 — `http-wrapper-undeclared:` 판정과 `baseRef`에 쓴다.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// 출처를 모르는 값.
    Unknown,
    /// 감싸는 함수의 매개변수(이름) — 선언되지 않은 래퍼 싱크의 표지다.
    Param(String),
    /// 구조체 필드나 상수·static — 그 생산자 id를 `baseRef`로 싣는다.
    Base(String),
}

/// 문자열 식의 조각 하나다.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Piece {
    /// 값을 아는 리터럴 조각.
    Lit(String),
    /// 정적으로 모르는 값(보간·매개변수 등).
    Value(Origin),
    /// 값이 모두 `?`로 시작하거나 비어 있음을 증명한 지역 변수(`compose.suffix`).
    QueryTail,
}

/// 템플릿이 서버 루트부터 확정됐는지(`root`) 모르는 base 뒤인지(`base`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PathAnchor {
    Root,
    Base,
}

impl PathAnchor {
    /// 계약 문자열.
    pub fn as_str(self) -> &'static str {
        match self {
            PathAnchor::Root => "root",
            PathAnchor::Base => "base",
        }
    }
}

/// 확정한 정적 템플릿.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Template {
    pub template: String,
    pub anchor: PathAnchor,
    pub authority: Option<String>,
    pub query_tail_stripped: bool,
    pub masked_segments: usize,
}

/// 조립 결과다.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// 정적 템플릿.
    Template(Template),
    /// 템플릿으로 확정하지 못한 호출. `prefix`는 증명한 리터럴 접두사 템플릿(마스킹
    /// 적용), `ambiguous`는 미상 base 뒤 상대 경로라 `ambiguous-base-join:`으로 센다.
    Dynamic {
        prefix: Option<String>,
        anchor: PathAnchor,
        ambiguous: bool,
        masked_segments: usize,
    },
    /// 요청이 될 수 없는 URL — base 없는 상대 URL(`url::Url::parse`가 거부한다).
    Unrequestable,
}

/// 이 모듈이 아는 base 결합 방식이다.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Join {
    /// 문자열 연결 후 `url::Url::parse`(reqwest, ureq 2.x).
    WhatwgConcat,
    /// `url::Url::join`(WHATWG 상대 해석 = http(s)에서 RFC 3986).
    WhatwgJoin,
    /// 문자열 연결 후 `http::Uri`(ureq 3.x) — 점 세그먼트를 지우지 않는다.
    HttpUriConcat,
    /// base 끝 `/`와 경로 앞 `/`를 하나로 합친다. Rust 라이브러리 중 이 방식을 쓰는
    /// 것은 모델링하지 않았고, 공유 벡터의 `slash-join` 사례를 위해 둔다.
    SlashJoin,
}

/// 조립 중인 URL 값 — `url::Url`이나 URL 문자열을 해석한 상태다.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UrlVal {
    /// 소문자 scheme(`http`·`https`) — 모르면 None.
    pub scheme: Option<String>,
    /// 리터럴 authority(`host[:port]`) — 모르거나 동적이면 None.
    pub authority: Option<String>,
    pub path: UrlPath,
    /// query·fragment를 떼어 냈는가.
    pub query: bool,
    /// dot 세그먼트를 지우는 해석기인가(`http::Uri`는 지우지 않는다).
    pub dots: bool,
}

/// URL의 경로 상태다.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UrlPath {
    /// `/`로 시작하는 경로 조각들과 그 앵커.
    Known {
        anchor: PathAnchor,
        pieces: Vec<Piece>,
    },
    /// 경로를 전혀 모르는 base(`Url::parse(미상)`) — 상대 해석만 뒤에 잇는다.
    Opaque,
    /// 템플릿을 포기한 경로.
    Dynamic {
        prefix: Option<(PathAnchor, Vec<Piece>)>,
        ambiguous: bool,
    },
    /// base 없는 상대 URL — 요청이 되지 않는다.
    Unrequestable,
}

impl UrlVal {
    /// 경로·authority를 모두 모르는 URL(매개변수로 받은 `Url` 등).
    pub fn opaque(dots: bool) -> UrlVal {
        UrlVal {
            scheme: None,
            authority: None,
            path: UrlPath::Opaque,
            query: false,
            dots,
        }
    }

    /// 조각을 이어 붙인 문자열을 절대 URL로 해석한다(`Url::parse`·`http::Uri`).
    ///
    /// 앞 리터럴이 `http(s)://authority`를 모두 담으면 경로는 root다. authority
    /// 자리에 값이 끼면 그 값이 경로를 담을 수 있으므로 뒤의 첫 `/`부터 base다.
    /// 값으로 시작하면 그 값이 base이고, 뒤 리터럴이 `/`로 시작하지 않으면 base에
    /// 달라붙어 모호하다. 리터럴이 scheme 없이 시작하면 요청이 될 수 없다.
    pub fn parse(pieces: &[Piece], dots: bool) -> UrlVal {
        let pieces = preprocess(pieces);
        let mut out = UrlVal {
            scheme: None,
            authority: None,
            path: UrlPath::Dynamic {
                prefix: None,
                ambiguous: false,
            },
            query: false,
            dots,
        };
        match pieces.first() {
            None => out.path = UrlPath::Unrequestable,
            Some(Piece::Lit(first)) => match split_scheme(first) {
                Some((scheme, rest)) if scheme == "http" || scheme == "https" => {
                    out.scheme = Some(scheme);
                    let rest = rest.trim_start_matches('/').to_string();
                    let mut tail = vec![Piece::Lit(rest)];
                    tail.extend(pieces[1..].iter().cloned());
                    out.parse_authority(&tail);
                }
                // mailto:·file: 등 http가 아닌 scheme — reqwest·ureq가 보내기 전에 거부한다.
                Some(_) => out.path = UrlPath::Unrequestable,
                None => out.path = UrlPath::Unrequestable,
            },
            Some(_) => {
                let rest = &pieces[1..];
                out.path = match rest.first() {
                    Some(Piece::Lit(l)) if l.starts_with('/') => UrlPath::Known {
                        anchor: PathAnchor::Base,
                        pieces: rest.to_vec(),
                    },
                    Some(Piece::Lit(l)) if !l.starts_with(['?', '#']) => UrlPath::Dynamic {
                        prefix: None,
                        ambiguous: true,
                    },
                    _ => UrlPath::Dynamic {
                        prefix: None,
                        ambiguous: false,
                    },
                };
            }
        }
        out.strip_query();
        out
    }

    /// `scheme://` 뒤 조각에서 authority와 경로를 가른다.
    fn parse_authority(&mut self, tail: &[Piece]) {
        let Some(Piece::Lit(head)) = tail.first() else {
            return;
        };
        if let Some(end) = head.find(['/', '?', '#']) {
            self.authority = authority_of(&head[..end]);
            let mut pieces = vec![Piece::Lit(head[end..].to_string())];
            pieces.extend(tail[1..].iter().cloned());
            if let Some(Piece::Lit(l)) = pieces.first_mut() {
                if l.starts_with(['?', '#']) {
                    l.insert(0, '/');
                }
            }
            self.path = UrlPath::Known {
                anchor: PathAnchor::Root,
                pieces,
            };
            return;
        }
        if tail.len() == 1 {
            self.authority = authority_of(head);
            self.path = UrlPath::Known {
                anchor: PathAnchor::Root,
                pieces: vec![Piece::Lit("/".to_string())],
            };
            return;
        }
        // authority 안에 값이 있다 — 계약: host가 동적이면 base다.
        for (i, p) in tail.iter().enumerate().skip(1) {
            let Piece::Lit(l) = p else { continue };
            if let Some(end) = l.find(['/', '?', '#']) {
                if l[end..].starts_with('/') {
                    let mut pieces = vec![Piece::Lit(l[end..].to_string())];
                    pieces.extend(tail[i + 1..].iter().cloned());
                    self.path = UrlPath::Known {
                        anchor: PathAnchor::Base,
                        pieces,
                    };
                }
                return;
            }
        }
    }

    /// 경로 조각의 첫 `?`·`#`부터 끝까지를 떼고 `query`를 표시한다 — base 경로의
    /// query가 상대 병합의 마지막 `/` 판정에 섞이지 않게 한다.
    fn strip_query(&mut self) {
        if let UrlPath::Known { pieces, .. } = &mut self.path {
            if cut_query(pieces) {
                self.query = true;
            }
        }
    }

    /// `url::Url::join`(WHATWG 상대 해석)으로 경로를 잇는다.
    pub fn join(&self, input: &[Piece]) -> UrlVal {
        let input = preprocess(input);
        // 해석에 실패한 base(`Url::parse`가 Err)는 `?`로 빠져 요청이 되지 않는다.
        if self.path == UrlPath::Unrequestable {
            return self.clone();
        }
        let mut out = self.clone();
        out.query = false;
        let unknown_base = matches!(
            self.path,
            UrlPath::Opaque
                | UrlPath::Known {
                    anchor: PathAnchor::Base,
                    ..
                }
        );
        let first = match input.first() {
            // 빈 참조는 base 경로 그대로다 — base를 모르면 경로를 주장할 수 없다.
            None if unknown_base => return out.into_dynamic(true),
            None => return out,
            Some(Piece::Lit(l)) => l.clone(),
            Some(_) => return out.into_dynamic(false),
        };
        if let Some((scheme, rest)) = split_scheme(&first) {
            let same = self.scheme.as_deref() == Some(scheme.as_str());
            if same && !rest.starts_with('/') && (scheme == "http" || scheme == "https") {
                // WHATWG: 같은 special scheme의 `http:x`는 상대 참조다.
                let mut rel = vec![Piece::Lit(rest.to_string())];
                rel.extend(input[1..].iter().cloned());
                return self.join(&rel);
            }
            return UrlVal::parse(&input, self.dots);
        }
        if first.starts_with("//") {
            return match &self.scheme {
                Some(s) => {
                    let mut abs = vec![Piece::Lit(format!("{s}:{first}"))];
                    abs.extend(input[1..].iter().cloned());
                    UrlVal::parse(&abs, self.dots)
                }
                None => out.into_dynamic(false),
            };
        }
        if first.starts_with('/') {
            out.path = UrlPath::Known {
                anchor: PathAnchor::Root,
                pieces: input,
            };
        } else if first.starts_with(['?', '#']) || first.is_empty() {
            out.query = true;
            if matches!(self.path, UrlPath::Opaque) {
                return out.into_dynamic(true);
            }
        } else if unknown_base && has_dot_dot(&input) {
            // HTTP-WRAPPERS `rfc3986`, base 미상: `..`는 지울 세그먼트를 알 수 없다.
            return out.into_dynamic(true);
        } else {
            out.path = match &self.path {
                UrlPath::Known { anchor, pieces } => UrlPath::Known {
                    anchor: *anchor,
                    pieces: merge(pieces, &input),
                },
                UrlPath::Opaque => {
                    let mut pieces = vec![Piece::Lit("/".to_string())];
                    pieces.extend(input);
                    UrlPath::Known {
                        anchor: PathAnchor::Base,
                        pieces,
                    }
                }
                other => other.clone(),
            };
        }
        out.strip_query();
        out
    }

    /// 경로를 dynamic으로 바꾼다.
    fn into_dynamic(mut self, ambiguous: bool) -> UrlVal {
        self.path = UrlPath::Dynamic {
            prefix: None,
            ambiguous,
        };
        self
    }

    /// 이 URL을 다시 문자열 조각으로 쓴다(`as_str()`·`format!("{url}")`).
    pub fn to_pieces(&self) -> Vec<Piece> {
        let mut out = Vec::new();
        match (&self.path, &self.scheme, &self.authority) {
            (
                UrlPath::Known {
                    anchor: PathAnchor::Root,
                    pieces,
                },
                Some(s),
                Some(a),
            ) => {
                out.push(Piece::Lit(format!("{s}://{a}")));
                out.extend(pieces.iter().cloned());
            }
            (UrlPath::Known { pieces, .. }, _, _) => {
                out.push(Piece::Value(Origin::Unknown));
                out.extend(pieces.iter().cloned());
            }
            _ => out.push(Piece::Value(Origin::Unknown)),
        }
        if self.query {
            out.push(Piece::QueryTail);
        }
        out
    }

    /// 계약 사실로 확정한다.
    pub fn outcome(&self) -> Outcome {
        match &self.path {
            UrlPath::Known { anchor, pieces } => {
                let authority = match anchor {
                    PathAnchor::Root => self.authority.clone(),
                    PathAnchor::Base => None,
                };
                let mut out = compose_path(*anchor, pieces, authority.as_deref(), self.dots);
                if let Outcome::Template(t) = &mut out {
                    t.query_tail_stripped |= self.query;
                }
                out
            }
            UrlPath::Opaque => Outcome::Dynamic {
                prefix: None,
                anchor: PathAnchor::Base,
                ambiguous: false,
                masked_segments: 0,
            },
            UrlPath::Dynamic { prefix, ambiguous } => {
                let (prefix, anchor, masked) = match prefix {
                    Some((anchor, pieces)) => {
                        let (p, m) = prefix_of(pieces, self.authority.as_deref());
                        (p, *anchor, m)
                    }
                    None => (None, PathAnchor::Base, 0),
                };
                Outcome::Dynamic {
                    prefix,
                    anchor,
                    ambiguous: *ambiguous,
                    masked_segments: masked,
                }
            }
            UrlPath::Unrequestable => Outcome::Unrequestable,
        }
    }
}

/// base와 경로를 결합 방식대로 잇는다 — 벡터 러너와 추출기가 같이 쓴다. base가
/// None이면 미상 base다.
pub fn join(join: Join, base: Option<&str>, path: &[Piece]) -> Outcome {
    let dots = join != Join::HttpUriConcat;
    match join {
        Join::WhatwgJoin => {
            let base = match base {
                Some(b) => UrlVal::parse(&[Piece::Lit(b.to_string())], dots),
                None => UrlVal::opaque(dots),
            };
            base.join(path).outcome()
        }
        Join::WhatwgConcat | Join::HttpUriConcat => {
            let mut pieces = vec![match base {
                Some(b) => Piece::Lit(b.to_string()),
                None => Piece::Value(Origin::Unknown),
            }];
            pieces.extend(path.iter().cloned());
            UrlVal::parse(&pieces, dots).outcome()
        }
        Join::SlashJoin => {
            let mut rest = path.to_vec();
            if let Some(Piece::Lit(l)) = rest.first_mut() {
                *l = l.trim_start_matches('/').to_string();
            }
            match base {
                Some(b) => {
                    let mut pieces = vec![Piece::Lit(format!("{}/", b.trim_end_matches('/')))];
                    pieces.extend(rest);
                    UrlVal::parse(&pieces, dots).outcome()
                }
                None => {
                    let mut pieces = vec![Piece::Lit("/".to_string())];
                    pieces.extend(rest);
                    compose_path(PathAnchor::Base, &pieces, None, dots)
                }
            }
        }
    }
}

/// WHATWG 전처리 — 앞뒤 C0·공백을 자르고 탭·줄바꿈을 지우며 `\`를 `/`로 읽는다
/// (http(s)는 special scheme이다). 빈 리터럴은 버리고 이웃 리터럴은 합친다.
fn preprocess(pieces: &[Piece]) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    for p in pieces {
        match p {
            Piece::Lit(l) => {
                let cleaned: String = l
                    .chars()
                    .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
                    .map(|c| if c == '\\' { '/' } else { c })
                    .collect();
                if let Some(Piece::Lit(prev)) = out.last_mut() {
                    prev.push_str(&cleaned);
                } else if !cleaned.is_empty() {
                    out.push(Piece::Lit(cleaned));
                }
            }
            other => out.push(other.clone()),
        }
    }
    let is_c0_or_space = |c: char| c <= ' ';
    if let Some(Piece::Lit(l)) = out.first_mut() {
        *l = l.trim_start_matches(is_c0_or_space).to_string();
    }
    if let Some(Piece::Lit(l)) = out.last_mut() {
        *l = l.trim_end_matches(is_c0_or_space).to_string();
    }
    out.retain(|p| !matches!(p, Piece::Lit(l) if l.is_empty()));
    out
}

/// 리터럴 조각에 `..`(또는 퍼센트 인코딩 변형) 세그먼트가 있는가. query·fragment 뒤는 보지 않는다.
fn has_dot_dot(pieces: &[Piece]) -> bool {
    for p in pieces {
        if let Piece::Lit(l) = p {
            let path = l.split(['?', '#']).next().unwrap_or("");
            if path.split('/').any(is_double_dot) {
                return true;
            }
            if path.len() != l.len() {
                return false;
            }
        }
    }
    false
}

/// `scheme:` 접두사를 떼어 (소문자 scheme, 나머지)로 돌려준다.
fn split_scheme(text: &str) -> Option<(String, &str)> {
    let colon = text.find(':')?;
    let scheme = &text[..colon];
    let mut chars = scheme.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic()
        || !chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return None;
    }
    Some((scheme.to_ascii_lowercase(), &text[colon + 1..]))
}

/// 리터럴 authority에서 userinfo를 떼고 소문자 `host[:port]`만 남긴다. 계약이
/// 받는 모양(ASCII host 문자, IPv6 괄호, 숫자 port)이 아니면 싣지 않는다.
fn authority_of(raw: &str) -> Option<String> {
    let host_port = raw.rsplit_once('@').map_or(raw, |(_, h)| h);
    let lower = host_port.to_ascii_lowercase();
    let (host, port) = if lower.starts_with('[') {
        let close = lower.find(']')?;
        let rest = &lower[close + 1..];
        (&lower[..=close], rest.strip_prefix(':'))
    } else {
        match lower.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (lower.as_str(), None),
        }
    };
    // WHATWG: 빈 포트(`h:`)는 포트 없음이다(url 2.5.8 실행 확인).
    let port = port.filter(|p| !p.is_empty());
    let host_ok = if host.starts_with('[') {
        host[1..host.len() - 1]
            .chars()
            .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
    } else {
        !host.is_empty()
            && host
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
    };
    let port_ok = port.is_none_or(|p| p.chars().all(|c| c.is_ascii_digit()));
    (host_ok && port_ok).then(|| match port {
        Some(p) => format!("{host}:{p}"),
        None => host.to_string(),
    })
}

/// 조각의 첫 `?`·`#`부터 끝까지 뗀다. 뗐으면 true.
fn cut_query(pieces: &mut Vec<Piece>) -> bool {
    for i in 0..pieces.len() {
        if let Piece::Lit(l) = &pieces[i] {
            if let Some(at) = l.find(['?', '#']) {
                let head = l[..at].to_string();
                pieces.truncate(i);
                if !head.is_empty() {
                    pieces.push(Piece::Lit(head));
                }
                return true;
            }
        }
    }
    false
}

/// RFC 3986 병합 — base 경로의 마지막 `/`까지 남기고 상대 경로를 붙인다.
fn merge(base: &[Piece], rel: &[Piece]) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    for (i, p) in base.iter().enumerate().rev() {
        if let Piece::Lit(l) = p {
            if let Some(at) = l.rfind('/') {
                out.extend(base[..i].iter().cloned());
                out.push(Piece::Lit(l[..=at].to_string()));
                break;
            }
        }
    }
    if out.is_empty() {
        out.push(Piece::Lit("/".to_string()));
    }
    out.extend(rel.iter().cloned());
    out
}

/// 경로 세그먼트 하나 — 리터럴 또는 세그먼트 전체 보간.
#[derive(Clone, Debug, PartialEq, Eq)]
enum PSeg {
    Lit(String),
    Param,
}

/// 경로 조각을 정규 템플릿으로 조립한다(`compose.*` 규칙).
///
/// 1. 첫 `?`·`#`부터 뗀다(`compose.query-tail`). 끝의 [`Piece::QueryTail`]도 뗀다
///    (`compose.suffix`) — 중간에 오면 보통 값이다.
/// 2. 값은 세그먼트 전체를 채울 때만 `{}`다(`compose.interpolation`). 아니면 그
///    값 앞까지의 조립 결과를 `channelPrefix`로 싣는 dynamic이다.
/// 3. `dots`면 점 세그먼트를 지운다. base 앵커에서 알려진 경로 위로 올라가면
///    dynamic이다.
/// 4. 정규화(`compose.normalize`)·마스킹(`compose.mask`)을 적용한다.
pub fn compose_path(
    anchor: PathAnchor,
    pieces: &[Piece],
    authority: Option<&str>,
    dots: bool,
) -> Outcome {
    let mut pieces = preprocess_path(pieces);
    let mut query = cut_query(&mut pieces);
    if matches!(pieces.last(), Some(Piece::QueryTail)) {
        pieces.pop();
        query = true;
    }
    let dynamic = |prefix: Option<String>, masked: usize| Outcome::Dynamic {
        prefix,
        anchor,
        ambiguous: false,
        masked_segments: masked,
    };
    match pieces.first() {
        Some(Piece::Lit(l)) if l.starts_with('/') => {}
        _ => return dynamic(None, 0),
    }
    let segs = match segments(&pieces) {
        Ok(segs) => segs,
        Err(prefix_pieces) => {
            let (prefix, masked) = prefix_of(&prefix_pieces, authority);
            return dynamic(prefix, masked);
        }
    };
    let segs = if dots {
        match remove_dots(segs, anchor) {
            Some(s) => s,
            None => return dynamic(None, 0),
        }
    } else {
        segs
    };
    let mut rendered = to_template_segs(&segs);
    let masked = mask(authority, &mut rendered);
    let template = render(&rendered);
    if template_problem(&template).is_some() {
        return dynamic(None, 0);
    }
    Outcome::Template(Template {
        template,
        anchor,
        authority: authority.map(str::to_string),
        query_tail_stripped: query,
        masked_segments: masked,
    })
}

/// 경로 조각의 리터럴을 합친다(빈 리터럴 제거). `\`는 이미 전처리됐다고 본다.
fn preprocess_path(pieces: &[Piece]) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    for p in pieces {
        match (p, out.last_mut()) {
            (Piece::Lit(l), Some(Piece::Lit(prev))) => prev.push_str(l),
            (Piece::Lit(l), _) if l.is_empty() => {}
            (other, _) => out.push(other.clone()),
        }
    }
    out
}

/// 조각을 세그먼트로 나눈다. 세그먼트 일부만 채우는 값을 만나면 그 값 앞까지의
/// 조각(접두사 후보)을 Err로 돌려준다.
///
/// 호출자는 첫 조각이 `/`로 시작하는 리터럴임을 보장한다.
fn segments(pieces: &[Piece]) -> Result<Vec<PSeg>, Vec<Piece>> {
    let mut segs: Vec<PSeg> = Vec::new();
    // 지금 채우는 세그먼트 — 빈 리터럴이면 막 `/`를 지난 자리다.
    let mut cur = PSeg::Lit(String::new());
    for (i, p) in pieces.iter().enumerate() {
        match p {
            Piece::Lit(l) => {
                let body = if i == 0 { &l[1..] } else { l.as_str() };
                for (j, part) in body.split('/').enumerate() {
                    if j > 0 {
                        segs.push(std::mem::replace(&mut cur, PSeg::Lit(String::new())));
                    }
                    if part.is_empty() {
                        continue;
                    }
                    match &mut cur {
                        PSeg::Lit(text) => text.push_str(part),
                        // 값 바로 뒤에 `/` 없는 리터럴 — 아래 값 검사가 먼저 막는다.
                        PSeg::Param => return Err(pieces[..i].to_vec()),
                    }
                }
            }
            Piece::Value(_) | Piece::QueryTail => {
                let starts_segment = cur == PSeg::Lit(String::new());
                let ends_segment = match pieces.get(i + 1) {
                    None => true,
                    Some(Piece::Lit(next)) => next.starts_with('/'),
                    Some(_) => false,
                };
                if !(starts_segment && ends_segment) {
                    return Err(pieces[..i].to_vec());
                }
                cur = PSeg::Param;
            }
        }
    }
    segs.push(cur);
    Ok(segs)
}

/// WHATWG 점 세그먼트 제거(`.`·`..`와 `%2e` 변형). 끝 세그먼트가 점이면 빈
/// 세그먼트(끝 슬래시)를 남긴다. base 앵커에서 첫 세그먼트 위로 오르면 None.
fn remove_dots(segs: Vec<PSeg>, anchor: PathAnchor) -> Option<Vec<PSeg>> {
    let n = segs.len();
    let mut out: Vec<PSeg> = Vec::new();
    for (i, s) in segs.into_iter().enumerate() {
        let last = i + 1 == n;
        match &s {
            PSeg::Lit(t) if is_single_dot(t) => {
                if last {
                    out.push(PSeg::Lit(String::new()));
                }
            }
            PSeg::Lit(t) if is_double_dot(t) => {
                if out.pop().is_none() && anchor == PathAnchor::Base {
                    return None;
                }
                if last {
                    out.push(PSeg::Lit(String::new()));
                }
            }
            _ => out.push(s),
        }
    }
    if out.is_empty() {
        out.push(PSeg::Lit(String::new()));
    }
    Some(out)
}

/// `.` 또는 `%2e`(대소문자 무관).
fn is_single_dot(s: &str) -> bool {
    s == "." || s.eq_ignore_ascii_case("%2e")
}

/// `..`와 그 퍼센트 인코딩 변형.
fn is_double_dot(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    matches!(lower.as_str(), ".." | ".%2e" | "%2e." | "%2e%2e")
}

/// 경로 세그먼트를 템플릿 세그먼트로 바꾼다.
fn to_template_segs(segs: &[PSeg]) -> Vec<Seg> {
    segs.iter()
        .map(|s| match s {
            PSeg::Lit(t) => Seg::Lit(t.clone()),
            PSeg::Param => Seg::Param {
                prefix: String::new(),
                suffix: String::new(),
                constraint: None,
            },
        })
        .collect()
}

/// 접두사 조각을 마스킹한 정규 템플릿으로 쓴다. `/`로 시작하지 않으면 None.
fn prefix_of(pieces: &[Piece], authority: Option<&str>) -> (Option<String>, usize) {
    let pieces = preprocess_path(pieces);
    match pieces.first() {
        Some(Piece::Lit(l)) if l.starts_with('/') => {}
        _ => return (None, 0),
    }
    let Ok(segs) = segments(&pieces) else {
        return (None, 0);
    };
    let mut rendered = to_template_segs(&segs);
    let masked = mask(authority, &mut rendered);
    let text = render(&rendered);
    if template_problem(&text).is_some() {
        return (None, 0);
    }
    (Some(text), masked)
}

/// 알려진 웹훅 host와 고엔트로피 리터럴 세그먼트를 `{}`로 바꾸고 바꾼 수를 센다.
pub fn mask(authority: Option<&str>, segs: &mut [Seg]) -> usize {
    let host = authority.map(|a| {
        let h = a.rsplit_once(':').map_or(a, |(h, _)| h);
        h.to_ascii_lowercase()
    });
    let webhook_from = match host.as_deref() {
        Some("hooks.slack.com") => Some(0),
        Some("discord.com" | "discordapp.com") => {
            let lit = |i: usize, want: &str| matches!(segs.get(i), Some(Seg::Lit(t)) if t == want);
            (lit(0, "api") && lit(1, "webhooks")).then_some(2)
        }
        _ => None,
    };
    let mut count = 0;
    for (i, seg) in segs.iter_mut().enumerate() {
        let Seg::Lit(text) = seg else { continue };
        let webhook = webhook_from.is_some_and(|from| i >= from) && !text.is_empty();
        if webhook || is_high_entropy(text) {
            *seg = Seg::Param {
                prefix: String::new(),
                suffix: String::new(),
                constraint: None,
            };
            count += 1;
        }
    }
    count
}

/// 퍼센트 디코드한 값이 16자 이상이고 ASCII 글자와 숫자를 모두 담는가.
fn is_high_entropy(text: &str) -> bool {
    let decoded = percent_decode(&normalize_uri_path(text));
    decoded.chars().count() >= 16
        && decoded.chars().any(|c| c.is_ascii_alphabetic())
        && decoded.chars().any(|c| c.is_ascii_digit())
}

/// `%XX`를 바이트로 풀어 UTF-8(손실 허용)로 읽는다.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        // 바이트로 검사한다 — 문자열 슬라이스는 다중 바이트 문자 경계에서 패닉한다.
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && bytes[i + 1].is_ascii_hexdigit()
            && bytes[i + 2].is_ascii_hexdigit()
        {
            let hex = |b: u8| (b as char).to_digit(16).unwrap_or(0) as u8;
            out.push(hex(bytes[i + 1]) * 16 + hex(bytes[i + 2]));
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// 정규 템플릿 문자열을 세그먼트로 읽는다(`{}`만 파라미터) — 벡터의 `compose.mask`
/// 입력용이다.
pub fn parse_template(template: &str) -> Vec<Seg> {
    template
        .strip_prefix('/')
        .unwrap_or(template)
        .split('/')
        .map(|s| {
            if s == "{}" {
                Seg::Param {
                    prefix: String::new(),
                    suffix: String::new(),
                    constraint: None,
                }
            } else {
                Seg::Lit(s.to_string())
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(s: &str) -> Piece {
        Piece::Lit(s.to_string())
    }

    fn val() -> Piece {
        Piece::Value(Origin::Unknown)
    }

    fn tpl(o: &Outcome) -> (&str, PathAnchor) {
        match o {
            Outcome::Template(t) => (t.template.as_str(), t.anchor),
            other => panic!("not a template: {other:?}"),
        }
    }

    #[test]
    fn whatwg_join_matches_url_crate() {
        // 기대값은 url 2.5.8 `Url::join` 실행 결과다(docs/HTTP-ROUTES.md).
        let cases = [
            ("http://h/api", "x", "/x"),
            ("http://h/api/", "x", "/api/x"),
            ("http://h/a/b/c", "../x", "/a/x"),
            ("http://h/api/", "a/./b/../c", "/api/a/c"),
            ("http://h/api", "\\x", "/x"),
            ("http://h/api/", "x//y", "/api/x//y"),
            ("http://h/api/", "http:x", "/api/x"),
            ("http://h/api", "?q=1", "/api"),
            ("http://h", "x", "/x"),
        ];
        for (base, path, want) in cases {
            let o = join(Join::WhatwgJoin, Some(base), &[lit(path)]);
            assert_eq!(tpl(&o), (want, PathAnchor::Root), "{base} + {path}");
        }
        let other = join(Join::WhatwgJoin, Some("http://h/api"), &[lit("//Other/x")]);
        match other {
            Outcome::Template(t) => {
                assert_eq!(t.template, "/x");
                assert_eq!(t.authority.as_deref(), Some("other"));
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn opaque_base_relative_and_dots() {
        let o = join(Join::WhatwgJoin, None, &[lit("a/b")]);
        assert_eq!(tpl(&o), ("/a/b", PathAnchor::Base));
        let up = join(Join::WhatwgJoin, None, &[lit("a/../x")]);
        assert!(
            matches!(
                up,
                Outcome::Dynamic {
                    ambiguous: true,
                    ..
                }
            ),
            "{up:?}"
        );
        let q = join(Join::WhatwgJoin, None, &[lit("?x")]);
        assert!(matches!(
            q,
            Outcome::Dynamic {
                ambiguous: true,
                ..
            }
        ));
        let empty = UrlVal::opaque(true).join(&[]).outcome();
        assert!(matches!(
            empty,
            Outcome::Dynamic {
                ambiguous: true,
                ..
            }
        ));
        let known_empty = UrlVal::parse(&[lit("http://h/a")], true)
            .join(&[])
            .outcome();
        assert!(matches!(known_empty, Outcome::Template(_)));
        let query_dots = join(Join::WhatwgJoin, None, &[lit("x?next=../y")]);
        assert_eq!(tpl(&query_dots), ("/x", PathAnchor::Base));
        // 두 번 잇기 — 미상 base 뒤 `api/` + `users`.
        let u = UrlVal::opaque(true)
            .join(&[lit("api/")])
            .join(&[lit("users")]);
        assert_eq!(tpl(&u.outcome()), ("/api/users", PathAnchor::Base));
    }

    #[test]
    fn concat_parses_like_url_parse() {
        let o = join(Join::WhatwgConcat, Some("http://h/a/../b"), &[lit("/c")]);
        assert_eq!(tpl(&o), ("/b/c", PathAnchor::Root));
        let keep = join(Join::HttpUriConcat, Some("http://h/a/../b"), &[lit("/c")]);
        assert_eq!(tpl(&keep), ("/a/../b/c", PathAnchor::Root));
        let dynamic_host = UrlVal::parse(&[lit("http://"), val(), lit("/users/"), val()], true);
        assert_eq!(
            tpl(&dynamic_host.outcome()),
            ("/users/{}", PathAnchor::Base)
        );
        let relative = UrlVal::parse(&[lit("/users")], true);
        assert_eq!(relative.outcome(), Outcome::Unrequestable);
        let glued = UrlVal::parse(&[val(), lit("users")], true);
        assert!(matches!(
            glued.outcome(),
            Outcome::Dynamic {
                ambiguous: true,
                ..
            }
        ));
        let ftp = UrlVal::parse(&[lit("ftp://h/x")], true);
        assert_eq!(ftp.outcome(), Outcome::Unrequestable);
        assert_eq!(
            relative.join(&[lit("/x")]).outcome(),
            Outcome::Unrequestable
        );
        let query_only = UrlVal::parse(&[lit("https://h?x=1")], true);
        match query_only.outcome() {
            Outcome::Template(t) => {
                assert_eq!(t.template, "/");
                assert!(t.query_tail_stripped);
            }
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn authorities_are_validated() {
        assert_eq!(
            authority_of("u:p@API.Example.com:8080").as_deref(),
            Some("api.example.com:8080")
        );
        assert_eq!(authority_of("[::1]:3000").as_deref(), Some("[::1]:3000"));
        assert_eq!(authority_of("caf\u{e9}.com"), None);
        // 밑줄 host와 빈 포트는 WHATWG가 받는다(url 2.5.8: `my_api.example.com`, `h:` → 포트 없음).
        assert_eq!(authority_of("My_Api.test").as_deref(), Some("my_api.test"));
        assert_eq!(authority_of("h:").as_deref(), Some("h"));
        assert_eq!(authority_of("h:x1"), None);
    }

    #[test]
    fn url_round_trips_through_pieces() {
        let u = UrlVal::parse(&[lit("http://h/v1/")], true).join(&[lit("items")]);
        let again = UrlVal::parse(&u.to_pieces(), true);
        assert_eq!(tpl(&again.outcome()), ("/v1/items", PathAnchor::Root));
        let opaque = UrlVal::opaque(true).to_pieces();
        assert_eq!(opaque, vec![val()]);
    }

    #[test]
    fn percent_decode_handles_edges() {
        assert_eq!(percent_decode("a%41"), "aA");
        assert_eq!(percent_decode("a%4"), "a%4");
        assert_eq!(percent_decode("%zz"), "%zz");
    }
}
