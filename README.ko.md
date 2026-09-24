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
cargo install --git https://github.com/ictechgy/rustograph --tag v0.2.1
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
rustograph query mycrate::module::f --depth 2
rustograph impact mycrate::Type --depth 3
rustograph mcp                           # MCP stdio 서버 — 에이전트가 되묻는 통로
rustograph schema --dir . --out schema-facts.json  # isthmus persistence 사실
```

`#[cfg]` 조건은 메타데이터로 그래프에 실립니다 — 정점의 `cfg`는 자기
`#[cfg(...)]` 토큰, 간선의 `cfg`는 그 조건 아래서만 성립하는 의존을
표시합니다. `unsafe`는 경계를 표시합니다 — `unsafe fn`/`unsafe trait`이거나
`unsafe {}` 블록을 품은 정점에 `unsafe: true`, `unsafe {}` 안에서 만든
간선은 경계 진입 간선입니다.

`rustograph mcp`는 개행 구분 JSON-RPC 2.0을 stdio로 말하고 도구 6종
(`rustograph_summary`/`_query`/`_impact`/`_cycles`/`_dead`/`_rules`)을
서빙합니다. 문서는 기동 시 한 번 수확해(또는 `--graph`로 읽어) 같은
스냅샷 위에서 답합니다.

종료 코드: `0` 정상 · `1` strict 위반/발견 · `2` 사용법/분석 오류.

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

비한정 이름(`query!`, `sql_query`, `table!`)은 그 파일이 sqlx/diesel에서
import할 때만 인정합니다. 파싱 실패 파일·문법이 다른 `table!`·테이블
바인딩 없는
컬럼 어트리뷰트는 조용히 넘기지 않고 `limitations`로 셉니다. 이름 기반
스캔은 추측하지 않습니다 — 정적으로 해석할 수 없는 것은 지어내지 않고
센 것입니다.

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
