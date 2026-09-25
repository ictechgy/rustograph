# HANDOFF.md

세션을 이어받는 에이전트가 먼저 읽는 문서입니다.

## 현재 상태 (2026-09-24)

**feature/schema-facts 머지됨 — PR #15(be116ae).** isthmus persistence
도메인의 두 번째 코드 생산자로 `rustograph schema`를 추가했다:
bridge-facts v1, `platform: "rust"` + `target: "persistence"`(사실
없으면 `target: null`), SQL 문자열·sqlx 매크로/함수·diesel `table!`·
DSL 경로·sea_orm 어트리뷰트에서 relation-use 사실 수확. GLM 리뷰
3라운드 반영 — 산문 오탐 게이트, 별칭·서브쿼리·`;` 다중 문장·
GRANT/REVOKE 객체 종류어, 미해석·unlocated 계수, DSL 경로는 선언된
table! 이름으로만 정적화. isthmus 쪽 계약 확장은 PR #110(rust 플랫폼 +
relation-use 허용, bridge 구성과 격리)로 머지됐다. `schema`는 v0.3.0부터
발행본에 들어 있다.

persistence 생산자는 현재 다섯 개 — schemagraph(SQL `relation-decl`
수신 측), gartograph(Go, #11)·rustograph(Rust)·kartograph(Kotlin,
#102)·cartograph(Swift, #136)가 `relation-use`를 낸다. isthmus의 도메인
판정은 `target === 'persistence'` 기준이며 persistence 문서는 bridge
수신 측 요건을 채우지 않는다(isthmus #111·#112). 계열 전체의 남은
후보는 dartograph 생산자·교차 도메인 상관·네트워크 도메인.

**v0.3.0 배포 완료(2026-09-25, PR #19, 태그 2da1fbf).**
https://github.com/ictechgy/rustograph (public),
`brew install ictechgy/tap/rustograph`로 설치 가능(brew test 통과).
semantic feature·paths/search/deps/schema·baseline·필터·캐시와 플래그
거부 동작 변경이 들어가 0.2.2가 아니라 마이너 bump다. 설치된 0.3.0
바이너리로 verify-cli-contract OK, 자기 분석 `rules`/`cycles --strict` 0.
이전: v0.2.0은 자기 분석에서 cli↔mcp 모듈 순환이 잡혀 인자 파서를
cli_args로 분리한 0.2.1로 패치됐다. PR #1~#6 머지됨.

검증 상태: `cargo test` 133개 통과(단위 88 + 통합 30 + schema 15) +
semantic feature 47개, 커버리지 92.66%(게이트 90, fix/cfg-attr-unparsed
기준), clippy 클린, verify-cli-contract OK(mcp 9도구 + `schema` 종료
코드·계약 필드), 자기 분석 `rules --strict` 0 위반 /
`cycles --strict` 0 — semantic 모드도 동일 0.
PR #8(의미 해석) 70ea82e · #10(의미 하드닝) 011c05b · #12(handoff)·
#13(gitignore) a04c270, a4b576a · **#14(경쟁툴 보완 팩) 머지됨 —
941da08.** PR #14는 Codex 2라운드 + GLM 1라운드 독립 리뷰를 거쳤고
지적은 전부 수정 커밋으로 반영됐다. **#15(schema) 머지됨 — be116ae.**

## 구조

- `src/graph.rs` — 순수 도메인(Document/Vertex/Edge/Level), 결정적 정렬,
  `Edge.tentative` + 투영 병합. `Vertex.cfg`/`unsafe` + `Edge.cfg`/`unsafe`
  메타데이터. 문서 필터 focus/without_tests/for_target. 외부 의존 0(serde만).
- `src/cfgeval.rs` — cfg 표현식 삼값 평가(Some/None — 미지 조건은
  보존). Facts = pairs+flags(명시 false 가능)+platform+rustc. 팩트는
  `rustc --print cfg --target` 실측이 권위, 폴백은 트리플 이름 기반
  추정(위치 직역 금지 — arm64_32=aarch64/32, wasip1=wasi+p1 등).
  부재=거짓은 KNOWN_FLAGS(platform)·TARGET_KEYS(rustc 실측)로만
  제한하고 VOLATILE(feature·debug_assertions·panic·target_feature
  등 프로필/빌드 의존)은 항상 미지. 순수 도메인.
- `src/cargo_meta.rs` — `cargo metadata` → 패키지(version/lib_name)/
  타깃/depends 간선, `rustc --print cfg`/`rustc -vV` 프로브,
  `Package::crate_vertex()`(패키지→그래프 정점 ID). lib_name은
  `package = "real"` rename의 코드상 이름.
- `src/modtree.rs` — `mod` 선언만으로 모듈 트리(`x.rs`|`x/mod.rs`|
  `#[path]`|인라인), orphan .rs 계수, 2단계 스코프(fill_items→fill_imports,
  글롭 확장 포함), 경로 해석. DepCrates는 크레이트별 스코프 —
  선언된 dep 별칭이 동명 워크스페이스 루트보다 우선하고, 트리에 없는
  멤버 루트(proc-macro 크레이트)는 외부처럼 크레이트 정점으로 붕괴.
- `src/harvest.rs` — syn 방문자. 아이템→정점, impl→메서드+implements,
  본문→call/references, 시그니처→signature, 매크로 인자·포맷 캡처,
  `#[cfg]` 추출(cfg_of), unsafe 감지(unsafety/unsafe 블록/unsafe impl),
  속성 경로 수확(#[dep::attr]·derive(dep::X)·cfg_attr — dep 사용 증거).
  cfg_attr 술어는 토큰 원문 보존(split_cfg_attr — 이스케이프 디코드로
  조건이 뒤집히는 것 방지), 도구 네임스페이스(rustfmt/clippy/
  diagnostic) 속성은 수집하지 않는다. 속성 목록을 Meta로 못 읽은
  cfg_attr는 `Harvest.unparsed_attrs`로 세고 limitation 한 줄을 낸다
  (deps 보고서는 harvest limitation을 싣지 않는다 — 기존 설계).
- `src/source.rs` — 오케스트레이터. AST arena('static 누수), 루트 병합
  (lib/bin 같은 이름 → extra_files), 보존 루트(main/#[no_mangle]/
  --tests/--retain-public), load(캐시)/harvest 분리, semantic 캐시
  (.rustograph/semantic-cache.json, 스키마 v2 — 지문은 소스+매니페스트+
  .cargo/config+rustc -vV+RUSTFLAGS의 FNV). 캐시는 커버되지 않는 입력
  (include! 계열·루트 밖 #[path]·target/·숨김 디렉터리 정점)이 있는
  문서를 읽기·쓰기 양쪽에서 거부한다.
- `src/analysis.rs` — Tarjan SCC(tentative 제외), dead(BFS+explain),
  query/impact(전이 클로저), paths(bounded BFS — 예산 소진 뒤에도
  큐의 목적지 상태를 수확, 상한 초과는 truncated), search(정확>
  꼬리>부분 순위), resolve_id(정확만 즉시, 애매는 후보 Err).
- `src/rules.rs` — allowlist+deny+signature 규칙, unmapped 보고,
  skipped_tentative 계수, Baseline(`rule|from|to|kind` 키 — 기존 위반
  얼리기, baselined/stale_baseline 계수).
- `src/deps.rs` — 미사용 의존 + 중복 버전 보고(선언 대비 실참조).
  사용 증거는 정점 ID 기준 — 멤버는 lib/bin 타깃 이름, 외부는 패키지
  이름. build·dev 의존은 수확 범위 밖이라 판정 제외+limitation 계수
  (착신 오탐 금지). 외부명=멤버 루트명 충돌은 모호로 limitation.
  report는 항상 자체 syn 수확 — 호출자 문서는 필터로 증거가 지워진다.
- `src/export.rs` — 결정적 JSON + mermaid + save/load.
- `src/sarif.rs` — SARIF 2.1.0(`rustograph/deny` 등 ruleId).
- `src/config.rs` — `.rustograph.yml` 파싱(serde_yml 격리), baseline 키.
- `src/source/schema.rs` — isthmus bridge-facts 생산자(`rustograph
  schema`). SQL 문자열·sqlx·diesel table!·DSL 경로·sea_orm에서
  relation-use 사실 수확, 산문 오탐 게이트·미해석/unlocated 계수.
- `src/cli.rs` — graph/cycles/dead/rules/query/impact/paths/search/deps/
  schema/mcp/version, 종료 코드 0/1/2.
- `src/cli_args.rs` — cli/mcp 공유 인자 파서(최하층, 순환 방지).
  이름이 args가 아닌 이유는 파일 헤더 주석 참고 — 지역 변수 `args`가
  이름 해석으로 모듈을 가리키는 가짜 참조를 피한다. 문서 필터는
  --exclude-tests → --target → --focus 순으로 수확·로드 문서 모두에 적용.
- `src/mcp.rs` — MCP stdio 서버(NDJSON JSON-RPC 2.0). 기동 시 문서 1회
  수확 후 스냅샷 서빙. 도구 9종: rustograph_summary/query/impact/paths/
  search/cycles/dead/rules/deps. deps 보고서는 자체 syn 수확이
  필요해(호출자 문서는 필터로 증거가 지워짐) 스냅샷 밖에서
  OnceLock lazy-once
  (요청마다 수확하면 Box::leak AST가 누수). 숫자 인자는 usize::try_from
  검증. stdin을 파라미터로 받아 테스트 가능.
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
   없어 탭 갱신은 수동으로 했다(0.3.0까지) — 넣으면 다음 릴리스부터 자동.
   수동 절차: 게시된 checksums.txt를 받아 `shasum -c`로 대조 →
   release.yml의 sed와 같은 치환으로 Formula/rustograph.rb를 렌더 →
   탭 main에 `rustograph X.Y.Z` 커밋(기존 관례) → `brew upgrade` 후
   설치 바이너리로 계약·자기 분석.
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
   비공허 검증됨.
6. ~~경쟁툴 보완 팩~~ — 완료. PR #14 머지됨(941da08). cargo-modules·
   cargo-callgraph·arch 계열·machete/udeps·depgraph와 비교해 벤치마크한
   7종: paths(도달 경로 BFS+예산), rules baseline(레거시 도입),
   deps(미사용+중복 버전 — 속성 경로 수확으로 proc-macro 의존 오탐 방지),
   search+애매 ID 거부(후보 열거), semantic 캐시(지문 키),
   focus/exclude-tests/target 필터(cfgeval 삼값 — 미지 조건 보존).
   deps는 syn 수확을 쓴다 — 의미 해석은 외부 크레이트 내부를 안 봐서
   증거가 아니라 오탐이다. Codex 2라운드+GLM 1라운드 독립 리뷰의 지적은
   전부 수정됨 — 핵심은 "부재=거짓"의 범위를 KNOWN_FLAGS·TARGET_KEYS로
   제한한 cfg 팩트 모델과 dev/build 의존 판정 제외다.
   다음 우선순위는 사용자가 정한다.
7. ~~feature/schema-facts~~ — 완료. PR #15 머지됨(be116ae).
   `source/schema.rs` 신규 + `rustograph schema` 명령으로 isthmus
   persistence 도메인의 bridge-facts v1을 낸다. GLM 리뷰 3라운드 반영.
   상세는 위 "현재 상태" 첫 단락 참고. 후속(fix/schema-contract):
   schema는 --dir/--out 외 플래그·위치 인자를 허용 목록으로 거부(2)하고,
   verify-cli-contract가 종료 코드와 target null/persistence 계약을 본다.
8. 다음 우선순위는 사용자가 정한다. 알려진 보류 항목은 아래
   "막힌 것 / 주의" 참고 — Codex 3차 리뷰(사용량 한도 — GLM 리뷰가
   역할을 대신했다). `split_cfg_attr` 미계수는 fix/cfg-attr-unparsed로
   해소.

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
- **독립 리뷰 도구.** `packet-ask review --provider glm --diff <ref>`가
  diff를 스크럽해 GLM에 보낸다(패킷만 보고 diff는 실물 repo를 못 본다 —
  발견은 반드시 코드 대조 검증). `codex exec`도 쓰지만 사용량 한도가
  있다. GLM 리뷰 보류 항목이던 `split_cfg_attr` 미계수는 전용 카운터
  (unparsed_attrs)로 해소됐다 — syn 2.0.119는 `unsafe(...)`도 Meta로
  받으므로 rustc도 거부하는 입력에서만 나는 경로다.
- **deps 보고서의 판정 경계.** `--deps` 문서에만 외부 정점이 있고,
  dev 의존의 사용은 tests/examples/benches(미수확)에 산다 — 둘 다
  증거 불완전이니 finding이 아니라 limitation이다. 외부 패키지명과
  멤버 루트 ID가 같으면 간선 귀속을 문자열로 구별 못 하니 모호 계수.
- AST arena는 `Box::leak` — CLI 수명 모델이라 의도적.
- 이름 기반 경로 해석은 지역 바인딩을 모른다 — 모듈·타입 이름을 흔한
  지역 변수명(`args` 등)으로 지으면 `let args`가 모듈을 가리키는 가짜
  참조가 생긴다. `cli_args`라는 이름이 그래서다.
- 릴리스 직후 설치된 바이너리로 자기 분석(`cycles`/`rules --strict`)을
  다시 돌려라 — cli↔mcp 순환은 커밋 시점이 아니라 v0.2.0 배포 바이너리
  검증에서 잡혔고 0.2.1 패치가 됐다.
