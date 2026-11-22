# AGENTS.md

이 저장소에서 작업하는 코딩 에이전트를 위한 안내입니다.

> This file is written in Korean because it is the maintainer's working language.
> For the project overview in English, see [README.md](README.md).

**지금 어디까지 왔고 다음이 무엇인지는 [HANDOFF.md](HANDOFF.md)에 있습니다.**
세션을 이어받을 때 먼저 읽으세요.

자매 프로젝트가 바탕화면에 있습니다: [cartograph](../cartograph)(Swift) ·
[kartograph](../kartograph)(Kotlin/Android) · [dartograph](../dartograph)(Dart/Flutter) ·
[gartograph](../gartograph)(Go) · [pythograph](../pythograph)(Python) ·
[tsograph](../tsograph)(TypeScript/JavaScript) · [schemagraph](../schemagraph)(데이터베이스) ·
[isthmus](../isthmus)(언어 경계 조인 — 교환 형식·공유 적합성 벡터의 정본). 출력 계약 관례
(`state`/`edges`/`limitations` 철학)는 계열과 같게 유지합니다. `query` 출력 스키마는 이 저장소 고유의
공개 계약이며 cartograph·kartograph·dartograph와 맞추지 않습니다. 최상위는
`id`·`found`·`depth`·`truncated`·`neighbors[]`·`limitations`이고, 이웃은 `id`·`edges`·`direction`·`depth`·
`evidence`·`edge_evidence`입니다(`edge_evidence`만 snake_case인 것도 발행된 계약이라 유지합니다). 키 이름·구조를
바꾸면 소비자가 깨지므로 CHANGELOG Breaking 항목과 `docs/MIGRATING-*` 안내가 필요합니다. 새 필드는 camelCase로,
계열에 같은 의미의 이름이 있으면 그 이름으로 더합니다. 여기서 계약을 바꾸면 자매 저장소에도 알리세요.

## 이 프로젝트가 하는 일

Cargo 워크스페이스의 의존성 그래프를 만들고, 그 위에서 순환(cycles)·
미사용(dead)·규칙(rules)·이웃(query)·영향(impact)을 질의합니다.

핵심 설계는 한 문장입니다. **그래프가 산출물이고, 나머지는 전부 그 위의
질의입니다.** 새 기능을 넣을 때 "이것도 그래프 질의로 표현되는가"를 먼저
물어보세요.

수확은 두 층입니다: `cargo metadata`가 크레이트·의존을 주고, `syn`이
모듈 트리와 아이템·간선을 줍니다. 얕은 레벨(crate/module)은 심볼 그래프의
투영이지 별도 분석이 아닙니다. opt-in `semantic` feature를 켜면 `sem`이
rust-analyzer(`ra_ap_*`)로 본문 간선을 보강합니다 — 정점과 구조는 여전히
syn이 권위이고, 의미 해석은 호출·참조의 정확도만 올립니다.

## 명령

```bash
cargo build
cargo test                      # 단위 + tests/fixture 통합
cargo test --features semantic  # + tests/semantic.rs 의미 해석 테스트
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
cargo clippy --all-targets --features semantic   # sem.rs는 기본 빌드에 없다
cargo run -- rules --strict      # 도그푸딩 — 이 도구로 이 저장소를 분석
cargo run -- cycles --level symbol --strict
scripts/verify-self-dead.sh      # dead 자기 분석 — 알려진 목록(scripts/self-dead.expected)과 대조
```

**자기 분석은 필수입니다.** 크로스 크레이트 `use` 유실, 구조체 리터럴·
연관 함수 호출 미수확, lib/bin 루트 충돌은 전부 단위 테스트 통과 뒤
자기 분석에서 발견됐습니다. 건너뛰지 마세요.

## 절대 하지 말 것

- **`graph`에 외부 의존성을 추가하지 마세요.** 순수 도메인이어야 분석 계층
  전체를 cargo/syn 없이 테스트할 수 있습니다. `serde`만 허용됩니다.
- **`syn`과 `cargo`를 `harvest`/`modtree`/`cargo_meta`/`source` 밖에서
  쓰지 마세요.** `ra_ap_*`는 `sem` 안에만 있습니다(semantic feature).
  마찬가지로 YAML 파서 `serde_norway`는 `config` 안에만 있습니다(YAML 파싱은
  `config::load_file` 하나로).
  `.rustograph.yml`이 모듈 사이 경계를 스스로 강제합니다 — `rules --strict`가
  0이어야 합니다. 단 자기 분석은 syn이 수확한 경로만 봅니다. 표현식 목록으로
  파싱되지 않는 매크로 토큰(`json!({...})` 등) 안의 경로와 외부 크레이트
  (`syn`·`ra_ap_*`·YAML 파서) 사용은 규칙이 강제하지 못하니 리뷰에서 직접
  확인하세요. `json!` 안에서 `crate::cli::VERSION`을 읽은 sarif가 숨은
  cli ↔ sarif 순환을 만든 적이 있습니다 — 도구 버전은 진입점이 넘기거나
  (reach·schema·routes) 그 모듈에서 `env!("CARGO_PKG_VERSION")`을 바로 씁니다
  (sarif·mcp). 공개 함수 시그니처를 바꾸면 라이브러리 호출자가 깨지니
  `docs/MIGRATING-*.md`의 Rust library callers 절에 적으세요.
- **유령 정점을 만들지 마세요.** 해석 불가한 경로는 간선이 아니라
  `unresolved_paths` 카운터로 갑니다. 타깃 루트 모듈이 크레이트 정점을
  겸합니다 — 별도 루트 정점은 ID 충돌을 만듭니다. 같은 이름의 lib/bin은
  하나의 루트를 공유합니다(`extra_files`).
- **추정 간선을 확정 증거로 쓰지 마세요.** 메서드 팬아웃(`tentative: true`)은
  dead/도달성에만 쓰이고 cycles/rules는 제외+계수합니다. semantic 모드도
  같은 규칙입니다 — 타입으로 확정된 호출만 확정이고, `dyn`/제네릭 트레이트
  디스패치는 impl 후보 행렬로 펼치되 tentative를 유지합니다.
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
  따라 크로스 크레이트 임포트가 조용히 유실됩니다. 해석은 재노출(`pub use`)을
  정의 위치까지 따라가며 다른 모듈의 임포트 표를 읽으므로, 2단계는 임포트 표가
  더 바뀌지 않을 때까지 되풀이합니다(`fill_imports_settled`, 상한
  `MAX_IMPORT_PASSES` — 넘으면 limitation).
- **`use m::*` 글롭은 아이템별로 펼칩니다.** 모듈 자체만 매핑하면
  `f()` 호출이 미해석으로 샙니다. 대상 모듈의 재노출도 펼치되 rustc처럼
  들이는 모듈에서 보이는 이름만 펼칩니다(`Scope` — 비공개·`pub(super)`·
  `pub(crate)` 경계). 글롭한 모듈 자신은 이름이 아니라 의존 증거라
  `glob_targets`에만 둡니다(글롭 대상은 타입 이름공간에서 찾습니다). 펼친
  재노출의 cfg는 조건 집합으로 합쳐 같은 조건을 다시 합쳐도 그대로입니다.
- **임포트 표는 이름 → 이름공간(타입·값·매크로)별 바인딩입니다**
  (`PerNs<Binding>`). 글롭은 이름공간마다 따로 들이고, 명시 `use`·로컬
  아이템은 같은 이름공간의 글롭만 가립니다. 명시 `use a::x`는 `a`에서 `x`가
  차지한 이름공간을 모두 들입니다. 해석되지 않는 이름은 `Binding::Unresolved`로
  그 이름의 해석을 멈춥니다(크레이트 루트로 넘어가지 않습니다).
  **이름공간을 정하지 못한 명시 `use`**(외부 크레이트로 붕괴한 경로·std 경로·
  매크로가 등록한 이름)는 세 칸이 아니라 `PerNs::undetermined`에 둡니다 —
  rustc에서 명시 `use`는 같은 이름공간의 로컬 아이템·다른 명시 `use`와
  공존하지 못하므로(E0255·E0252), 같은 이름의 로컬 아이템이나 이름공간이
  정해진 바인딩이 있으면 타입·값 자리에서 그것을 이기지 못합니다. 같은 근거로
  매크로 자리에서는 그 `use`가 유일한 후보입니다 — 로컬 fn·trait과 정해진
  타입·값 바인딩은 매크로가 될 수 없습니다(`fn info` 옆 `use log::info`의
  `info!()`는 그 `use`이고, 로컬 fn은 추정 간선입니다). `#[cfg]`로
  갈린 대안 임포트(워크스페이스 쪽과 std 쪽)는 실제로는 같은 이름공간이라 이
  근거가 닿지 않지만, 소스 순서와 무관하게 워크스페이스 쪽이 이깁니다(std 쪽은
  그래프 밖이라 간선이 없습니다). 대신 어느
  이름공간을 차지하는지 모르니 같은 이름의 글롭은 모두 가립니다(`use
  std::fmt::Result`가 글롭의 로컬 `Result`를 가리는 것). 해석은 자리의
  이름공간이 먼저고(`ModTree::resolve_in` — 호출·값은 값, 타입 자리·구조체
  리터럴은 타입, 매크로 호출·속성·derive 인자는 매크로), 거기 묶인 것이
  없으면 자리를 모르는 해석(`resolve`: 로컬 → 정해진 임포트 → 정해지지 않은
  임포트)으로 넘어갑니다. 문법을 모르는 매크로 인자 토큰도 자리를 모르는
  해석입니다. 매크로 경로의 대상이 자리를 모르는 해석과 다르면 자리를 모르는
  해석의 대상에 추정 간선을 남깁니다(대개 이름공간을 나누기 전의 대상 — 보존을
  좁히지 않습니다).
- **같은 칸에 대상이 다른 임포트 둘은 둘 다 남습니다**(`Binding::Alternatives`).
  같은 이름공간에 같은 이름을 들이는 명시 `use` 둘이나 글롭 둘은 rustc에서 한
  빌드에 공존하지 못하니(E0252·E0659) `#[cfg]`로 갈린 다른 빌드의 것이고,
  그래프는 모든 빌드를 보므로 둘 다 남깁니다(이름을 묶지 않는 `use T as _`도
  모두 남습니다). 하나만 남기면 다른 빌드의 대상이 거짓 unreachable이 됩니다
  (rustix `ioctl`의 `use linux as platform`·`use bsd as platform`에서 리눅스 쪽
  `compose_opcode`와 상수 11개). 첫 원소가 소스
  순서상 뒤의 것이라 대상 하나를 고르는 `resolve`·`resolve_in`은 예전 대상을
  고르고, 간선을 만드는 해석은 `ModTree::resolve_all`로 대안마다 간선을 냅니다.
  **임포트를 거친 해석은 그 임포트의 cfg를 싣습니다**(`Resolved::cfg` — 거쳐
  온 `use`·재노출·글롭의 조건을 합친 것). 본문·시그니처·필드·속성 간선과
  `uses` 간선이 모두 그렇습니다. 간선을 만드는 새 해석 자리는
  `resolve_all`을 쓰세요 — 대상 하나만 잇거나 조건을 버리면 다른 빌드의
  대상이 거짓 unreachable이 되거나 `--target`·`--exclude-tests`가 그 빌드에
  없는 임포트를 거친 간선을 지우지 못합니다. 자기 `#[cfg]`를 단 명시 `use`는
  같은 칸의 글롭을 가리지 않고 합칩니다 — 다만 글롭 임포트의 조건이 그 `use`의
  조건을 함축하면(글롭이 보이는 빌드에는 늘 그 `use`도 있으면) 그 글롭은
  가립니다. 해석되는 임포트는 해석되지 않는 임포트(std 경로)를 소스 순서와
  무관하게 이깁니다. 모든 선언이 `#[cfg]`를 단 로컬 아이템(`Module::cfg_gated`,
  `#[cfg] mod` 포함)이 없는 빌드의 대안은 `Module::local_alternatives`입니다 —
  같은 이름공간의 명시 `use`는 늘(E0255), 글롭 이름은 선언이 하나이고 글롭
  대상이 스스로 조건부가 아닐 때만 그 조건의 부정을 달고 대안입니다(글롭 이름은
  로컬 아이템 옆에 합법적으로 있으며 가려지기만 하니, 선언이 여럿이면 모든
  빌드를 덮을 수 있습니다). 갈린 임포트를 self 타입이나 트레이트 경로로 쓴
  impl은 대안마다 수확하거나 implements 간선을 냅니다. routes는 아직 대상
  하나(`resolve`)입니다. 임포트 표 수렴 중 앞 바퀴의 대상이 거둬지면
  `fill_imports_settled`가 빈 표에서 다시 쌓습니다 — 서로를 글롭으로 들이는
  모듈 사이에 근거 없는 대상이 남지 않게 합니다. 2015 판 크레이트에서는
  거두지 않습니다(크레이트 기준 `use` 경로를 모델하지 않습니다 — limitation).
- **매크로 인자는 토큰 스트림입니다.** 쉼표로 나뉜 표현식 목록만 파싱을
  시도하고, 포맷 문자열의 `{ident}` 캡처를 참조로 잡습니다. 나머지 매크로
  문법은 limitation으로 셉니다.
- **파일 스캔이 모듈의 권위가 아닙니다.** `mod x;` 선언만이 파일을
  모듈로 만듭니다. 선언되지 않은 .rs는 orphan으로 세고 정점을 만들지
  않습니다.

## 커밋

Conventional Commits, 본문은 한국어. 스코프는 모듈 이름을 씁니다
(`graph`, `modtree`, `harvest`, `source`, `analysis`, `rules`, `export`,
`sarif`, `cli`, `config`, `cargo_meta`, `sem`).

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
