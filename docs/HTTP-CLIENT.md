# HTTP 호출 규칙 (`rustograph routes --role client`)

rustograph가 Rust 클라이언트 코드에서 isthmus http `route-call` 사실을 만드는 규칙과 근거를 적는다. 계약 정본은
isthmus [GRAPH-EXCHANGE "HTTP 경계"](https://github.com/ictechgy/isthmus/blob/main/docs/GRAPH-EXCHANGE.md)와
[HTTP-WRAPPERS](https://github.com/ictechgy/isthmus/blob/main/docs/HTTP-WRAPPERS.md)(url-compose 규칙·래퍼 선언)다.
서버 선언 규칙은 [HTTP-ROUTES.md](HTTP-ROUTES.md)에 있다.

```sh
rustograph routes --role client [--dir DIR] [--out FILE] [--wrappers http-wrappers.json] [--service NAME]
```

- 문서: bridge-facts v1, `platform: "rust"`, `target: "http"`, `roles: ["client"]`, `sourceSets.tests: "excluded"`,
  선택 `service`(`--service`). 사실이 0건이어도 target은 `http`다(스캔했으나 호출 없음). `dispatch`는 없다.
- 스캔: 모든 워크스페이스 멤버의 lib·bin 타깃, `impact`와 같은 syn 수확(모듈 트리·AST·정점 집합). `#[cfg(test)]` 모듈과
  `#[test]` 함수는 뺀다. 컴파일·실행·네트워크 접근이 없다.
- `symbol.usr`: 호출을 감싸는 가장 안쪽 그래프 정점(함수·메서드·트레이트 기본 메서드, 상수·static 초기식) — `impact`·
  `reach`의 정점 ID와 같다. 중첩 함수·클로저 안의 호출은 바깥 정점이다. 정점이 없으면 usr 없이 내고
  `missing-route-usrs:`로 센다. `verify-cli-contract.sh`가 `impact --roots-from`으로 모든 usr가 정점임을 확인한다.
- `location`: 호출식이 시작하는 줄(`wrapper.location` — 메서드 체인은 수신자 시작)과 UTF-8 바이트 열 + 1.
- dynamic 사실은 `channel: null`이다 — 원문 식은 URL의 userinfo·query를 담을 수 있어 싣지 않는다. 증명한 리터럴
  접두사만 마스킹한 `channelPrefix`로 싣는다.
- `authority`: base가 리터럴이고 root 앵커일 때만 싣는 소문자 `host[:port]`(userinfo 제거, 포트는 적힌 대로).
- `baseRef`: base URL이 구조체 필드(`app::api::ApiClient::base_url`)나 상수·static(`app::HOST`)에서 오면 그 id다.
  필드 id는 그래프 정점이 아니다 — workspace 매니페스트 `match.baseRefs`에 쓰는 안정 이름이다.

## 라이브러리와 결합 방식

| 라이브러리(확인 버전) | 인식 | URL 해석 | 근거 |
|---|---|---|---|
| reqwest 0.13.5(0.12.28도 오라클 실행) | `reqwest::get`·`reqwest::blocking::get`, `Client`·`blocking::Client`의 `get`·`post`·`put`·`patch`·`delete`·`head`·`request(Method::X, url)`, `Request::new(Method::X, url)` | 문자열은 `url::Url::parse`(WHATWG), `Url`은 그대로 | `src/into_url.rs` `IntoUrlSealed for &str`(`Url::parse(self)` 뒤 `has_host()` 검사), `src/async_impl/client.rs` `execute_request`(http·https 외 scheme 거부) |
| ureq 3.4.2 | `ureq::get`·`post`·`put`·`delete`·`patch`·`head`·`options`·`trace`, `Agent`의 같은 메서드 | `http::Uri` — **점 세그먼트를 지우지 않는다** | `src/lib.rs` `pub fn get<T>(uri: T) where Uri: TryFrom<T>`, 오라클 `ureq_dots` |
| ureq 2.x | 위와 같은 자유 함수·`Agent`, `request(method, url)`·`request_url` | `url::Url::parse`(소스 기준, 실행 미확인 — 버전 한계를 낸다) | — |
| url 2.5.8 | `Url::parse(s)`(reqwest 재수출 `reqwest::Url` 포함), `url.join(p)` 체인 | WHATWG URL Standard | url 크레이트 실행 결과(아래) |

**결합 방식 이름(제안).** isthmus url-compose 벡터의 `join` 입력에 Rust 결합을 더할 때 쓸 이름이다. 벡터에 Rust 이름이
생기면 러너를 그 이름으로 옮긴다.

| 이름 | 쓰는 곳 | `/x`(미상 base) | `x`(미상 base) | base 리터럴 |
|---|---|---|---|---|
| `whatwg-concat` | `format!`·`+`·`concat!`로 이은 문자열을 reqwest·ureq 2에 넘김 | base | dynamic + `ambiguous-base-join:` | 이은 문자열을 WHATWG로 해석(점 세그먼트 제거, `//` 보존), root |
| `whatwg-join` | `url::Url::join` | root | base(RFC 3986 병합) | WHATWG 상대 해석, root |
| `http-uri-concat` | 이은 문자열을 ureq 3에 넘김 | base | dynamic + `ambiguous-base-join:` | 이은 문자열 그대로(점 세그먼트 보존), root |

WHATWG 결합은 http(s)에서 RFC 3986과 같다. 다른 점은 셋이고 모두 구현했다: `\`를 `/`로 읽고, 같은 scheme의
`http:x`는 상대 참조이며, 앞뒤 C0·공백과 탭·줄바꿈을 지운다. url 2.5.8로 실행해 확인한 값(단위 테스트
`whatwg_join_matches_url_crate`가 고정): `http://h/api` + `x` → `/x`, `http://h/api/` + `x` → `/api/x`,
`http://h/a/b/c` + `../x` → `/a/x`, `http://h/api` + `\x` → `/x`, `http://h/api/` + `http:x` → `/api/x`,
`http://h/api` + `//other/x` → host `other`의 `/x`, `http://h/api` + `?q=1` → `/api`.

문자열 연결의 base 미상 행은 dio 단순 연결과 결과가 같아 공유 벡터의 `dio-concat` 사례를 `whatwg-concat`으로 실행한다.
`slash-join` 사례는 벡터용 `Join::SlashJoin`으로 실행한다(그 방식을 쓰는 Rust 라이브러리는 모델링하지 않았다).

## 해석과 증명

- 문자열: 리터럴, `format!`/`format_args!`(위치·이름·인라인 캡처 인자, `{{`·`}}`), `concat!`, `a + b`,
  `to_string`·`to_owned`·`into`·`as_str`·`clone`·`String::from` 같은 전달, 워크스페이스 `const`·`static`·연관 상수
  (`Self::BASE`)를 따라간다. `{:?}`처럼 서식 지정이 있으면 그 값은 모르는 값이고, `{:>1$}`처럼 인자 순서를 바꾸는
  서식은 문자열 전체를 모른다.
- 지역 변수: 스코프 스택(블록·클로저·`for`·`match` 팔·`if let`/`while let`·let-else)으로 그림자를 따르고, 함수 안에서
  대입·`&mut`·`push_str` 같은 수정 메서드가 닿는 이름은 믿지 않는다. 매개변수는 출처(`Origin::Param`)를 단 값이다.
- query 꼬리(`compose.suffix`): 끝에 붙은 지역 변수의 `if`/`match` 모든 가지가 `?` 리터럴로 시작하거나 빈 문자열
  (`String::new()` 포함)이면 떼고 `queryTailStripped`를 단다. `else` 없는 `if`는 증명하지 못한다.
- 구조체 필드 base: 크레이트의 모든 구조체 리터럴(`Self { .. }` 포함)이 그 필드를 **같은 리터럴·상수 값**으로 채우면
  그 값이다(인스턴스와 무관하게 `self.f`·`x.f` 모두). 생성 위치가 매개변수로 채우거나, 값이 둘 이상이거나,
  `..rest` 생략, `#[derive(Default)]`, 필드 대입·`&mut`·수정 메서드가 있으면 값을 버리고 base 앵커 + `baseRef`다.
  `Url` 필드도 같다(`Url::parse(리터럴).unwrap()`).
- 수신자 판정: 식의 타입을 구문으로 추론한다 — 생성자(`Client::new()`·`Client::builder()…build()?`·`ureq::agent()`·
  `Agent::new_with_defaults()`·`AgentBuilder`·`config_builder()…new_agent()`), 타입 주석이 있는 지역 변수·매개변수,
  구조체 필드 타입, 워크스페이스 함수·메서드의 반환 타입, `static`의 선언 타입. `&`·`Arc`·`Rc`·`Box`·`Lazy`·
  `LazyLock`·`OnceLock`·`Result`는 벗긴다. 증명하지 못한 수신자에서 보낸 요청 모양 호출(`x.get(u).send()`)은 사실로
  내지 않고 센다.
- 동사: 메서드 이름, `reqwest::Method::X`·`http::Method::X` 경로, ureq `request("GET", ..)`의 정확한 대문자 리터럴만
  동사다. 그 밖은 `methodDynamic: true`다.
- base 없는 상대 URL(`client.get("/x")`)과 http(s)가 아닌 scheme은 클라이언트가 보내기 전에 거부하므로 사실을 내지 않고
  센다(오라클 `relative_url`: 요청 없음).
- 모델링하지 않는 클라이언트(hyper·hyper-util의 client 경로, surf, awc, isahc, attohttpc, minreq, curl, ehttp,
  gloo-net, reqwasm, http-client, reqwest-middleware)는 멤버가 직접 의존할 때 경로 사용 위치를 크레이트별로 센다.

## 래퍼 선언(`http-wrappers` v1)의 Rust 규칙

`"language": "rust"` 항목만 적용한다(다른 언어 항목은 공백으로도 세지 않는다). 선언 오류는 종료 코드 2다.

- `function`: `owner::name`이 rustograph 정점 ID다. 자유 함수는 모듈 경로(`app::net` + `send`), 메서드·연관 함수는
  타입 ID(`app::api::ApiClient` + `request`), 트레이트 impl 메서드는 `app::api::ApiClient::<Trait>` + 이름.
- `constructor`: `owner`는 타입 ID. `name`이 타입 이름과 같으면 구조체 리터럴(`Endpoint { verb, path }` — `label`은
  필드 이름)이나 튜플 구조체 생성(`Ep(m, p)` — `index`)이고, 다르면 연관 함수(`Endpoint::new`)다.
- 인자: Rust에는 이름 붙은 인자가 없으므로 함수·메서드는 `index`(메서드는 수신자를 빼고 0부터, UFCS 호출도
  같다)로 묶는다. `label`은 구조체 리터럴 필드에만 맞는다. `methodEnum`은 enum case·연관 상수 경로의 마지막 이름
  (`Verb::Post` → `Post`)과 문자열 리터럴을 case로 본다.
- 경로 인자: 전체 URL이면 절대 해석, `/`로 시작하면 선언 `pathAnchor`, 상대 경로는 래퍼 안 결합을 모르므로 dynamic
  (base 앵커면 `ambiguous-base-join:`). 래퍼 경로에는 `baseRef`를 싣지 않는다.
- 선언된 래퍼 본문의 dynamic 호출은 내지 않는다(래퍼 호출 사실이 대신한다). 선언되지 않은 함수가 매개변수를 URL
  전체·base 뒤 꼬리로 흘려보내거나 동사 매개변수를 넘기면 `http-wrapper-undeclared:`로 센다(세그먼트 일부를 채우는
  매개변수는 래퍼 선언으로 풀리지 않아 세지 않는다).
- 정점·구조체에 닿지 않는 선언과 호출 0건 선언은 `http-wrapper-unresolved:`다. `--service`와 다른 `service`를 선언한
  래퍼가 있으면 isthmus가 문서를 거부하므로 먼저 사용법 오류로 막는다.

## limitation

| 접두사 | 뜻 |
|---|---|
| `route-call-coverage:` | 모델링하지 않는 클라이언트 사용 수(크레이트별), 증명하지 못한 수신자의 요청 모양 호출 수, 보내기 전 거부되는 URL 수, 확인한 버전 밖(reqwest 0.12·0.13, ureq 3 밖)의 의존, 위치를 못 찾은 호출, cargo metadata 한계 |
| `ambiguous-base-join:` | 미상 base 뒤 상대 경로 수 |
| `http-wrapper-undeclared:` | 선언되지 않은 래퍼 싱크 수 |
| `http-wrapper-unresolved:` | 정점에 닿지 않거나 호출이 0건인 rust 래퍼 선언(`wrappers[n]`) |
| `missing-route-usrs:` | usr가 없는 사실 수 |

호출 측 공백은 증명할 수 있는 요청 범위가 없어 `limitationScopes`를 내지 않는다(문서 전체 효과).

## 공유 적합성 벡터

`conformance/url-compose.json`(isthmus `76b6141`)의 `producer` 사례 41건을 `tests/client_routes.rs`가 제품 함수로
실행한다(`compose::compose_path`·`join`·`UrlVal::parse`·`mask`, `wrappers::bind_method`). `wrapper.location`은 실제
스캐너로 여러 줄 호출의 시작 줄과 UTF-8 열을 확인한다. `producer:kartograph`(Spring) 13건은 적용하지 않는다.

## 모의 서버 오라클

`tests/fixture-client/app/`은 각 규칙을 부르는 합성 클라이언트이고, `experiments/client-oracle/`은 그 lib.rs를
crates.io의 진짜 reqwest 0.13.5·ureq 3.4.2·url 2.5.8로 컴파일해 시나리오를 실행한다. 모든 http 요청은 환경 프록시
(`HTTP_PROXY`)로 지정한 127.0.0.1 임시 포트의 기록 서버로 간다(ureq 3은 http 요청도 CONNECT 터널을 연다 — 서버가
터널을 수락하고 안의 평문 요청을 읽는다). 외부 네트워크 요청은 없고, 서버는 오라클 프로세스 안의 스레드라 함께
끝난다. 판정은 root는 전체 경로, base는 세그먼트 경계 꼬리, `{}`는 비어 있지 않은 세그먼트, authority는 요청 host이고,
정적 사실은 모두 어떤 요청과 맞아야 한다(정밀도). 재기록은 `experiments/client-oracle/run.sh`, 기본 테스트는 커밋된
기록(`recorded/client.json`)을 스텁 의존(`tests/fixture-client/stubs`, 이름·버전만)으로 해석한 사실과 오프라인 대조한다.

2026-09-30 기록: **41개 시나리오, 일치 37 · dynamic 3 · 요청 없음 1 · 불일치 0**, 귀속되지 않은 사실 0. 같은
소스를 reqwest 0.12.28로 바꾼 scratch 실행도 같은 결과였다.

| 시나리오 | 기록 요청 | route-call 사실 | 결과 |
|---|---|---|---|
| `literal_query` | GET /v1/items | GET `/v1/items` root | 일치 |
| `async_user` | GET /v1/users/7 | GET `/v1/users/{}` root | 일치 |
| `concat_post` | POST /v1/orders | POST `/v1/orders` root | 일치 |
| `put_positional` | PUT /v1/items/5 | PUT `/v1/items/{}` root | 일치 |
| `delete_plus` | DELETE /v1/items/9 | DELETE `/v1/items/{}` root | 일치 |
| `patch_local` | PATCH /v1/profile | PATCH `/v1/profile` root | 일치 |
| `head_health` | HEAD /v1/health | HEAD `/v1/health` root | 일치 |
| `request_options` | OPTIONS /v1/items | OPTIONS `/v1/items` root | 일치 |
| `request_dynamic` | PATCH /v1/verbs | 동사 동적 `/v1/verbs` root | 일치 |
| `join_relative` | GET /v2/users/5 | GET `/v2/users/{}` root | 일치 |
| **`join_replaces_last`**(RFC 3986 병합) | GET /v2/tags | GET `/v2/tags` root | 일치 |
| `join_absolute_path` | GET /root/ping | GET `/root/ping` root | 일치 |
| `dot_segments` | GET /v1/b | GET `/v1/b` root | 일치 |
| **`double_slash`**(`//` 보존) | GET /v3//items | GET `/v3//items` root | 일치 |
| `query_tail` | GET /v1/search | GET `/v1/search` root | 일치 |
| `query_tail_empty` | GET /v1/search | GET `/v1/search` root | 일치 |
| `partial_segment` | GET /v1/files/report.json | GET dynamic, prefix `/v1/files/` | dynamic |
| `unknown_base_rooted` | GET /prefix/v1/status | GET `/v1/status` base | 일치 |
| `unknown_base_glued` | GET /prefixstatus | GET dynamic(`ambiguous-base-join:`) | dynamic |
| `relative_url` | 없음 — 보내기 전 실패 | 없음 | 요청 없음 |
| `masked_token` | GET /v1/tokens/a1b2c3d4e5f6a7b8c9d0 | GET `/v1/tokens/{}` root | 일치 |
| `non_ascii` | GET /v1/caf%C3%A9 | GET `/v1/caf%C3%A9` root | 일치 |
| `userinfo_fragment` | GET /v1/secure | GET `/v1/secure` root | 일치 |
| `with_port` | GET /v1/port | GET `/v1/port` root(authority `api.example.com:8080`) | 일치 |
| `backslashes` | GET /v1/bs | GET `/v1/bs` root | 일치 |
| `builder_client` | GET /v1/built | GET `/v1/built` root | 일치 |
| `struct_field_base` | GET /api/items | GET `/api/items` root | 일치 |
| `struct_field_item` | GET /api/items/42 | GET `/api/items/{}` root | 일치 |
| `struct_field_plus` | POST /api/items | POST `/api/items` root | 일치 |
| `struct_field_query` | GET /api/search | GET `/api/search` root | 일치 |
| `struct_param_base` | GET /remote/users/3 | GET `/users/{}` base | 일치 |
| `url_field` | GET /cat/products | GET `/cat/products` root | 일치 |
| `ureq_get` | GET /v1/ureq/items | GET `/v1/ureq/items` root | 일치 |
| `ureq_delete` | DELETE /v1/ureq/items/4 | DELETE `/v1/ureq/items/{}` root | 일치 |
| **`ureq_dots`**(점 세그먼트 보존) | GET /v1/x/../ureq-dots | GET `/v1/x/../ureq-dots` root | 일치 |
| `ureq_agent_head` | HEAD /v1/ureq/health | HEAD `/v1/ureq/health` root | 일치 |
| `ureq_post` | POST /v1/ureq/orders | POST `/v1/ureq/orders` root | 일치 |
| `wrapper_function` | GET /wb/w/items | GET `/w/items` base | 일치 |
| `wrapper_method` | POST /api/w/orders | POST `/w/orders` base | 일치 |
| `wrapper_constructor` | DELETE /api/e/items/6 | DELETE `/e/items/{}` base(+ `execute`의 동사 동적 dynamic) | 일치 |
| `undeclared_sink` | GET /v1/undeclared | GET dynamic(`http-wrapper-undeclared:`) | dynamic |

## isthmus 호환

isthmus main(`c395c59`)은 rust `route-call`을 받지 않는다(`src/exchange/parse.ts` `routeKindPlatforms`, "Fact kind is
not valid for platform"). 그 집합에 `rust`를 더한 scratch isthmus로 fixture 문서와 axum08 서버 문서를 workspace
`trace`에 넣어 확인했다(HANDOFF의 e2e 절). isthmus가 rust route-call을 받는 변경을 머지하면 이 절을 갱신한다.
