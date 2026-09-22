# HANDOFF.md

세션을 이어받는 에이전트가 먼저 읽는 문서입니다.

## 현재 상태 (2026-09-22)

**MVP 완성 — 로컬 전용, 아직 push 안 함.** git init + 초기 커밋 단계.
원격 리포는 없다.

검증 상태: `cargo test` 55개 통과(단위 34 + 통합 21), 커버리지 91.6%
(게이트 90), clippy 클린, verify-cli-contract OK, 자기 분석
`rules --strict` 0 위반 / `cycles --strict` 0.

## 구조

- `src/graph.rs` — 순수 도메인(Document/Vertex/Edge/Level), 결정적 정렬,
  `Edge.tentative` + 투영 병합. 외부 의존 0(serde만).
- `src/cargo_meta.rs` — `cargo metadata` → 패키지/타깃/depends 간선.
- `src/modtree.rs` — `mod` 선언만으로 모듈 트리(`x.rs`|`x/mod.rs`|
  `#[path]`|인라인), orphan .rs 계수, 2단계 스코프(fill_items→fill_imports,
  글롭 확장 포함), 경로 해석.
- `src/harvest.rs` — syn 방문자. 아이템→정점, impl→메서드+implements,
  본문→call/references, 시그니처→signature, 매크로 인자·포맷 캡처.
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
- `src/cli.rs` — graph/cycles/dead/rules/query/impact/version,
  종료 코드 0/1/2.
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

1. **공개 리포 + 릴리스** — 계열 관례: GitHub `ictechgy/rustograph`,
   CI(`.github/workflows/ci.yml` 작성 필요), release 워크플로우 +
   Formula 템플릿(`Formula/rustograph.rb`), `ictechgy/homebrew-tap`.
   버전 태그는 cargo 관례 `v0.1.0`.
2. **MCP 서버** — `rustograph mcp` stdio, 계열과 같은 도구 셋
   (graph/cycles/dead/rules/query/impact).
3. **ra_ap_* 의미 해석** — MVP의 syn 수확을 rust-analyzer 의미론으로
   보강/대체: 타입 해석 메서드 호출, 매크로 확장, trait impl 행렬.
   그래프 계약은 불변 — 정확도만 올린다.
4. **feature/cfg 의존 모델링** — `#[cfg(feature)]`를 간선 메타데이터로.
5. **unsafe 경계** — `unsafe` 블록 진입을 간선/정점 속성으로 표시.

## 막힌 것 / 주의

- Homebrew의 rust가 PATH를 잡는다 — llvm-tools 없음. coverage.sh가
  rustup 툴체인으로 자동 폴백한다.
- `cargo metadata`는 비워크스페이스 의존을 패키지로만 준다 — `--deps`는
  정점만 만들고 내부 수확은 안 한다.
- AST arena는 `Box::leak` — CLI 수명 모델이라 의도적.
