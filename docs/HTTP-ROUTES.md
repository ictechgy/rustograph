# HTTP 라우트 선언 규칙 (`rustograph routes --role server`)

rustograph가 axum·actix-web 프로젝트에서 isthmus http `route-decl` 사실을 만드는 규칙과, 각 규칙을 확인한 공식
소스를 적는다. 규칙은 추정하지 않고 아래 버전의 crates.io 소스를 직접 읽어 확인했고(2026-09-30), 경계가
애매한 규칙은 합성 fixture를 진짜 크레이트로 컴파일해 요청을 보내는 오라클로 실측했다(아래 "오라클 검증").
소스 경로는 각 크레이트 루트 기준이다.

| 크레이트 | 확인한 버전 | rustograph가 받는 선언 범위 |
|---|---|---|
| axum | 0.7.9 (matchit 0.7.3), 0.8.9 (matchit 0.8.4) | 0.7.x·0.8.x |
| actix-web | 4.15.0 (actix-router 0.5.4, actix-web-codegen 4.4.0) | 4.x |
| tower-http | 0.6.11 (`NormalizePathLayer`) | 0.6.x |

버전은 `cargo metadata`의 resolve가 고정한 패키지 버전(= Cargo.lock)으로 멤버 크레이트마다 고른다. 확인한 메이저
밖이면 가까운 문법(axum 0.6 → 0.7 문법, 그 밖 → 0.8 문법, actix 4 밖 → 4 규칙)으로 읽고
`route-framework-version-unknown:` 한계를 낸다. 분석 대상은 syn으로만 읽는다 — 컴파일·실행·네트워크 접근이 없다.
워크스페이스에 두 프레임워크가 모두 있으면 추측하지 않고 `--framework axum|actix`를 요구한다(문서 하나는
dispatch 하나만 선언한다). 둘 다 없으면 사실 0건과 `route-coverage:`를 낸다.

## 심볼 id

`symbol.usr`(와 `qualifiedName`)는 `impact`·`reach`와 같은 rustograph 정점 ID다 — 같은 syn 수확의 모듈 트리로
핸들러 경로를 해석하고 그래프 정점 집합으로 확인한다. `reach --roots-from routes.json`이 그대로 root로 받는다
(verify-cli-contract가 확인한다).

| 핸들러 | id 예시 |
|---|---|
| 모듈 수준 함수 | `axum_app::handlers::items::list` |
| 연관 함수 `Api::list` | `app::Api::list` (트레이트 impl이면 `Type::<Trait>::m` 중 유일한 것) |
| actix 매크로 핸들러(`#[get]`) | 그 함수 — `actix_app::handlers::index` |
| 클로저 `get(\|\| async {..})` | 감싸는 함수(예: `axum_app::routes::api`) + `missing-route-usrs:` |
| tower 서비스·리다이렉트·해석 못 한 경로 | usr 없음 + `missing-route-usrs:` |

## axum

| 규칙 | 확인한 소스 |
|---|---|
| 0.7: `:`·`*`는 세그먼트 어디서든 와일드카드를 시작해 다음 `/`까지 간다. 앞 글자는 정적 접두사(`/user_:id`), 뒤 글자는 이름에 흡수된다(`/:id.json`은 이름 `id.json`인 세그먼트 전체 파라미터). 한 세그먼트에 둘은 `TooManyParams` | matchit 0.7.3 `src/tree.rs:651-670`, `:661` |
| 0.7: `*rest`는 `/` 바로 뒤·경로 끝에만 오고 빈 나머지와 맞지 않는다(`/files/*p`는 `/files`·`/files/` 불일치). `{`·`}`는 리터럴 | `tree.rs:266-275`, `:350`, `:482-499`; axum 0.7.9 `src/docs/routing/route.md:53-56` |
| 0.8: `{name}`은 앞에 정적 글자를 둘 수 있지만 뒤에는 못 둔다(`/{id}.json`은 삽입 오류). `{*name}`은 끝에만, `{{`·`}}`는 리터럴 중괄호 | matchit 0.8.4 `src/tree.rs:752-803`(`:783-788`), `src/error.rs:15-19`, `src/escape.rs:14-31`, `tree.rs:372-374` |
| 0.8: `{*rest}`도 빈 나머지와 맞지 않는다 | `tree.rs:491-507`, `:510`, `:614-632`; axum 0.8.9 `src/docs/routing/route.md:53-56` |
| 0.8: `:`·`*`로 **시작하는** 세그먼트는 기동 시 panic(`without_v07_checks`로 끌 수 있음). 세그먼트 중간의 `:`는 리터럴 | axum 0.8.9 `src/routing/path_router.rs:53-73`, `src/routing/mod.rs:170-174` |
| 우선순위: 정적 > 파라미터 > catch-all, 되돌아가기 있음(`/a/b/c`와 `/:x/b/d`에서 `/a/b/d`는 뒤쪽). 충돌 경로는 삽입 시 panic | 0.7.3 `tree.rs:164-174`, `:312-330`, `:361-384`; 0.8.4 `tree.rs:450-469`, `:526-559`; axum `route.md:154-155` |
| 끝 슬래시는 엄격하다 — `/foo`와 `/foo/`는 다른 경로이고 리다이렉트가 없다(불일치는 fallback으로) | axum 0.7.9 `path_router.rs:381-385`; matchit 0.8.4 `error.rs:116-119`, axum 0.8.9 `path_router.rs:418` |
| 경로를 찾은 뒤 method가 맞지 않으면 405이고 다른 라우트·fallback을 다시 시도하지 않는다. `get`은 HEAD도 받는다(명시 `head`가 먼저) | axum 0.7.9 `src/routing/method_routing.rs:54-56`, `:746-751`, `:1157-1159`, `path_router.rs:372-375` |
| `any(h)` = fallback이 있는 MethodRouter, `on(MethodFilter, h)`는 필터 비트마다, `MethodFilter`는 CONNECT·DELETE·GET·HEAD·OPTIONS·PATCH·POST·PUT·TRACE | `method_routing.rs:464-471`, `:506-513`, `src/routing/method_filter.rs:29-45` |
| 같은 경로의 `.route()` 두 번은 MethodRouter를 합친다(같은 method가 겹치면 panic) | 0.7.9 `path_router.rs:59-73`, `method_routing.rs:1038-1083` |
| `nest(prefix, r)`: `prefix`가 `/`로 끝나면 `prefix + path.trim_start('/')`, 안쪽이 `/`면 `prefix`, 아니면 `prefix + path`(두 버전 같음). 안쪽 `/`는 `/api`만 만든다(`/api/`는 아님) | 0.7.9 `path_router.rs:496-507`, `src/routing/tests/nest.rs:350-383`; 0.8.9 `path_router.rs:535-546`, `tests/nest.rs:368-373` |
| nest 경로 검증: 0.7은 빈 경로를 `/`로, `*` 포함 panic. 0.8은 루트 nest panic("use merge"), `{*`로 시작하는 세그먼트 panic | 0.7.9 `path_router.rs:482-494`; 0.8.9 `path_router.rs:517-533`, `mod.rs:210-212` |
| `merge`는 다른 라우터의 라우트를 같은 규칙으로 다시 넣는다 | 0.7.9 `path_router.rs:135-173` |
| Router fallback은 경로가 맞지 않은 요청만 받는다(method 불일치는 405). 안쪽 라우터에 fallback이 있으면 nest가 `{p}`·`{p}/{*..}`로 옮긴다 | `src/docs/routing/fallback.md:24-29`, 0.7.9 `mod.rs:209-211`, `:379-394` |
| `nest_service(p, svc)`는 `{p}/{*tail}`·`{p}`·`{p}/`를 등록한다. `route_service(p, svc)`는 단일 경로·모든 method | 0.7.9 `path_router.rs:213-250`, `mod.rs:177-185` |
| `Router::layer` 미들웨어는 **라우팅 뒤**에 돈다 — URI를 바꾸는 미들웨어(`NormalizePathLayer`)는 Router 바깥을 감쌀 때만 라우팅에 영향을 준다 | `src/docs/routing/layer.md:57-62`, `src/docs/middleware.md:525-530` |
| tower-http `NormalizePathLayer::trim_trailing_slash()`는 앞뒤 슬래시를 모두 떼고 `/` 하나를 붙인다(루트는 `/`). `append_trailing_slash()`는 끝에 `/` 하나 | tower-http 0.6.11 `src/normalize_path.rs:146-173`, `:175-207` |
| matchit은 **마지막이 아닌** 세그먼트의 파라미터(세그먼트 전체, 앞 글자 붙은 것 모두)가 빈 값과도 맞는다(`/items//tags/x`, `/v/status`). 마지막 세그먼트의 파라미터는 빈 값과 맞지 않는다(`/items/`, `/tag_` 불일치) | 오라클 실측(0.7.9·0.8.9) |
| matchit은 요청의 **원문(퍼센트 인코딩된) 경로**를 바이트로 비교한다 — 라우트 리터럴 `{braces}`는 `/%7Bbraces%7D` 요청과 맞지 않는다 | 오라클 실측(0.8.9) |

### 사실로 바꾸는 방법

- **dispatch**: `specificity`. isthmus 구체성(리터럴 > 부분 세그먼트 > 제약 있는 `{}` > `{}` > `{**}`)은 matchit 우선순위와
  같은 방향이다. axum은 경로를 먼저 고르고 method가 맞지 않으면 405인데 isthmus는 method를 먼저 거르므로, 차이는
  거짓 match 쪽(정적 경로가 GET만, 파라미터 경로가 POST를 받을 때 `POST /x/static`)이지 거짓 error가 아니다.
- **라우터 평가**: 서빙되는 식(`axum::serve(_, X)`, `X.into_make_service()`)을 루트로, 같은 함수의 `let`·재대입과
  크레이트 함수 호출(반환식)을 따라 `route`·`nest`·`merge`·`nest_service`·`route_service`·`fallback`을 정적으로 편다.
  `layer`·`route_layer`·`with_state` 등은 투명하다. 루트에서 닿지 않은 라우터 함수(`Router::new()`를 부르는 함수)의
  선언은 `pathAnchor: "base"`와 `unresolved-route-prefix:`(선언 템플릿의 `templateSuffixes` 스코프)로 낸다.
- **channel**: nest 결합을 axum과 같은 문자열 규칙으로 한 뒤 버전별 문법으로 해석한다. 파라미터는 `{}`(앞 글자가
  있으면 `p{}`), catch-all은 `{**}`. 리터럴이 정규형과 다르면(중괄호·공백·ASCII 밖 글자·인코딩된 unreserved)
  정규 템플릿으로 보낸 요청이 닿지 않으므로 선언 대신 그 템플릿 스코프의 `route-coverage:`를 낸다.
- **빈 값 변형**: 마지막이 아닌 세그먼트의 파라미터마다 빈 값으로 채운 변형(`/items//tags/{}`, `/v/status`)을 같은
  method·symbol·location으로 함께 낸다. 16개를 넘으면 dynamic과 `route-template-expansion-capped:`다. 같은 앵커·템플릿의
  진짜 선언이 있으면 변형은 뺀다 — 정적 경로가 먼저 골라지고 경로가 맞으면 method가 달라도 405로 끝나므로
  (`/v{x}/status` GET과 `/v/status` POST가 있으면 `GET /v/status`는 405, 오라클 실측) 변형 핸들러에 닿는 요청이 없다.
- **method**: 동사 생성자·체인은 그 동사, `any`·MethodRouter `fallback`은 `ANY`, `on`은 필터의 동사(모르는 필터는
  `ANY`). `get`의 자동 HEAD는 내지 않는다(소비자 `head-as-get`). CONNECT는 http 도메인 동사가 없어 한계다.
- **trailingSlash**: `strict`, `{**}`로 끝나면 생략. 레이어가 서비스를 감싸는 모양
  (`NormalizePathLayer::trim_trailing_slash().layer(svc)`, `ServiceBuilder::new()….layer(NormalizePathLayer::…).service(svc)`,
  `NormalizePath::trim_trailing_slash(svc)`)이 있으면 trim은 끝 슬래시 없는 템플릿을, append는 끝 슬래시 있는 템플릿을
  `optional`로 낸다. `Router::layer`·`MethodRouter::layer` 인자나 변수로 넘긴 레이어처럼 라우팅 전에 동작한다는 증거가
  없으면 효과가 없다고 본다(틀리면 선언이 strict로 남아 소비자는 끝 슬래시 불일치 경고를 낸다 — 거짓 error가 아니다).
- **location**: `.route()`의 경로 인자(줄, UTF-16 열). 경로 인자가 `&str` 상수면 그 값을 쓴다.

## actix-web

| 규칙 | 확인한 소스 |
|---|---|
| `{name}`은 `[^/]+`(비어 있지 않은 한 세그먼트), `{name:regex}`는 이름 붙은 그룹에 그대로 들어가 `/`를 넘을 수 있다. 전체는 `^…$`로 묶인다 | actix-router 0.5.4 `src/resource.rs:907`, `:956`, `:986`, `:1053-1062`, 문서 `:140-142` |
| 끝의 `{name}*`는 `.*`(빈 값·`/` 포함)이고 커스텀 정규식과 함께 쓰면 panic. `/files/{tail}*`는 `/files/`와 맞지만 `/files`와는 아니다 | `resource.rs:908`, `:935`, `:939`, 테스트 `:1338-1348` |
| 한 세그먼트에 정적 글자와 파라미터, 파라미터 여럿을 섞을 수 있다(최대 16) | `resource.rs` 문서 `:78-80`, `:1214`, `:16` |
| 매칭은 대소문자를 구분하고, 요청 경로는 `%25`·`%2F`·`%2B`만 남기고 디코드한 경로다 | `resource.rs:832-845`; `src/url.rs:4`, `src/quoter.rs:41-44` |
| 스코프 접두사는 세그먼트 경계로 매치해 소비하고 안쪽은 나머지로 정확 매치한다 — 결합은 문자열 연결과 같다(`/api`+`""`=`/api`, `/api`+`/`=`/api/`, `/api`+`/items`). `root_prefix`는 앞 `/`를 붙이고 안쪽은 `ensure_leading_slash` | `resource.rs:327-329`, `:503-519`; actix-web 4.15.0 `src/scope.rs:453-458`, `src/resource.rs:428-432`, `src/dev.rs:30-47`, `src/introspection.rs:576-594`, 테스트 `scope.rs:628-690` |
| App·Scope 라우터는 서비스를 등록 순서대로 시도하고 첫 매치가 이긴다(정렬·병합 없음). 리소스·스코프 수준 가드가 실패하면 다음으로 넘어간다 | `src/app_service.rs:83-85`, `:302-311`, `:332-335`; actix-router `src/router.rs:54-58`; 문서 `app.rs:213-214` |
| 리소스 안에서 맞는 라우트가 없으면 그 리소스의 기본(405)으로 끝난다 — 다음 리소스로 넘어가지 않는다. 스코프도 안에서 맞는 것이 없으면 스코프·App 기본으로 끝난다 | `src/resource.rs:562-570`, `:75-89`; `src/scope.rs:530-549`, 테스트 `:907-927` |
| `App::route`·`Scope::route`·`ServiceConfig::route`는 라우트 가드를 모두 **리소스 수준**으로 옮긴다(method 불일치는 다음 리소스로 → 404) | `src/app.rs:227-233`, `src/scope.rs:260-266`, `src/config.rs:385-391`, `src/route.rs:79-81`, 테스트 `scope.rs:694-748` |
| `#[get("/p")]` 등 매크로는 `Resource::new(p).guard(Get()).to(h)` — method 가드가 리소스 수준. `#[route(.., method=..)]`는 가드 하나에 동사 여럿, `#[routes]`는 속성마다 리소스 하나, `guard = ".."`는 리소스 가드 | actix-web-codegen 4.4.0 `src/route.rs:456-463`, `:148-197`, `:445-453`, `:509-550`, `:260-272` |
| `web::get()`…`web::trace()`는 method 가드, `web::route()`·`web::to(h)`·`Resource::to`는 모든 method, `Resource::get(h)` 등은 라우트 수준 가드, `web::redirect`는 모든 method | `src/web.rs:98-100`, `:123-129`, `:144-146`, `:162-169`; `src/resource.rs:241-249`, `:369-406`; `src/redirect.rs:150-160` |
| method 가드는 정확히 같은 동사만 받는다 — `web::get()`은 HEAD를 받지 않는다 | `src/guard/mod.rs:412`, 테스트 `src/route.rs:393-397` |
| `configure`는 호출 위치에 서비스를 순서대로 끼워 넣는다 | `src/app.rs:191-208`, `src/config.rs:374-380`, `src/scope.rs:186-206` |
| `NormalizePath::trim()`·`default()`는 Trim(`//+` 병합 + 끝 슬래시 제거, 루트는 `/`), `Always`는 끝에 `/`, `MergeOnly`는 병합만. `App::wrap`이면 라우팅 전에 동작한다 | `src/middleware/normalize.rs:44-50`, `:112-134`, `:151`, `:174-228`; `src/app.rs:367-368` |
| 정규화 미들웨어가 없으면 `/items`와 `/items/`는 서로 맞지 않는다 | actix-router `resource.rs` 문서 `:196-208` |

### 사실로 바꾸는 방법

- **dispatch**: `registration-order`. `App::new()` 체인마다 group 하나(`actix:<App을 만드는 함수 id>`, 한 함수에 둘 이상이면
  `#n`), 리소스 하나(매크로 속성 하나, `App::route` 하나, `web::resource` 하나)가 index 하나다. 한 리소스의 라우트·경로
  여럿(`web::resource(["/a","/b"])`)은 같은 index·같은 위치다. 스코프·`configure` 안의 리소스는 깊이 우선 순서로 번호를
  이어 받는다.
- **method**: 리소스 수준·라우트 수준 가드의 교집합. method가 아닌 가드(`Header`·`Host`·`fn_guard`·매크로 `guard =`)는
  `narrowed: true`다(조건이 맞지 않는 요청은 다음 등록으로 넘어간다 — 계약의 narrowed 뜻과 같다).
- **channel**: 스코프 접두사와 리소스 경로를 actix 규칙으로 이은 뒤 패턴을 해석한다. `{name}`은 `{}`(부분 세그먼트면
  `p{}s`), 한 세그먼트에 파라미터 둘 이상은 dynamic과 `route-coverage:`. `{tail}*`·끝의 `{x:.*}`는 `{**}`와 빈 값 변형
  (`/files/`), `{x:.+}`는 `{**}`만. 그 밖에 `/`와 맞을 수 있는 정규식은 dynamic이다.
- **paramConstraints**: 정규식을 실행하지 않고 흔한 모양(`X+`·`X{n,m}`과 그 연결)만 읽어 ASCII 문자 집합으로 판정한다
  — 숫자면 `int`, `[-A-Za-z0-9_]` 부분집합이면 `slug`, hex 8-4-4-4-12면 `uuid`, `/`를 뺀 전부면 제약 없음, 그 밖은
  `regex`(원문 `pattern`). 빈 값과 맞는 정규식(`\d*`)과 읽지 못한 정규식(그룹·대안·플래그)은 dynamic이다.
- **trailingSlash**: `strict`. App(또는 스코프)의 `NormalizePath`가 Trim이면 끝 슬래시 없는 템플릿이 `optional`이고
  빈 값 변형은 내지 않는다(그 요청은 트림돼 꼬리 파라미터에 닿지 않는다). Always면 끝 슬래시 있는 템플릿이 `optional`.
  `{**}`로 끝나면 생략.
- **App 평가**: `App::new()`에서 시작하는 체인마다, 그리고 함수 본문 최상위의 `let app = App::new()…;`와 그 재대입
  (`app = app.route(..)`)을 따라간다. 함수 인자로 넘긴 App은 그 함수가 더 등록할 수 있어 `route-coverage:`로 센다.
- **location**: 매크로 속성의 경로 리터럴, `App::route`·`web::resource`의 경로 인자.
- 어느 App에도 등록되지 않은 매크로 핸들러와 리터럴이 아닌 스코프 아래 선언은 `pathAnchor: "base"`(order 없음)와
  `unresolved-route-prefix:`·`route-dispatch-order-unknown:`(`templateSuffixes` 스코프)이다.

## 한계와 스코프

| 상황 | 결과 |
|---|---|
| 워크스페이스에 axum·actix-web 의존이 없음 | 사실 0건 + `route-coverage:` |
| 확인한 메이저 밖의 버전 | `route-framework-version-unknown:` |
| 리터럴·`&str` 상수가 아닌 경로 | dynamic 사실(원문) + `route-coverage:` |
| 평가하지 못한 nest·merge·서비스·설정 함수 | 그 자리 접두사가 리터럴이면 `templatePrefixes` 스코프의 `route-coverage:`, 아니면 스코프 없음 |
| 조건·반복 안의 actix `ServiceConfig` 등록 | `route-coverage:` |
| impl·trait 메서드 안의 `Router::new()`·`App::new()` | `route-coverage:`(추출기는 모듈 수준 함수만 평가한다) |
| 함수 인자로 넘긴 actix App | `route-coverage:` |
| axum Router fallback·actix App 기본 서비스 | 루트면 스코프 없는 `route-coverage:`, nest·스코프 아래면 그 접두사 스코프 |
| axum `nest_service` | `framework-provided-routes:` + 접두사 스코프 |
| actix-files `Files::new(prefix, ..)` | `framework-provided-routes:` + 접두사 스코프, `methods: ["GET","HEAD"]` |
| 기동 시 panic하는 axum 경로 | 스코프 없는 `route-coverage:`(선언 없음) |
| 원문 경로로만 닿는 axum 리터럴 | 그 템플릿 스코프의 `route-coverage:`(선언 없음) |
| CONNECT·사용자 동사 | `route-coverage:` |
| 클로저 핸들러 | 감싸는 함수 usr + `missing-route-usrs:`(체인 전용) |
| 서비스·리다이렉트·해석 못 한 핸들러 | usr 없음 + `missing-route-usrs:` |

스코프는 문서를 내기 전에 계약 검사(`scope_problem`, 공유 벡터 `scope.validate`)를 통과해야 하고, 통과하지 못하면
스코프 없이 남긴다. 템플릿 문법과 `order` 규칙(`dispatch.validate`)도 내기 전에 소비자와 같은 검사로 확인하며,
실패는 생산자 결함이라 문서를 내지 않고 오류로 보고한다.

## 결정 사항

- **axum은 specificity, actix-web은 registration-order.** 위 소스로 확인했다. 두 근사의 실패 방향은 모두 거짓 match다.
  - axum: 경로 우선(405) vs isthmus method 우선.
  - actix: 라우트 수준 method 가드(`web::resource().route(web::get())`)는 405로 끝나지만 isthmus는 method를 먼저 거른다.
    스코프가 접두사를 잡으면 뒤 App 서비스로 넘어가지 않는데(`/api` 스코프 뒤의 `/api/x` 리소스는 닿지 않음) 계약에는
    스코프가 없어 소비자가 뒤 리소스에 잇는다. 매크로·`App::route`의 리소스 수준 method 가드는 isthmus의 method 우선과
    정확히 같지만, 소비자의 `route-decl-path-shadowed` 경고는 method가 달라 다음 리소스로 넘어가는 경우에도 날 수 있다.
  - actix `web::get()`은 HEAD를 받지 않지만 소비자 `head-as-get`은 잇는다.
- **정규화 미들웨어가 닿지 않게 만든 선언도 선언이다.** Trim 아래 끝 슬래시 있는 리소스, Always 아래 끝 슬래시 없는
  리소스는 요청이 닿지 않지만 `strict`로 낸다. 미들웨어 판정이 틀렸을 때 선언을 빼면 거짓 error가 되기 때문이다.
- **빈 값 변형은 실측으로만.** matchit의 중간 파라미터 빈 값 매칭은 문서에 없어 오라클로 확인한 뒤 계약의 "빈 값 변형"
  규칙으로 낸다. actix `{name}`(`[^/]+`)은 빈 값과 맞지 않아 펼치지 않는다.
- **HEAD·자동 OPTIONS는 내지 않는다**(pythograph·tsograph와 같다). 명시한 `head`·`options`는 낸다.
- **테스트 소스는 제외한다**(`sourceSets.tests: "excluded"`) — `#[cfg(test)]` 모듈과 `#[test]`·`#[tokio::test]`·
  `#[actix_web::test]` 함수. tests/·examples/ 타깃은 수확 범위 밖이다.

## 오라클 검증

`experiments/routes-oracle/`는 fixture의 **같은 lib.rs**를 crates.io의 진짜 axum 0.8.9·0.7.9·actix-web 4.15.0으로 컴파일해
(패키지의 `[lib] path`가 fixture 소스를 가리킨다) 프로세스 안에서 요청을 보낸다(axum `tower::ServiceExt::oneshot`, actix
`test::init_service`·`call_service`). 핸들러는 자기 정점 ID를 본문(HEAD는 `x-handler` 헤더)으로 돌려준다. 판정 기준은
"rustograph 사실만으로 isthmus 방식(세그먼트·method·끝 슬래시·제약, 구체성 또는 등록 순서)으로 고른 핸들러 = 프레임워크가
실제로 디스패치한 핸들러"다.

- 사실 요청(정밀도): 사실마다 제약을 만족하는 표본 경로(ANY는 GET·POST·DELETE)를 보낸다.
- 끝 슬래시 요청: `trailingSlash`가 있는 사실마다 슬래시를 뒤집은 경로를 보내 `strict`·`optional` 판정을 확인한다.
- 기대 요청(재현율): 사람이 적은 (method, 경로, 핸들러) — 구체성(`/items/special` vs `/items/{}`), 등록 순서(정수 id가
  슬러그보다 먼저), 빈 값 변형, catch-all, HEAD, 설정 함수 등.
- 음성 요청: 어느 핸들러에도 닿지 않아야 하는 요청(404·405·fallback) — 사실도 아무것도 주장하지 않아야 한다.
  actix는 가드 헤더 없는 요청이 헤더 가드 스코프에 닿지 않는지도 본다.

| fixture | 정밀도 | 재현율 | 음성 | 끝 슬래시 | 요청 불가 |
|---|---|---|---|---|---|
| `tests/fixture-routes/axum08` (axum 0.8.9) | 24/24 | 23/23 | 12/12 | 22/22 | 1 (루트에 붙지 않은 base 라우터) |
| `tests/fixture-routes/axum07` (axum 0.7.9) | 14/14 | 11/11 | 6/6 | 13/13 | 0 |
| `tests/fixture-routes/actix` (actix-web 4.15.0) | 23/23 | 18/18 | 9/9 | 19/19 | 1 (등록되지 않은 매크로 핸들러) |

오라클이 처음 잡아 규칙을 고친 것: matchit의 중간 파라미터 빈 값 매칭(빈 값 변형 추가), 0.8 앞 글자 붙은 파라미터의
빈 값 매칭, axum이 원문 경로를 비교해 리터럴 중괄호 경로에 인코딩된 요청이 닿지 않는 것(선언 대신 스코프 한계),
actix Trim 아래 빈 꼬리 변형이 닿지 않는 것, 같은 템플릿의 정적 경로가 빈 값 변형을 가리는 것(GLM 리뷰 지적을 오라클로
재현). 오라클의 HEAD→GET 예측은 axum(specificity)에서만 쓴다(actix `web::get()`은 HEAD를 받지 않는다).

기록(`experiments/routes-oracle/recorded/*.json`)은 커밋하고 `tests/routes.rs`가 오프라인으로 지금 출력의 정적 사실이
기록에서 검증된 사실(과 요청할 수 없는 base 사실)과 정확히 같은지 확인한다. 다시 기록하려면
`experiments/routes-oracle/run_all.sh`(crates.io 내려받기만 네트워크를 쓴다).

## isthmus 호환

isthmus `main`(`76b6141`, #131)은 `platform: "rust"`의 http `route-decl`을 specificity·registration-order 모두 받는다.
공유 벡터는 같은 커밋에서 벤더링했다(`conformance/`, `conformance.lock`).
