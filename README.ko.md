# rustograph

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
cargo install --git https://github.com/ictechgy/rustograph --tag v0.1.0
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
```

종료 코드: `0` 정상 · `1` strict 위반/발견 · `2` 사용법/분석 오류.

## 개발

```bash
cargo test                      # 단위 + fixture 통합 테스트
scripts/coverage.sh             # 커버리지 게이트 90%
scripts/verify-cli-contract.sh  # 실제 바이너리 종료 코드 계약
cargo run -- rules --strict     # 자기 분석(도그푸딩)
```

## MVP의 한계

구문 분석(syn)은 매크로 확장·`dyn` 디스패치·제네릭을 꿰뚫지 못합니다 —
모든 사각지대는 `limitations`에 실측으로 잡힙니다. 로드맵은 이 수확기를
rust-analyzer(`ra_ap_*`) 의미론으로 대체/보강하는 것이며, 그래프 계약은
그대로입니다.

## 라이선스

MIT — [LICENSE](LICENSE).
