# HANDOFF.md

세션을 이어받는 에이전트가 먼저 읽는 문서입니다.

## 현재 상태 (2026-09-24)

**feature/schema-facts 진행 중 — PR #15.** isthmus persistence 도메인의
두 번째 코드 생산자로 `rustograph schema`를 추가했다: bridge-facts v1,
`platform: "rust"` + `target: "persistence"`(사실 없으면 `target: null`),
SQL 문자열·sqlx 매크로/함수·diesel `table!`·DSL 경로·sea_orm 어트리뷰트에서
relation-use 사실 수확. GLM 리뷰 3라운드 반영 — 산문 오탐 게이트,
별칭·서브쿼리·`;` 다중 문장·GRANT/REVOKE 객체 종류어, 미해석·unlocated
계수, DSL 경로는 선언된 table! 이름으로만 정적화. isthmus 쪽 계약 확장은
PR #110(rust 플랫폼 + relation-use 허용, bridge 구성과 격리).

**v0.2.1 배포 완료.** https://github.com/ictechgy/rustograph (public),
`brew install ictechgy/tap/rustograph`로 설치 가능(brew test 통과).
v0.2.0도 배포됐으나 자기 분석에서 cli↔mcp 모듈 순환이 잡혀
인자 파서를 cli_args로 분리한 0.2.1이 최신이다. PR #1~#6 머지됨.

검증 상태: `cargo test` 63개 통과(단위 40 + 통합 23), 커버리지 91.2%
(게이트 90), clippy 클린, verify-cli-contract OK(mcp 포함),
자기 분석 `rules --strict` 0 위반 / `cycles --strict` 0.
`--features semantic` 빌드에서는 +47 semantic 테스트, 자기 분석
`rules`/`cycles` 동일 0(semantic 모드 366 타입 해석 간선).
PR #8(의미 해석) 머지됨 — 70ea82e. PR #10(의미 하드닝) 머지됨 —
011c05b.

## 구조

- `src/graph.rs` — 순수 도메인(Document/Vertex/Edge/Level), 결정적 정렬,
  `Edge.tentative` + 투영 병합. `Vertex.cfg`/`unsafe` + `Edge.cfg`/`unsafe`
  메타데이터. 외부 의존 0(serde만).
- `src/cargo_meta.rs` — `cargo metadata` → 패키지/타깃/depends 간선.
- `src/modtree.rs` — `mod` 선언만으로 모듈 트리(`x.rs`|`x/mod.rs`|
  `#[path]`|인라인), orphan .rs 계수, 2단계 스코프(fill_items→fill_imports,
  글롭 확장 포함), 경로 해석.
- `src/harvest.rs` — syn 방문자. 아이템→정점, impl→메서드+implements,
  본문→call/references, 시그니처→signature, 매크로 인자·포맷 캡처,
  `#[cfg]` 추출(cfg_of), unsafe 감지(unsafety/unsafe 블록/unsafe impl).
- `src/source.rs` — 오케스트레이터. AST arena('static 누수), 루트 병합
  (lib/bin 같은 이름 → extra_files), 보존 루트(main/#[no_mangle]/
  --tests/--retain-public).
- `src/analysis.rs` — Tarjan SCC(tentative 제외), dead(BFS+explain),
  query/impact(전이 클로저).
- `src/rules.rs` — allowlist+deny+signature 규칙, unmapped 보고,
  skipped_tentative 계수.
- `src/export.rs` — 결정적 JSON + mermaid + save/load.
- `src/sarif.rs` — SARIF 2.1.0(`rustograph/deny` 등 ruleId).
- `src/config.rs` — `.rustograph.yml` 파싱(serde_yml 격리).
- `src/cli.rs` — graph/cycles/dead/rules/query/impact/mcp/version,
  종료 코드 0/1/2.
- `src/cli_args.rs` — cli/mcp 공유 인자 파서(최하층, 순환 방지).
  이름이 args가 아닌 이유는 파일 헤더 주석 참고 — 지역 변수 `args`가
  이름 해석으로 모듈을 가리키는 가짜 참조를 피한다.
- `src/mcp.rs` — MCP stdio 서버(NDJSON JSON-RPC 2.0). 기동 시 문서 1회
  수확 후 스냅샷 서빙. 도구: rustograph_summary/query/impact/cycles/
  dead/rules. stdin을 파라미터로 받아 테스트 가능.
- `src/sem.rs` — ra_ap_* 의미 해석 엔진(`semantic` feature, opt-in).
  `Engine::load`(load_workspace_at+attach_db), 정규 ID↔hir 인덱스,
  본문 워커(메서드 타입 해석·경로 해석·매크로 확장·trait impl 행렬),
  Stats 실측. 정점·구조는 syn이 권위 — sem은 본문 간선만 낸다.
  hir이 모르는 본문(cfg 비활성·매크로 생성)은 syn 폴백으로 돌아간다.
- `tests/fixture/` — 세 멤버 워크스페이스(fixture_macros proc 매크로
  포함), 모든 아이템 종류+글롭/별칭/cfg/generated/orphan/build.rs
  OUT_DIR 생성 모듈 커버.
- `scripts/` — coverage.sh(llvm-cov), verify-cli-contract.sh.
- `.rustograph.yml` — 자기 계층 규칙(graph 순수 도메인 강제).

## 알려진 의미론 한계 (설계상, limitation으로 계측됨)

- 메서드 호출 `x.m()`은 이름 팬아웃(tentative) — 타입 정보 없음.
- `dyn`/제네릭/외부 크레이트 디스패치 불가시.
- 매크로는 쉼표 표현식 인자와 `{ident}` 캡처만 — 독자 문법은 미가시.
- `#[cfg]`는 평가 없이 포함+계수.
- 병합 루트에서 같은 이름의 인라인 mod가 양쪽 파일에 있으면 한쪽만 수확.

## 다음 단계 (우선순위 순)

1. ~~공개 리포 + 릴리스~~ — 완료. `HOMEBREW_TAP_TOKEN` 리포 시크릿이
   없어 탭 갱신은 수동으로 했다 — 넣으면 다음 릴리스부터 자동.
2. ~~MCP 서버~~ — 완료(v0.2.0).
3. ~~feature/cfg 의존 모델링~~ — 완료: 정점·간선 `cfg` 필드, 여러 cfg는
   `all(...)` 합성, cfg 다른 같은 간선은 별개로 유지.
4. ~~unsafe 경계~~ — 완료: 정점 `unsafe`(unsafe fn/trait/impl, unsafe 블록
   본문), 간선 `unsafe`(unsafe {} 안의 참조·호출 = 경계 진입).
5. ~~ra_ap_* 의미 해석~~ — 완료. PR #8 머지됨(머지 커밋 70ea82e).
   `semantic` feature로 opt-in: `u.m()` 수신자 타입 해석(확정 간선),
   매크로 확장 워크, dyn/제네릭 디스패치 → 워크스페이스 impl 후보
   행렬(tentative 유지 — 실제 impl은 런타임 사실). hir이 모르는 본문은
   syn 폴백 + unmapped 실측. feature 없는 빌드에서 --semantic은
   종료 코드 2 + 빌드 안내(조용한 폴백은 거짓 계약이라 금지).
   후속 하드닝(PR #10, semantic-hardening → 머지됨): derive 생성 impl
   호출을 impl 대상 타입 정점으로 귀속, `load_out_dirs_from_check` +
   `ProcMacroServerChoice::Sysroot`로 build.rs 산출물·proc 매크로
   확장 로드, include!/생성 정의는 소속 모듈 정점 폴백, 타깃 없는 멤버
   (proc 매크로 크레이트)에 명시 Crate 정점(depends 간선 dangling 치유),
   proc 매크로 호출은 proc_macros 계수 + 서버 부재 시 실측 limitation.
   provenance 폐쇄: site→def 정체·모듈 문맥·소유 범위 대조, 확장 트리의
   함수형 매크로 인자·attr 인자·derive 출력·`#![inner]`까지 후보원 전수
   순회, 위조 impl 헤더는 트레이트/self 원본 범위 대조, 호출 속성은
   Attr+Meta 범위로 개별 확인 후 즉시 중단(뒤 메타는 입력 토큰),
   derive 인자는 순수 경로 세그먼트만 인정. modtree는 Module::dir
   실효 디렉터리 모델(rustc 실증: #[path] 로드 파일은 파일 디렉터리
   소유, 인라인 #[path]는 세그먼트 오버라이드). 각 가드는 뮤테이션으로
   비공허 검증됨. 로드맵 1~5 모두 완료 — 다음 우선순위는 사용자가 정한다.

## 막힌 것 / 주의

- Homebrew의 rust가 PATH를 잡는다 — llvm-tools 없음. coverage.sh가
  rustup 툴체인으로 자동 폴백한다.
- **ra_ap 버전 결합 함정.** ra_ap_*는 주 단위 릴리스이고 rust-version이
  자주 오른다 — 0.0.350부터 rustc 1.98 요구라 현재 최소 toolchain(rustc
  1.96)에는 0.0.349가 마지막이다. 게다가 0.0.349는 salsa 0.28.2·
  unicode-ident 1.0.24 조합에서만 빌드된다(salsa-macro-rules의 생성 코드가
  라이브러리 버전과 결합, unicode-properties와 unicode-ident의 유니코드
  버전이 일치해야 함) — Cargo.lock 핀을 함부로 `cargo update`하지 말 것.
- **ra_ap 트레이트·타입 쿼리는 `ra_ap_hir::attach_db`가 선행 조건이다.**
  스레드 로컬 attached db 없이 self_ty/resolve_method_call을 부르면
  panic. Engine::load의 인덱스 빌드와 본문 워크 둘 다 attach 안에서 돈다.
- **ra 타입의 "구체 여부"는 화이트리스트로 판정한다.** `type_is_concrete`가
  `Type::walk`으로 구성 타입 전부를 순회하며 알려진 구체 kind만 허용 —
  블랙리스트는 `dyn Send`처럼 principal 없는 객체(as_dyn_trait → None)와
  `!`(독립 kind, builtin 아님)를 놓쳤다. 모르는 kind는 열림으로 — 불확실성은
  항상 tentative 쪽으로만 새게 하는 계약과 같은 방향.
- **ra는 매크로 확장 안의 자기 크레이트명 경로를 해석하지 못한다.**
  proc 매크로가 `mycrate::x()`·`::mycrate::x()`를 emit하면 호출부가 그
  크레이트 안이어도 resolve_path가 None을 돌려준다(rustc는 받는다).
  fixture의 proc 매크로는 `crate::util::helper()`를 emit한다 — proc
  매크로는 `$crate`를 못 쓰므로 `crate::`가 호출부 크레이트를 가리킨다.
  해석 못한 확장 내 경로는 unresolved 카운터로만 간다.
- **semantic 경로 성능**(debug 바이너리, 웜 캐시): 자기 분석(29 파일)은
  syn 0.16s → semantic 29s, 합성 151파일 크레이트는 syn 1.6s → semantic
  18.7s. 비용은 거의 `load_workspace_at`(cargo check + salsa 크레이트
  그래프)에 있다 — 본문 워크 자체는 작다. `load_out_dirs_from_check`가
  `cargo check`를 `target/rust-analyzer` 서브디렉터리로 돌리므로
  첫 `--semantic` 실행은 전체 의존 트리 check 빌드 시간이 더해진다
  (rustograph 기준 수 분, 이후 증분). `/target` ignore가 이 서브디렉터리를
  커버한다.
- **ra_ap 버전 유지 절차.** bump할 때: (1) `rustc --version`이 대상
  ra_ap의 rust-version 이상인지 확인(rust-toolchain.toml 없음 —
  Homebrew rust가 PATH를 잡는다), (2) Cargo.toml의 `=0.0.X` 핀을 올리고
  `cargo update -p`가 아니라 lockfile 재생성, (3) salsa·salsa-macro-rules·
  unicode-ident 조합이 ra_ap가 요구하는 정확 버전으로 resolve되는지 확인
  (어긋나면 수동 pin), (4) `cargo test --features semantic`으로 통과 확인.
  rustc 1.98+ toolchain이 기본이 되면 0.0.350+로 올릴 수 있다.
- `cargo metadata`는 비워크스페이스 의존을 패키지로만 준다 — `--deps`는
  정점만 만들고 내부 수확은 안 한다.
- AST arena는 `Box::leak` — CLI 수명 모델이라 의도적.
- 이름 기반 경로 해석은 지역 바인딩을 모른다 — 모듈·타입 이름을 흔한
  지역 변수명(`args` 등)으로 지으면 `let args`가 모듈을 가리키는 가짜
  참조가 생긴다. `cli_args`라는 이름이 그래서다.
- 릴리스 직후 설치된 바이너리로 자기 분석(`cycles`/`rules --strict`)을
  다시 돌려라 — cli↔mcp 순환은 커밋 시점이 아니라 v0.2.0 배포 바이너리
  검증에서 잡혔고 0.2.1 패치가 됐다.
