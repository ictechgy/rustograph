# AGENTS.md

이 저장소에서 작업하는 코딩 에이전트를 위한 안내입니다.

> This file is written in Korean because it is the maintainer's working language.
> For the project overview in English, see [README.md](README.md).

**지금 어디까지 왔고 다음이 무엇인지는 [HANDOFF.md](HANDOFF.md)에 있습니다.**
세션을 이어받을 때 먼저 읽으세요.

자매 프로젝트가 바탕화면에 있습니다: [cartograph](../cartograph)(Swift) ·
[gartograph](../gartograph)(Go) · [kartograph](../kartograph)(Kotlin/Android) ·
[dartograph](../dartograph)(Dart/Flutter) · [isthmus](../isthmus)(언어 경계 조인) ·
[schemagraph](../schemagraph)(데이터베이스). `query` 출력 스키마와
출력 계약 관례(`state`/`edges`/`limitations` 철학)는 계열과 같게 유지합니다.
여기서 바꾸면 자매 저장소에도 알리세요.

## 이 프로젝트가 하는 일

Cargo 워크스페이스의 의존성 그래프를 만들고, 그 위에서 순환(cycles)·
미사용(dead)·규칙(rules)·이웃(query)·영향(impact)을 질의합니다.

핵심 설계는 한 문장입니다. **그래프가 산출물이고, 나머지는 전부 그 위의
질의입니다.** 새 기능을 넣을 때 "이것도 그래프 질의로 표현되는가"를 먼저
물어보세요.

수확은 두 층입니다: `cargo metadata`가 크레이트·의존을 주고, `syn`이
모듈 트리와 아이템·간선을 줍니다. 얕은 레벨(crate/module)은 심볼 그래프의
투영이지 별도 분석이 아닙니다.

## 명령

```bash
cargo build
cargo test                      # 단위 + tests/fixture 통합
scripts/coverage.sh             # 테스트 + 커버리지 게이트(기준 90%)
scripts/verify-cli-contract.sh  # 빌드된 바이너리로 종료 코드 계약 검증
cargo clippy --all-targets      # 경고 0 유지
cargo fmt --check               # 포맷 정규

cargo run -- graph --level symbol | jq .
cargo run -- query <id> --depth 2
cargo run -- dead --explain <id>
```

완료를 보고할 때 아래 검증의 통과 근거가 있어야 합니다. 입력이 바뀌지 않은
검사는 기존 근거를 재사용합니다.

```bash
cargo test && scripts/coverage.sh && scripts/verify-cli-contract.sh
cargo clippy --all-targets
cargo run -- rules --strict      # 도그푸딩 — 이 도구로 이 저장소를 분석
cargo run -- cycles --level symbol --strict
```

**자기 분석은 필수입니다.** 크로스 크레이트 `use` 유실, 구조체 리터럴·
연관 함수 호출 미수확, lib/bin 루트 충돌은 전부 단위 테스트 통과 뒤
자기 분석에서 발견됐습니다. 건너뛰지 마세요.

## 절대 하지 말 것

- **`graph`에 외부 의존성을 추가하지 마세요.** 순수 도메인이어야 분석 계층
  전체를 cargo/syn 없이 테스트할 수 있습니다. `serde`만 허용됩니다.
- **`syn`과 `cargo`를 `harvest`/`modtree`/`cargo_meta`/`source` 밖에서
  쓰지 마세요.** 마찬가지로 `serde_yml`은 `config` 안에만 있습니다.
  `.rustograph.yml`이 이 경계를 스스로 강제합니다 — `rules --strict`가
  0이어야 합니다.
- **유령 정점을 만들지 마세요.** 해석 불가한 경로는 간선이 아니라
  `unresolved_paths` 카운터로 갑니다. 타깃 루트 모듈이 크레이트 정점을
  겸합니다 — 별도 루트 정점은 ID 충돌을 만듭니다. 같은 이름의 lib/bin은
  하나의 루트를 공유합니다(`extra_files`).
- **추정 간선을 확정 증거로 쓰지 마세요.** 메서드 팬아웃(`tentative: true`)은
  dead/도달성에만 쓰이고 cycles/rules는 제외+계수합니다.
- **삭제 판정을 내지 마세요.** `dead`는 `unreachable` 그래프 사실만
  보고합니다. trait 객체·제네릭·매크로로의 디스패치는 구문 분석에
  보이지 않습니다 — 그래서 limitation에 실측으로 남습니다.
- **`limitations`를 장식으로 쓰지 마세요.** 매번 붙는 상투적 경보는
  읽히지 않습니다. 실제로 센 값만 싣습니다. 셀 것이 없으면 조용해야 합니다.
- **커버리지 숫자를 올리려고 아무것도 검증하지 않는 테스트를 쓰지 마세요.**
  종료 코드 계약은 실제 바이너리(verify-cli-contract.sh)로 검증합니다.

## 에이전트가 소비하는 출력

`query`/`impact`와 JSON 리포트는 사람이 아니라 코딩 에이전트가 읽는다고
전제합니다.

- **이웃에 닿는 간선은 전부 줍니다**(`edges: ["call", "references"]`).
- **`truncated`/`depth`/`level`을 항상 씁니다.** 도달성 분석은 항상
  심볼 레벨입니다.
- **값이 없는 선택 필드는 키가 빠집니다** — `tentative` 없음 = 확정 간선.
- **정렬은 결정적이어야 합니다** — 정점 ID·간선 키 정렬, `sort()`가
  확정 간선을 추정 간선보다 우선시합니다.

## 수확 의미론 주의점

- **스코프는 두 단계입니다.** `fill_items`가 전 모듈의 아이템 표를 먼저
  채우고 `fill_imports`가 `use`를 해석합니다 — 한 패스면 모듈 처리 순서에
  따라 크로스 크레이트 임포트가 조용히 유실됩니다.
- **`use m::*` 글롭은 아이템별로 펼칩니다.** 모듈 자체만 매핑하면
  `f()` 호출이 미해석으로 샙니다.
- **매크로 인자는 토큰 스트림입니다.** 쉼표로 나뉜 표현식 목록만 파싱을
  시도하고, 포맷 문자열의 `{ident}` 캡처를 참조로 잡습니다. 나머지 매크로
  문법은 limitation으로 셉니다.
- **파일 스캔이 모듈의 권위가 아닙니다.** `mod x;` 선언만이 파일을
  모듈로 만듭니다. 선언되지 않은 .rs는 orphan으로 세고 정점을 만들지
  않습니다.

## 커밋

Conventional Commits, 본문은 한국어. 스코프는 모듈 이름을 씁니다
(`graph`, `modtree`, `harvest`, `source`, `analysis`, `rules`, `export`,
`sarif`, `cli`, `config`, `cargo_meta`).

```
feat(harvest): 구조체 리터럴 참조 간선 수확
fix(modtree): 크로스 크레이트 use가 모듈 순서에 의존하던 문제 수정
```

커밋은 작고 한 가지 목적만 담습니다. 본문에는 *왜*를 쓰세요.
`main`에 직접 커밋하지 말고 `feature/…`, `fix/…`, `refactor/…` 브랜치에서
작업하세요.

## 코드 스타일

- `cargo fmt` 기본, 주석은 한국어, 식별자는 영어. 사용자에게 보이는
  출력 문자열은 영어(오픈소스 대상).
- 모든 pub 타입·함수에 문서 주석. *무엇을*이 아니라 *왜*를 적으세요.
- 함수는 하나의 역할만. 본문이 길어지면 분리를 검토하세요.
- 빈 `match`/`Err` 무시 금지. 오류 메시지에는 원인과 해결 방향을 함께
  담습니다. 프로덕션 경로의 `unwrap`은 왜 실패 불가인지 주석이 있어야
  합니다.
