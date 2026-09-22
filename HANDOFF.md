# HANDOFF.md

세션을 이어받는 에이전트가 먼저 읽는 문서입니다.

## 현재 상태 (2026-09-22)

**v0.2.1 배포 완료.** https://github.com/ictechgy/rustograph (public),
`brew install ictechgy/tap/rustograph`로 설치 가능(brew test 통과).
v0.2.0도 배포됐으나 자기 분석에서 cli↔mcp 모듈 순환이 잡혀
인자 파서를 cli_args로 분리한 0.2.1이 최신이다. PR #1~#6 머지됨.

검증 상태: `cargo test` 63개 통과(단위 40 + 통합 23), 커버리지 91.2%
(게이트 90), clippy 클린, verify-cli-contract OK(mcp 포함),
자기 분석 `rules --strict` 0 위반 / `cycles --strict` 0.

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
- `tests/fixture/` — 두 멤버 워크스페이스, 모든 아이템 종류+글롭/별칭/
  cfg/generated/orphan 커버.
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
5. **ra_ap_* 의미 해석** — MVP의 syn 수확을 rust-analyzer 의미론으로
   보강/대체: 타입 해석 메서드 호출, 매크로 확장, trait impl 행렬.
   그래프 계약은 불변 — 정확도만 올린다. 의존 크기와 주 단위 API 변동
   때문에 v0.2.0에서는 유보했다 — cargo feature로 opt-in 경로가
   자연스럽다.

## 막힌 것 / 주의

- Homebrew의 rust가 PATH를 잡는다 — llvm-tools 없음. coverage.sh가
  rustup 툴체인으로 자동 폴백한다.
- `cargo metadata`는 비워크스페이스 의존을 패키지로만 준다 — `--deps`는
  정점만 만들고 내부 수확은 안 한다.
- AST arena는 `Box::leak` — CLI 수명 모델이라 의도적.
- 이름 기반 경로 해석은 지역 바인딩을 모른다 — 모듈·타입 이름을 흔한
  지역 변수명(`args` 등)으로 지으면 `let args`가 모듈을 가리키는 가짜
  참조가 생긴다. `cli_args`라는 이름이 그래서다.
- 릴리스 직후 설치된 바이너리로 자기 분석(`cycles`/`rules --strict`)을
  다시 돌려라 — cli↔mcp 순환은 커밋 시점이 아니라 v0.2.0 배포 바이너리
  검증에서 잡혔고 0.2.1 패치가 됐다.
