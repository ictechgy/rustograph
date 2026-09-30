# isthmus 공유 적합성 벡터

isthmus(`3a4545088e8eaf1095b94765a407a9a0b5d823f2`)의 `conformance/`를 그대로 가져온 사본이다. 정본은 isthmus가
소유하며, 이 디렉터리 파일을 직접 고치지 않는다. 갱신할 때는 isthmus main의 파일과 `SHA256SUMS`를 함께 다시
복사하고(새 suite 파일 포함) 저장소 루트의 `conformance.lock`에 커밋과 파일별 sha256을 적은 뒤
`cargo test --test routes`로 확인한다.

rustograph가 실행하는 사례(생산자 대상, `rustograph routes --role server|client`):

| suite | ruleId | 검사 |
|---|---|---|
| `http-template` | `template.grammar` | 정규 문법 검사기(`source::routes::template::template_problem`)가 소비자와 같은 판정·거부 사유를 낸다 |
| `http-template` | `template.normalize` | URI 경로 정규화(`normalize_uri_path`)가 같다 |
| `http-dispatch` | `dispatch.validate` | `order` 검증기(`source::routes::validate::order_problem`)가 소비자와 같이 판정한다. fixture 출력에도 적용한다 |
| `http-limitation-scope` | `scope.validate` | 스코프 검증기(`scope_problem`)가 소비자와 같이 판정한다(생산 문서가 거부되지 않게) |
| `url-compose` | `compose.*`·`wrapper.*`(`producer` 41건 + `producer:rustograph` 7건) | 클라이언트 조립(`source::routes::compose`)과 래퍼 동사 바인딩(`source::routes::wrappers`)이 같은 결과를 낸다(`tests/client_routes.rs`). `dio-concat`은 결과가 같은 WHATWG 문자열 연결로, `wrapper.location`은 실제 스캐너로 확인한다 |

건너뛰는 사례와 이유: 소비자 전용 사례(`match.*`, `dispatch.match`, `dispatch.shadow`, `scope.applies` — 적용 판정은
소비자가 한다), 다른 생산자의 프레임워크 변환(`framework.openapi.*`, `framework.spring.*`), `url-compose`의 다른 생산자
전용 사례(`producer:kartograph`·`producer:gartograph`·`producer:pythograph`), `scope.dynamic-validate`·
`scope.dynamic-applies`(dynamic 선언의 선택 필드 `dynamicScope` — rustograph는 내지 않는다).
