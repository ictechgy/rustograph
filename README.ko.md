# rustograph

<img src="icon.png" alt="rustograph의 새 마스코트" width="112" height="112" align="right">

Rust 의존성 그래프 도구 — Cargo 워크스페이스를 읽어 의존성 그래프를 만들고,
그 위에서 순환·도달성·영향 범위·레이어 규칙을 질의합니다.

설계는 한 문장입니다: **그래프가 산출물이고, 나머지는 전부 그 위의 질의입니다.**

> 정본은 [README.md](README.md)(영어)입니다. 이 문서는 한국어 참조입니다.

자매 프로젝트: [cartograph](https://github.com/ictechgy/cartograph)(Swift) ·
[gartograph](https://github.com/ictechgy/gartograph)(Go) ·
kartograph(Kotlin/Android) · dartograph(Dart/Flutter) ·
[schemagraph](https://github.com/ictechgy/schemagraph)(데이터베이스) ·
isthmus(언어 경계 조인).

## 왜

Rust에는 `cargo-modules`, `cargo deps`, 그리고 syn/tree-sitter 기반 콜그래프
도구들이 이미 있지만 — 모듈 레벨에서 멈추거나, 이름 추측 간선을 추정이라고
밝히지 않습니다. rustograph는 네 레벨(crate/module/type/symbol)의 **하나의
버전 관리된 그래프**를 만들고 모든 분석을 그 위의 질의로 표현합니다.
결정적 JSON 계약의 독자는 코딩 에이전트입니다.

- **추정 간선은 숨기지 않고 표시합니다.** `x.m()` 메서드 호출은 구문만으로
  타입을 알 수 없습니다 — `tentative: true`로 표시된 팬아웃 간선을 만들어
  도달성에는 쓰고("살아 있다" 편향), 순환·규칙 증거에서는 제외 수와 함께
  제외합니다.
- **limitation은 상용구가 아니라 실측입니다.** 미해석 경로·매크로 본문·
  `#[cfg]` 아이템·orphan 파일을 프로젝트별로 세어 모든 응답에 싣습니다.
- **삭제 판정을 내지 않습니다.** `dead`는 그래프 사실 `unreachable`만
  보고합니다 — "지워도 된다"가 아닙니다.

## 설치

```bash
brew install ictechgy/tap/rustograph
# 또는
cargo install --git https://github.com/ictechgy/rustograph --tag v0.3.0
```

타입 해석 분석은 opt-in `semantic` feature 빌드가 필요합니다
(rust-analyzer `ra_ap_*` — 의존이 커서 기본이 아닙니다):

```bash
cargo build --release --features semantic
rustograph graph --level symbol --semantic
```

## 사용

```bash
rustograph graph --level crate --deps    # 크레이트 + 의존 그래프
rustograph graph --level symbol          # call/references/implements/signature
rustograph graph --out .rustograph/graph.json
rustograph cycles --graph .rustograph/graph.json --strict
rustograph dead --retain-public --tests
rustograph dead --explain mycrate::f     # 왜 살아 있나 — 도달 경로
rustograph rules --strict                # .rustograph.yml 레이어 규칙
rustograph rules --format sarif          # GitHub 코드 스캐닝용
rustograph rules --write-baseline        # 기존 위반을 기준선으로 얼림
rustograph query mycrate::module::f --depth 2
rustograph impact mycrate::Type --depth 3
rustograph paths mycrate::a mycrate::b   # 두 정점 사이 제한된 경로 탐색
rustograph search entry                  # 정확 > 꼬리 > 부분 문자열 순위
rustograph deps                          # 미사용 의존 + 중복 버전 보고
rustograph deps --strict                 # 발견이 있으면 종료 코드 1
rustograph graph --focus mycrate::sub    # 서브트리만 남김
rustograph dead --exclude-tests          # #[cfg(test)] 서브트리 제외
rustograph graph --target x86_64-pc-windows-msvc  # cfg(트리플) 평가
rustograph mcp                           # MCP stdio 서버 — 에이전트가 되묻는 통로
rustograph schema --dir . --out schema-facts.json  # isthmus persistence 사실
rustograph routes --role server --out routes.json  # isthmus http route-decl(axum·actix-web)
rustograph routes --role client --wrappers http-wrappers.json --out calls.json  # route-call(reqwest·ureq)
rustograph reach mycrate::api::list_users          # isthmus language-traversal(정방향)
rustograph impact --format language-traversal --roots-from schema-facts.json  # 역방향
```

`#[cfg]` 조건은 메타데이터로 그래프에 실립니다 — 정점의 `cfg`는 자기
`#[cfg(...)]` 토큰, 간선의 `cfg`는 그 조건 아래서만 성립하는 의존을
표시합니다. `unsafe`는 경계를 표시합니다 — `unsafe fn`/`unsafe trait`이거나
`unsafe {}` 블록을 품은 정점에 `unsafe: true`, `unsafe {}` 안에서 만든
간선은 경계 진입 간선입니다.

`rustograph mcp`는 개행 구분 JSON-RPC 2.0을 stdio로 말하고 도구 9종
(`rustograph_summary`/`_query`/`_impact`/`_paths`/`_search`/`_cycles`/
`_dead`/`_rules`/`_deps`)을 서빙합니다. 문서는 기동 시 한 번 수확해
(또는 `--graph`로 읽어) 같은 스냅샷 위에서 답합니다. 부분 ID는 후보
목록과 함께 거절됩니다 — `rustograph_search`로 정확한 ID를 찾으세요.

`--semantic` 문서는 `.rustograph/semantic-cache.json`에 캐시됩니다 —
워크스페이스 소스와 매니페스트 지문이 키라, 오래되거나 깨진 캐시는
조용히 새 수확으로 돌아갑니다. `--no-cache`로 끌 수 있습니다.

종료 코드: `0` 정상 · `1` strict 위반/발견 · `2` 사용법/분석 오류.
순회 명령(`reach`, `impact --format language-traversal`)은 isthmus 계열
계약을 따릅니다 — 사용법 오류는 표준 출력을 비운 채 `64`, 그래프 정점이
아닌 root가 섞이면 문서를 쓴 뒤 `64`입니다.

## persistence 사실 — `schema`

`rustograph schema`는 코드가 SQL 관계를 어떻게 참조하는지 담은 isthmus
`bridge-facts` v1 문서(`platform: "rust"`, `target: "persistence"`)를
냅니다 — isthmus가 `schemagraph facts` 출력과 조인해 미선언·미사용
스키마 객체와 컬럼 드리프트를 보고합니다. 읽어내는 참조:

- 어디에든 있는 SQL 형태의 문자열 리터럴(`format!` 계열 매크로 안
  포함) — `FROM`/`JOIN`/`INTO`/`UPDATE`/`TABLE`/`TRUNCATE` 뒤의 관계,
  `schema.table` 한정과 인용 식별자 보존
- `sqlx::query*` 매크로·함수(`query!`, `query_as!`, `query_scalar!` …)
  — 리터럴은 스캔하고 비리터럴 인자는 `dynamic` 사실로 보존해
  isthmus가 공백을 셀 수 있게 합니다
- `sqlx::query*_file!` — SQL이 파일에 있으므로 `dynamic`으로 보고
- `diesel::table!` 매크로 본문 — 관계 + 컬럼 사용
- `#[diesel(table_name = …)]`·`#[sea_orm(table_name = "…")]` 구조체와
  필드/`column_name`/`sqlx::rename` 컬럼
- diesel DSL 경로 — `users::table`, `users::dsl::id`,
  `users::columns::name` — 워크스페이스에 선언된 `table!` 이름과
  맞물릴 때만 정적으로 인정하고, 안 맞는 같은 모양 경로는 `dynamic`

사실마다 `symbol: {qualifiedName, usr}`을 싣고 `usr`는 감싸는 그래프
정점 ID입니다 — `impact`/`reach`와 같은 ID라 isthmus `trace`가 핸들러
도달과 관계 사용을 잇습니다. fn·impl 메서드(`Type::method`, 트레이트 impl은
`Type::<Trait>::method`)·트레이트 기본 메서드·const/static 초기화식은 그
정점, 구조체 어트리뷰트·필드 컬럼은 구조체, 메서드 밖 연관 상수는 impl의
self 타입입니다. 감싸는 정점이 없는 사실(최상위 `table!` 호출, 모듈 트리
밖 파일)은 `symbol` 없이 내고 `missing-relation-usrs:`로 셉니다. ID는
`impact`와 같은 syn 수확에서 받아 오고(다시 유도하지 않음) 그래프 정점
집합으로 확인합니다.

비한정 이름(`query!`, `sql_query`, `table!`)은 그 파일이 sqlx/diesel에서
import할 때만 인정합니다. 파싱 실패 파일·문법이 다른 `table!`·테이블
바인딩 없는
컬럼 어트리뷰트는 조용히 넘기지 않고 `limitations`로 셉니다. 이름 기반
스캔은 추측하지 않습니다 — 정적으로 해석할 수 없는 것은 지어내지 않고
센 것입니다.

## 서버 라우트 — `routes --role server`

`rustograph routes --role server`는 axum 0.7·0.8과 actix-web 4 서버가 선언한
(method, 정규 경로 템플릿)마다 `route-decl` 사실 하나를 담은 isthmus
`bridge-facts` v1 문서(`platform: "rust"`, `target: "http"`)를 냅니다.
핸들러 `symbol.usr`가 `reach`의 정점 ID와 같아서 `isthmus trace`가 route →
핸들러 → relation-use(`schema`) → 테이블(`schemagraph facts`)로 잇습니다.

- **axum**(`dispatch: "specificity"`, matchit의 정적 > 파라미터 > catch-all):
  `Router::new().route(..)` 체인, 메서드 라우터(`get`/`post`/…/`any`/
  `on(MethodFilter)`), `nest`(axum `path_for_nested_route`와 같은 결합),
  `merge`, 지역 `let`·재대입, 라우터를 돌려주는 크레이트 함수. 경로 문법은
  해석된 axum 버전을 따릅니다 — 0.7은 `:id`/`*rest`, 0.8은 `{id}`/`{*rest}`/`{{`.
- **actix-web**(`dispatch: "registration-order"`, App마다 `order.group`,
  리소스마다 `order.index`): `#[get("/x/{id}")]` 매크로,
  `web::resource().route(web::get().to(h))`, `web::scope`, `App::route`,
  `configure`, 가드(`narrowed`), `{id:\d+}` → `paramConstraints`, `{tail}*`
  catch-all, `NormalizePath` 미들웨어의 `trailingSlash` 효과.
- 정적으로 확정하지 못한 것(리터럴이 아닌 경로, 모르는 함수가 만든 라우터,
  fallback, tower 서비스)은 dynamic 사실이나 스코프 있는
  `route-coverage:`·`framework-provided-routes:` 한계가 됩니다 — 추측한
  라우트를 만들지 않습니다.

규칙마다 근거가 된 axum·matchit·actix-web 소스 줄은
[docs/HTTP-ROUTES.md](docs/HTTP-ROUTES.md)에 있습니다. 오라클
(`experiments/routes-oracle/`)이 같은 fixture 소스를 진짜 크레이트로 컴파일해
프로세스 안에서 요청을 보내 세 fixture 모두 정밀도·재현율 100%를 확인했고,
그 기록을 `cargo test`가 오프라인으로 대조합니다. isthmus 공유 벡터는
`conformance/`에 잠금 파일과 함께 벤더링했습니다.

## 클라이언트 호출 — `routes --role client`

`rustograph routes --role client`는 코드가 만드는 HTTP 요청마다 `route-call`
사실 하나를 담은 isthmus `bridge-facts` v1 문서(`platform: "rust"`,
`target: "http"`, `roles: ["client"]`)를 냅니다. `symbol.usr`는 호출을 감싼
함수·메서드이고 `impact`의 정점 ID와 같아서 `isthmus trace`가 호출부에서 그
호출부에 기대는 클라이언트 코드로 이어 갑니다.

- **reqwest**(0.13, 0.12도 확인): `reqwest::get`·`blocking::get`,
  `Client`·`blocking::Client`의 동사 메서드, `request(Method::X, url)`,
  `Request::new`. 문자열은 `url::Url::parse`(WHATWG)로 해석합니다 — 점
  세그먼트는 지우고 `//`는 남깁니다.
- **ureq** 3(2.x는 소스 기준): 자유 함수와 `Agent` 메서드. ureq 3은
  `http::Uri`로 해석해 점 세그먼트를 남깁니다.
- **URL 조립**: 리터럴, `format!`(위치·이름·인라인 인자), `concat!`, `+`,
  상수·static·연관 상수, 지역 변수(그림자 추적, 수정되는 이름은 믿지 않음),
  `Url::parse(..)?.join(..)`(RFC 3986 병합 — `…/v2/catalog` + `tags`는
  `/v2/tags`), 모든 생성자가 같은 리터럴·상수로 채우는 구조체 필드 base.
  그 밖은 dynamic 사실(`channel: null`, 증명한 경우 마스킹한 `channelPrefix`)
  이나 센 한계입니다.
- **래퍼**: isthmus `http-wrappers` v1 파일에 선언한 함수·메서드·구조체
  리터럴 엔드포인트(`"language": "rust"`, `owner::name`이 rustograph 정점
  ID)는 선언한 동사·앵커로 호출 사실이 됩니다.
- **센 공백**: 모델링하지 않는 클라이언트(hyper client·surf·awc·isahc 등),
  클라이언트로 증명하지 못한 수신자의 요청, 클라이언트가 거부하는 상대
  URL, 선언되지 않은 래퍼 싱크, 풀리지 않는 선언은 `route-call-coverage:`·
  `ambiguous-base-join:`·`http-wrapper-undeclared:`·`http-wrapper-unresolved:`
  한계입니다.

규칙, 제안한 결합 방식 이름(`whatwg-concat`·`whatwg-join`·`http-uri-concat`),
오라클 표는 [docs/HTTP-CLIENT.md](docs/HTTP-CLIENT.md)에 있습니다. 모의 서버
오라클(`experiments/client-oracle/`)이 fixture를 진짜 reqwest·ureq·url로
컴파일해 로컬 서버가 받은 요청을 기록합니다 — 41개 시나리오 불일치 0, 기록은
`cargo test`가 오프라인으로 대조합니다. isthmus `url-compose` 벡터의 생산자
사례 41건을 통과합니다.

## 순회 문서 — `reach` / `impact --format language-traversal`

isthmus [`language-traversal` v1](https://github.com/ictechgy/isthmus/blob/main/docs/LANGUAGE-TRAVERSAL.md)
문서를 냅니다. `reach`는 root가 기대는 쪽(`dependencies`), `impact`는 root에
기대는 쪽(`dependents`)입니다.

- root는 위치 인자 다음 `--roots-from`(JSON 문자열 배열 또는 bridge-facts
  문서의 `symbol.usr`, `-`는 표준 입력) 순서로, 처음 나온 자리에만 남깁니다.
  빈 id·제어 문자 id·10,000개 초과는 사용법 오류(64, 표준 출력 비움)입니다.
- 모든 root를 한 번에 훑어 정점마다 닿는 root 목록(64개까지 + `rootsTruncated`),
  가장 가까운 깊이, 최단 경로 목격(`via`)을 싣습니다. root별 BFS 오라클과
  무작위 그래프로 대조합니다.
- 근거 등급: `tentative` 간선(이름 팬아웃·`dyn`/제네릭 트레이트 impl 후보)은
  `candidate`, 나머지는 `direct`이고 정점마다 root별 하한을 싣습니다.
  미해석 호출 수가 완전하다고 말할 수 없어 `dispatch`·`unresolvedCalls`는
  싣지 않습니다.
- `--max-depth` 1~128(기본 128, `--depth 0`은 128), `--max-reached` 1~100,000.
- `project`는 `schema`와 같은 realpath, `revision`은 `--revision` 또는 작업
  트리가 깨끗할 때의 git HEAD, `graphRevision`은 그래프 JSON의 SHA-256입니다.
- 그래프 정점이 아닌 root는 `symbol` 없이 싣고 `root-not-found:` limitation을
  더한 문서를 쓴 뒤 64로 끝납니다.

isthmus는 Rust `route-decl` 문서를 받아 axum·actix-web 핸들러에서 route 선택
`trace`를 시작합니다. Rust `route-call`은 아직 받지 않습니다 — 그 변경이
들어오기 전까지 `routes --role client` 문서는 그것을 허용한 isthmus 빌드로
검증했습니다(HANDOFF.md).

## 개발

```bash
cargo test                      # 단위 + fixture 통합 테스트
scripts/coverage.sh             # 커버리지 게이트 90%
scripts/verify-cli-contract.sh  # 실제 바이너리 종료 코드 계약
cargo run -- rules --strict     # 자기 분석(도그푸딩)
```

## MVP의 한계

구문 분석(syn)은 매크로 확장·`dyn` 디스패치·제네릭을 꿰뚫지 못합니다 —
모든 사각지대는 `limitations`에 실측으로 잡힙니다. opt-in `semantic`
feature(`--features semantic` 빌드 후 `--semantic`)는 rust-analyzer
(`ra_ap_*`) 의미론으로 본문 수확을 보강합니다 — 메서드 호출은 수신자
타입으로 해석하고, 매크로 확장 트리를 걷고, `dyn`/제네릭 트레이트 호출은
워크스페이스 impl 후보로 펼칩니다(실제 impl은 런타임 사실이라 여전히
추정 간선). 그래프 계약은 그대로이고 정확도만 올라갑니다 — 의미 해석이
보지 못하는 본문(cfg 비활성·매크로 생성)은 syn 경로로 되돌아가고 그 수를
셉니다.

## 라이선스

MIT — [LICENSE](LICENSE).
