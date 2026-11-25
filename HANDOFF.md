# HANDOFF.md

세션을 이어받는 에이전트가 먼저 읽는 문서입니다. 아래 "현재 상태"만 지금의 지시이고, 그 아래 날짜가 붙은 절은
당시 기록입니다 — 거기 적힌 "릴리스 준비"·"다음 단계"를 지금 할 일로 되살리지 마세요.

## 현재 상태 (2026-11-25, 5차) — 타입 별칭 우변·제네릭 간선, 선언 타입 경로의 자리별 해석

- 발행본은 [v0.7.0](https://github.com/ictechgy/rustograph/releases/tag/v0.7.0)(2026-11-03, 자산 6개)이다.
  v0.6.0은 2026-10-25, v0.5.0은 2026-10-13에 발행됐다. v0.4.2는 태그 없이 v0.5.0으로 이어졌다.
- 4차 브랜치(타입 경로 호출의 타입 참조·`const _` 루트·밖 타입 impl 트레이트 간선)는
  [PR #58](https://github.com/ictechgy/rustograph/pull/58)로 `main`에 스쿼시 머지됐다(`61e719b`, 아래 그 날짜 절).
- **브랜치 `ccr-37904670-hxdjsx`**(`origin/main` `61e719b` 위에서 다시 시작)에 아래 커밋이 있다. `2c548be`까지는
  코디네이터가 [PR #60](https://github.com/ictechgy/rustograph/pull/60)으로 push했고, 독립 리뷰 반영(`ca36e69`부터)은
  push하지 않았다(2026-11-25). 모두 4차 절의 후속 후보("타입 별칭 우변")와 그 수정이 드러낸 같은 계열 결함이다. 공개
  Rust API 변화는 0.8에서 새로 생긴 `harvest::DeclRef`의 필드 `slot`과 새 `harvest::DeclSlot`뿐이다
  ([0.8 migration](docs/MIGRATING-0.8.md)). 사용자 변화는 CHANGELOG `[Unreleased]`, README·README.ko 그래프 모델에 있다.
  - `115103e` fix(harvest): 타입 별칭 우변(`type A = B;`)을 필드 타입처럼 `DeclOwner`로 모아 별칭에서 나가는
    references 간선으로 낸다(타입 이름공간 `resolve_all` — 대안 임포트마다, 별칭·임포트 조건을 싣고, `#[cfg]`로 두 번
    선언한 별칭은 소스 인스턴스 ID마다). 별칭을 그대로 모으면 `type Arr<const N: usize> = [u8; N];`의 `N`이 모듈의
    `struct N`으로 풀리는 거짓 간선이 새로 생겨, 선언 타입의 표현식 자리(배열 길이, `{ N }` 인자)에서 const 매개변수와
    이름이 같은 한 세그먼트 경로를 그 매개변수로 본다(`DeclTypeVisitor`·`is_generic_param`) — 필드에도 있던 결함이다.
    심볼 레벨 cycles는 별칭을 데이터 타입으로 본다(`analysis::symbol_level_data_types`).
  - `97d2ca1` fix(harvest): 시그니처(`signature_edges`)도 같은 규칙 — `fn f<const N: usize>(a: [u8; N])`, impl·트레이트
    const 매개변수를 쓰는 메서드 시그니처가 모듈의 `struct N`·`const LEN`으로 signature 간선을 달던 것.
  - `2c548be` docs: README·README.ko 그래프 모델, CHANGELOG, HANDOFF.
  - `ca36e69` fix(harvest) — 리뷰 S1·S2: 선언 타입 경로를 자리대로 푼다(`DeclSlot`). 표현식 자리는 값 이름공간이 먼저고,
    둘러싼 선언의 const 매개변수와 이름이 같은 맨 제네릭 인자는 타입 이름공간의 대상만 잇는다(`TypeOrConstParam`).
  - `f010e35` fix(harvest) — 리뷰 S3: 별칭 제네릭의 bound·기본값·where 절과 const 기본값을 수확한다(`push_generics`).
  - docs: README·README.ko·CHANGELOG·0.8 migration, 이 HANDOFF.
- **독립 리뷰(PR #60, `2c548be` 대상 — blocker 없음) 반영** — 리뷰어 스크래치가 지워져 설명된 fixture를 다시 만들었다
  (스크래치 `r5-rustograph/rv/`, 모두 `cargo check` 통과). main·`2c548be`·고친 뒤로 대조했다. f02~f08·f10~f14·
  f16~f18·f20·f21은 내용이 전해지지 않아 다시 만들지 못했다 — 대신 이 세션의 a1~a18·t1~t5를 `2c548be`와 고친 뒤로
  다시 돌렸고, 바뀐 것은 a5c(아래)뿐이다.
  - S1(이 브랜치의 회귀): f09 `type A<const N: usize> = Buf<N>;` 옆 `const N` — rustc 경고 `N`. main `N`, `2c548be`
    0건과 거짓 type 레벨 순환 `crate -> A`, 고친 뒤 `N`·순환 0. f22(필드 `Buf<N>`·시그니처 `Buf<M>`, rustc 경고 `N`·
    `M`): main·`2c548be` 0건, 고친 뒤 `M`·`N`(main부터 있던 거짓 type 순환 `crate -> F`도 사라짐). a5c(`Buf2<LEN>`, rustc
    경고 `LEN`): main·`2c548be` 0건, 고친 뒤 `LEN`. `Vec<N>` 옆 `struct N`(s1의 `t::V`)은 rustc처럼 그 struct다.
  - S2: f19(`use super::k::N;` + `struct N` + `[u8; N]` — 별칭·필드·시그니처, rustc 경고 struct `N`·`Z`·`Q`): main
    `k::N`·`k::Q`·`k::Z`(거짓)·`m::N`, `2c548be` `k::*`만(거짓 간선 `m::A -> m::N`이 `m::N`을 가림), 고친 뒤 `f::Z`·
    `m::N`·`s::Q` — rustc와 같다.
  - S3: f23(`pub type Result<T, E = Error> = ..`, rustc 경고 없음): main·`2c548be` `Error`, 고친 뒤 0건. 별칭만으로 다시
    만든 f15(별칭의 bound·기본값·where 절·const 기본값 `DEF`): main·`2c548be` 넷, 고친 뒤 0건. 리뷰어의 원래 f15는
    `struct D<const N: usize = DEF>`도 담아, 고친 뒤에도 `DEF`를 낸다(재확인에서 확인) — struct·enum·trait·fn 제네릭의
    bound·기본값·where 절은 수확하지 않는다(아래 후속 후보, README에 적었다).
  - nit: 별칭을 거친 자기 재귀(n1 `type Link2 = Option<Box<Node2>>;`)는 type 레벨 순환이고 같은 struct 직접 재귀는 아니다
    — 기록만(아래 후속 후보, 사용자 결정). 모듈 상수·매크로를 가리키는 별칭은 type·module 레벨에서 상수가 모듈로 접혀
    그 모듈과 순환일 수 있다(f01 `type Arr = [AE; LEN];`, 같은 필드를 가진 struct도 main부터 같다) — README에 적었다.
    `deps`는 `#[cfg]`를 평가하지 않아 n5 `#[cfg(any())] type Never = never::X;`가 `never`를 쓰인 것으로 센다(rustc
    `-W unused-crate-dependencies`는 경고, main은 별칭을 수확하지 않아 unused로 냈다 — 필드와 같은 규칙) — README에
    적었다. impl 연관 타입 `type Item = OnlyItem`과 `macro_rules!`가 만든 별칭(n4)은 main부터 거짓 dead로 남는다(아래 후속
    후보).
- **재현과 근거**(스크래치 `r5-rustograph/fx/`, 모두 `cargo check` 통과 — 리눅스, cfg 갈림은
  `--target x86_64-pc-windows-msvc`도 — main(`61e719b`) 바이너리와 새 바이너리로 대조):
  - a1·a4·t1(별칭으로만 만드는 enum·struct, 별칭 사슬 `A = B = C`, `<Q as Tr>::Assoc`, 제네릭 `GA<T>` 옆 `struct T`,
    쓰이지 않는 별칭 뒤 `Never`): rustc 경고는 `T`·`Never`·`UnusedAlias`뿐인데 main은 9개(`Only`·`Lit`·`B`·`C`·`Q`·
    `Tr`까지)를 냈다. 새것은 그 셋만.
  - a3·a8·t3(`#[cfg(unix)] pub type Sys = U;`/windows, 갈린 임포트를 우변으로 쓴 `pub type Imp = Impl;`): main은 기본·
    리눅스·windows 모두 넷을 냈다. 새것은 기본 0건, 리눅스 `plat::W`·`w::Impl`, windows `plat::U`·`u::Impl` — rustc 두
    빌드 경고와 같다. a7(`#[cfg(test)] type Clock = fake::Clock;`/`not(test)`): `--exclude-tests`가 main은 둘 다, 새것은
    `fake::Clock`만(rustc 비테스트 빌드 경고와 같음).
  - a15(`app`이 `dep`을 별칭 `Option<dep::Shared>`에서만 씀): main `deps`가 `unused: app -> dep (kind: normal)`(종료 코드
    `--strict` 1), 새것 0 unused.
  - a5b·t2(필드 `[u8; N]`·`[u8; LEN]`·`Buf<{ N }>`, 별칭 `[u8; N]`, `size_of::<N>()`): rustc 경고는 `m::N`·`m::LEN`뿐(`k::N`은
    필드를 읽지 않는다는 경고만). main은 둘 다 내지 않고 대신 `k::N`을 냈다(별칭에서만 쓰여서). 새것은 rustc와 같다.
    t5·a16(fn 시그니처 `[u8; N]`, impl·트레이트 const 매개변수를 쓰는 메서드 시그니처): main은 `N`·`LEN`을 내지
    않았다. 새것은 rustc처럼 둘 다 낸다. a17(밖 타입 impl `impl<const N: usize> Get for [u8; N]`, 함수 안
    `fn inner<const N>`)은 `Z::<Mk>::mk → K` 거짓 signature 간선만 사라지고 dead는 main과 같다(`N`, 그리고 main부터 내던 본문 없는 트레이트 선언 메서드
    `Mk::mk`).
  - a6·t4(`type Link = Option<Box<Node>>`, `type Cb = fn(&Ctx)`, 모듈을 넘는 `type Res<T> = Result<T, b::Error>`):
    심볼 레벨 순환 0(별칭을 데이터 타입으로 보지 않는 변이는 `Link -> Node`를 낸다), type 레벨 `Cb ↔ Ctx`·`Link ↔ Node`,
    module 레벨 `a ↔ b`(main은 모두 0).
- **적대적 입력**(위 fixture와 a2·a9·a10·a11·a12·a13·a14·a18, 모두 main·새 바이너리로 `dead` 기본·`--tests`·
  `--exclude-tests`·`--target` 리눅스/windows, `cycles` symbol·type·module 대조): 별칭의 const 매개변수(a2 — `struct N`·
  `T`는 rustc처럼 dead), `type Out = <Q as Tr>::Assoc`(a9), `type Result<T> = std::result::Result<T, Error>`·
  `type Option = std::option::Option<OnlyAliased>`(a10), 표현식 안 타입 자리(a11), 함수 안 별칭·impl 연관 타입(a12 —
  변화 없음), lib `--retain-public`(a13 — 변화 없음), 다른 크레이트의 별칭(a14 — main의 거짓 `dep::Shared` 사라짐),
  `use std::process;` 옆 `fn process`와 `type C = process::Child`·글롭으로 들인 타입의 별칭·`pub use user::X as Re`·
  `#[cfg(test)]` 모듈 안 별칭(a18 — 새것은 rustc처럼 `fn process`·`Other`만, `--tests`는 테스트 별칭의 `Helper`도 살림).
  새로 낸 dead는 모두 rustc가 경고하는 것이다. `2c548be`에서는 a5c `type ArrLen<const LEN: usize> = Buf2<LEN>;`·필드
  `Buf2<LEN>`의 맨 인자 `LEN`이 모듈 `const LEN`으로 갔다(main은 필드에서 같았고, 별칭에서는 이 브랜치가 새로 드러냈다 —
  리뷰 S1). `ca36e69`부터 rustc처럼 `LEN`을 낸다.
- **반례 말뭉치**(4차와 같은 크레이트 17개, 이 저장소는 `61e719b` 사본. main·`2c548be`·마지막 바이너리로 `graph --level
  symbol --tests`, `dead` 여섯 변형, `cycles` 세 레벨 × `--tests` 유무, `deps` — 스크립트는 스크래치 `r5-rustograph/bin/`,
  리뷰 반영 뒤 셋 모두 다시 돌렸다):
  - 정점 ID·그래프 limitation·`deps`는 17개 모두 같다. main 대비 잃은 간선 0, 새 간선 280개(모두 별칭에서 나가는
    references — ra_ap_hir_ty 141·ra_ap_hir_def 55·zerocopy 28·rowan 22·rustix 10·이 저장소 10·tokio 5·clap_builder 5·
    regex-syntax 2·syn 2, 추정 8개는 rustix `Opcode → _Opcode`(선언 8개인 cfg 별칭)). 커밋별로: `115103e` +268,
    `97d2ca1` 0, `ca36e69`(자리 규칙) 0 — 필드·시그니처·별칭의 간선 가운데 대상이 바뀌거나 사라진 것이 없다, `f010e35`
    +12(별칭 bound·기본값: clap_builder·ra_ap_hir_ty의 `Result<T, E = ..>`류, ra_ap_hir_ty `Autoderef`의 기본값,
    ra_ap_hir_def `ItemTreeAstId<T: ItemTreeNode>`, zerocopy의 `TryFromBytes` bound 등).
  - 새로 낸 dead 0(여섯 변형 모두). main 대비 사라진 것: 기본 23(ra_ap_hir_def 19 — `BlockIdLt`·`Binding`·`Pat` 등
    `Idx<..>`·`..Lt<'static>` 별칭 뒤 타입, ra_ap_hir_ty 3 — `DbInterner`·`HirDatabase`·`DbInternerInferExt`, zerocopy 1 —
    `ConvertError`), `--tests` 19(rustix `_Opcode` 선언 8·ra_ap_hir_def 4·ra_ap_hir_ty 4·clap_builder 3 — `pub type Error =
    error::Error<error::DefaultFormatter>` 뒤 포매터), `--retain-public`·`--tests --retain-public`·리눅스·windows 각 2
    (ra_ap_hir_ty `infer::MinCaptureList` — 별칭 `RootVariableMinCaptureList`가 쓴다, ra_ap_hir_def `ItemTreeNode` —
    `f010e35`의 bound 간선). 바뀐 dead limitation은 셈만 바뀐다(루트 없는 lib의 공개 아이템 수가 줄고, 새로 닿은 타입의
    외부 트레이트 impl 메서드 수가 는다).
  - 심볼(29)·module(34) 레벨 순환은 17개 모두 그대로다. type 레벨은 8개 크레이트에서 강연결 요소가 커지거나 합쳐져 합계
    99 → 97개다: syn 9 → 7(`ParseStream<'a> = &'a ParseBuffer<'a>`·`Result<T> = ..<T, Error>` 간선으로 7·2·2개짜리 셋이
    15개짜리 하나로), zerocopy 11 → 10(`CastError ↔ TryCastError`가 큰 것에 합쳐짐), ra_ap_hir_ty 4 → 5(330 → 428개와
    새 4개짜리 — 별칭 `DeconstructedPat`·`MatchArm`·`WitnessPat`과 `MatchCheckCtx`), 나머지는 수 같고 크기만
    (clap_builder 45 → 50, ra_ap_hir_def 134 → 189, regex-syntax 38 → 40, rowan 7 → 9·3 → 5, 이 저장소 `source` 4 → 5 —
    `type VertexSpans`).
  - 같은 크레이트에서 두 번 돌린 `graph`는 바이트 단위로 같았다(zerocopy).
- **검증**(2026-11-25 리뷰 반영 뒤, 로컬 cargo 1.97, `CARGO_PROFILE_DEV_DEBUG=0`·`-j 2`, `CARGO_TARGET_DIR`는 스크래치):
  `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings` 0, `cargo test --locked`(26개 실행 단위 602개
  통과 — 이 브랜치의 새 통합 테스트 9개), `scripts/coverage.sh` 94.87%(게이트 90%, 그 안의 테스트 실패 0),
  `scripts/verify-cli-contract.sh <바이너리>` OK, `rules --strict` 0, `cycles --level symbol --strict` 0(type 23·module 2 —
  변경 전과 같음), `scripts/verify-self-dead.sh <바이너리>` 0건 일치. MSRV `cargo +1.96.0 check --locked --all-targets` 통과
  (리뷰 반영 뒤 다시 돌림). 리뷰 반영의 새 테스트
  3개는 `2c548be`의 `src/`로 돌리면 3개 모두 실패하고, 변이 7개(타입 이름공간 거르기 끄기·const 이름 맨 인자를 늘 버리기·
  값 자리를 타입으로·맨 인자를 따로 적지 않기·제네릭 수확 끄기·where 절 빼기·const 기본값 빼기)를 모두 잡는다. 새 통합 테스트 5개(`115103e`)는 `origin/main`의 `src/`로 돌리면 모두 실패하고, 변이 6개(우변 수확
  끄기·별칭을 데이터 타입에서 빼기·const 매개변수 가리기 끄기·표현식 안 타입 자리를 되돌리지 않기·모든 자리에서 가리기·
  별칭 조건 버리기)를 모두 잡는다. 시그니처 테스트(`97d2ca1`)는 `115103e`의 `src/`로 돌리면 실패하고 시그니처에서 const
  매개변수를 가리지 않는 변이를 잡는다.
  **실행하지 않음**: `--features semantic` test·clippy와 MSRV의 semantic check(공유 디스크 여유가 5~7GB라 ra_ap 빌드를
  하지 않았다 — `sem.rs`와 semantic feature 경로는 이 변경이 바꾼 함수를 부르지 않는다. CI `semantic-lint`·`msrv`),
  GLM 리뷰(`packet-ask` 없음).
- 자기 분석 기준(`rules --strict` 0, `cycles --level symbol --strict` 0, type 레벨 23·module 레벨 2, `self-dead.expected`
  0건), 하지 않기로 한 것(`query` 스키마 맞추기·`edge_evidence` 이름 바꾸기, 토큰·릴리스), persistence `relation-use`
  생산자 목록은 아래 2026-11-15 절 그대로 유효하다.

### 다음 할 일

1. 코디네이터가 리뷰 반영 커밋(`ca36e69`부터)을 [PR #60](https://github.com/ictechgy/rustograph/pull/60)에 push하고 본문을
   갱신한다(초안은 이 세션의 스크래치 `pr/rustograph-r5.md`). CI(특히 `msrv`·`semantic-lint`·coverage)를 확인한다.
2. **0.8.0 릴리스**(사용자 결정): 절차는 아래 3차 절 "다음 할 일" 2번과 같다.
3. isthmus `compatibility.json`의 rustograph 핀(현재 0.4.1) 갱신을 isthmus 쪽에 알린다.

### 후속 후보 (사용자가 고른다)

- **별칭을 거친 연관 함수 호출**(`type A = B; A::new()`): 이제 `B`는 별칭 간선으로 닿지만 `B::new`는 앞부분 별칭 아래에서
  메서드를 찾지 못해 거짓 dead일 수 있다(미해석으로 센다 — 4차 절 후보 그대로). 별칭 우변의 바깥 경로 하나(`Box<B>`면
  `Box`)만 따라가야 한다.
- **impl 연관 타입·별칭이 아닌 아이템의 bound·기본값·매크로가 만든 별칭**: `impl Iterator for X { type Item = Y; }`의
  `Y`, struct·enum·trait·fn 제네릭의 bound·기본값(`struct S<T: Tr = D>`)과 supertrait, `macro_rules!`가 펼치는
  `type Gen = OnlyMacro;`의 `OnlyMacro`는 수확하지 않는다 — 그 자리에서만 이름 붙는 타입은 dead에 나올 수 있고
  limitation으로 세지도 않는다(n4, main부터). 별칭의 것은 `f010e35`에서 고쳤다.
- **표현식 자리의 연관 상수·배리언트 경로**(재확인 리뷰, main부터 — 회귀 아님): 선언 타입의 표현식 자리에서
  `Type::ITEM`(연관 상수 `OnlyAssoc::K`, 배리언트 `E::B as usize`)은 타입을 살리지 못한다. 리뷰어 n11 `struct OnlyAssoc;
  impl OnlyAssoc { const K: usize = 2; } enum E { A, B } struct H { a: [u8; OnlyAssoc::K] } fn sig(x: [u8; E::B as
  usize]) -> usize`는 rustc가 아무것도 경고하지 않는데 모든 빌드가 `E`·`OnlyAssoc`을 낸다. 별칭의 const 기본값
  `{ Al::D }`도 같다. `resolve_decl_path`(`src/harvest.rs`)가 값 이름공간에서 `Type::ITEM`을 풀지 못해 아무것도 돌려주지
  않기 때문이다. 본문 방문처럼 해석되는 가장 긴 타입 앞부분을 잇거나(`BodyVisitor::path_prefix_owners`) limitation으로
  세면 된다.
- **windows 전용 별칭이 `#[cfg(unix)]` 임포트를 거치면** 간선 조건이 `all(unix, windows)`다(재확인 리뷰). 어느 빌드에도
  없는 조건이라 `--target`이 걸러 무해하지만, 글자로 보이는 모순(`P`와 `not (P)`)만 지우는 `drop_impossible_edges`는
  `unix`와 `windows`의 배타를 모르니 조건 없는 그래프에는 남는다.
- **매크로로만 쓰는 배열 길이 상수**(재확인 리뷰): 배열 길이 `[u8; len!()]`의 매크로 정의 본문에서만 이름 붙는 `LEN`은
  거짓 dead다 — 매크로 정의 본문은 두 세그먼트 이상의 경로만 추정 간선으로 모으는 기존 한계(main부터)다.
- **`deps`의 `#[cfg]`**: `deps`는 조건을 평가하지 않아 어느 빌드도 켜지 않는 조건 아래(`#[cfg(any())]`)에서만 이름 붙인
  의존도 쓰인 것으로 센다(필드·별칭 모두 — README에 적었다). 대상 빌드의 cfg 팩트로 간선을 거르는 것은 `--target`과
  같은 선택지가 필요하다.
- type 레벨에서 별칭과 그 우변 타입이 서로를 가리키면 순환이다(서로를 필드로 담는 두 struct와 같은 판단). 직접 재귀
  (`struct Node { next: Option<Box<Node>> }`)는 순환이 아니므로, 별칭을 투명하게 보고 싶다면 type 레벨 투영에서 별칭을
  우변으로 접는 선택지가 있다 — 출력 의미를 바꾸니 사용자 결정이다(리뷰 nit n1도 같은 것이다).
- 4차 절의 나머지 후보(타입 자리의 값 이름공간 폴백, 모든 빌드를 덮는 `#[cfg]` 재정의, 트레이트 기본 메서드를 타입으로
  부른 호출, `use E::*` 배리언트, `const _` 블록 안 아이템, 두 번 선언한 트레이트의 밖 타입 impl)와 3차 절 후보는 그
  설명대로 유효하다.

## 타입 경로 호출의 타입 참조·`const _` 루트·워크스페이스 밖 impl 트레이트 브랜치 (2026-11-22, 4차) — PR #58로 머지됨(`61e719b`)

- 발행본은 [v0.7.0](https://github.com/ictechgy/rustograph/releases/tag/v0.7.0)(2026-11-03, 자산 6개)이다.
  v0.6.0은 2026-10-25, v0.5.0은 2026-10-13에 발행됐다. v0.4.2는 태그 없이 v0.5.0으로 이어졌다.
- 3차 브랜치(cfg 대안 임포트·임포트 조건 전파·매크로 자리)는 [PR #57](https://github.com/ictechgy/rustograph/pull/57)로
  `main`에 스쿼시 머지됐다(`54b4224`, 아래 그 날짜 절).
- **브랜치 `ccr-37904670-hxdjsx`**(`origin/main` `54b4224` 위에서 다시 시작)에 아래 커밋이 있다. `ef2edb6`까지와 독립
  리뷰 반영(`91bae91`까지)은 코디네이터가 [PR #58](https://github.com/ictechgy/rustograph/pull/58)로 push했고(CI 7개 성공),
  델타 리뷰 반영(`9ce0e99`부터)은 push하지 않았다. 사용자 변화는 CHANGELOG `[Unreleased]`, 공개 Rust API 변화(더하기만)는
  [0.8 migration](docs/MIGRATING-0.8.md)에 있다. 셋 다 보존을 넓히는 쪽(간선·루트를 더함)이다.
  - `b375d62` fix(harvest): 값 경로로 풀리지 않은 호출 `T::m()`·`E::V(1)`·`T::default()`가 앞부분 타입으로
    references 간선을 단다(`BodyVisitor::type_relative_call`). 이전에는 메서드에만 잇거나(배리언트·derive 연관 함수는
    메서드 정점이 없어) 아무것도 잇지 않았다 — 호출이 아닌 값 경로(`visit_expr_path`)는 이미 타입을 이었다. 앞부분이
    워크스페이스 모듈이면 잇지 않는다(앞부분 대상과 미해석 셈은 아래 리뷰 반영에서 다시 정했다).
  - `44a4816` fix(harvest): 이름 없는 상수 `const _`가 보존 루트다(`#[used]` static과 같은 `entry_roots`). 테스트 빌드에만
    있는 것(자기나 조상 모듈 `#[cfg]`가 test=false에서 거짓)은 `--tests`일 때만(`graph::test_only_anonymous_consts`).
    루트 없는 lib limitation은 이 루트를 세지 않는다(`graph::Vertex::is_anonymous_const` — 공개 API에 더함).
  - `5d2cbb8` fix(harvest): self 타입이 워크스페이스 밖인 impl의 트레이트 경로를 `resolve_all`로 풀어 대안 트레이트마다
    선언 메서드가 본문을 대신 소유하고(추정 간선, 임포트 조건) 트레이트로 references 간선을 둔다. 재정의하지 않은 기본
    메서드도 그 간선을 갖는다(`inherited_trait_methods`). `foreign_impl_bodies`는 메서드마다 모듈 소유 본문 하나와
    대안마다 선언 메서드 소유 본문을 주고, 트레이트 간선 후보는 `ImplParts::foreign_trait_methods`(`ForeignTraitMethod`)로
    넘긴다 — source가 두 정점이 남았는지 보고 간선을 둔다(유령 정점 금지). 공개 `harvest::impls`의 결과는 그대로다.
  - `bf8909a` docs: README·README.ko 그래프 모델과 보존 루트, AGENTS.md 수확 의미론, CHANGELOG, 0.8 migration.
  - 이 HANDOFF 커밋.
- **독립 리뷰(PR #58, `ef2edb6` 대상 — push됨, CI 7개 성공) 반영** — 리뷰어 fixture(스크래치 `r4-review-rustograph/fx/`)를
  `cargo check`로 다시 확인하고 main(`54b4224`)·`ef2edb6`·고친 뒤 바이너리로 대조했다.
  - `48bf761` fix(harvest) — BLOCKER: 경로 앞부분이 함수로 넘어간 해석이면 잇지 않는다. 타입 이름공간에 묶인 것이 없으면
    `resolve_in`이 다른 이름공간으로 넘어가 `use std::process;` 옆 로컬 `fn process`가 `process::exit(1)`의 앞부분이 됐다.
    fx1(rustc 경고 `process`·`helper`·`fmt`): main 3건, ef2 0건(거짓 간선 `main → process`·`fmt`), 고친 뒤 3건. fx8(같은
    이름 재노출 옆 `env::var(..)`, rustc 경고 없음): main 0 순환, ef2 `cycle: fx8::a::env -> fx8::b::get`(`--strict` 1),
    고친 뒤 0. 값 경로 `let f = process::exit;`(fx12, rustc 경고 `process`)는 main부터 거짓이었다 — main·ef2 0건, 고친 뒤
    `process`. 앞부분은 부모 모듈의 `type_items`(`type_item()`)·모듈·외부 크레이트일 때만 쓴다(`path_prefix_owners`, 호출과
    값 경로 둘 다). 호출의 모듈 앞부분(`m::gen()`)은 메서드 후보도 잇지 않는다(main부터 `m::S::gen`으로 가던 거짓 call).
    말뭉치에서 잃은 간선은 getrandom `util::truncate → u32`·`u64` 둘뿐이다 — `u32::MAX`·`u64::from(..)`(원시 타입)이
    루트의 `fn u32`·`fn u64`로 가던 것(`→ u32`는 main부터). `t-modprefix`(rustc 경고 `u32`·`S`·`S::gen`)로 고정했다
    (`2404184` test).
  - `d35b31a` fix(harvest) — SHOULD-FIX 2: 재정의가 모두 자기 `#[cfg]`를 달면 기본 메서드의 트레이트 간선도 둔다. fx10
    (`impl Tr for u8 { #[cfg(unix)] fn m }` + `1u8.m()`, windows `cargo check` 경고 없음): `--target` windows에서 main·ef2
    `Tr`, 고친 뒤 0건.
  - `d19779f` fix(source) — SHOULD-FIX 1: 중첩 아이템을 품은 `const _` 루트를 limitation으로 센다("N `const _` retention
    roots contain nested items ..."). fx6(rustc 다섯 모두 경고): main 4건(상수 둘은 거짓), ef2·고친 뒤 0건 + limitation 2.
    간선을 상수에서 떼지 않은 근거: `const _: () = { const fn check() -> bool { helper() } assert!(check()); };`는 rustc
    경고가 없다(`t-nested`) — 떼면 `helper`가 거짓 dead다. 말뭉치에서 11개(ra_ap_hir_ty 7·zerocopy 3·rustix 1).
  - `9740c02` fix(harvest) — 리뷰 밖에서 찾은 것: 파일 없는 `mod gone;`의 이름이 아이템 표에 있어 `crate::gone::f()`(ef2)·
    `crate::gone::X`(main부터)가 정점 없는 경로로 references 간선을 냈다(유령 정점). change-impact 테스트(작업 트리에서
    지운 `src/target.rs`)가 다음 커밋의 셈 변경으로 드러냈다. `ModTree::unloaded_mods`(공개 필드 추가)가 그 경로를 적고
    앞부분 대상에서 뺀다.
  - `1fba4db` fix(harvest) — nit: 타입으로 풀린 타입 경로 호출(`E::V(1)`, `D::default()`)은 값 경로 `E::W`처럼 미해석으로
    세지 않는다. 말뭉치 합계 25,411 → 22,814(main·ef2는 같았다). 함수로 넘어간 앞부분·유령 경로·모듈 앞부분 호출은 센다.
  - `e64e90a` docs: README·README.ko·AGENTS.md·CHANGELOG·0.8 migration, 그리고 이 HANDOFF 커밋.
  - 별칭 우변(`type A = B; A::new()`가 `A`에만 닿음, fx4)은 기존 후속 후보 그대로다. fx2·fx3·fx9·fx11의 남은 거짓 dead
    (`Gen::mk`, `Self::Assoc::new()`의 `U`, 함수 안 타입이 가리는 모듈 타입, 제네릭 연관 타입)는 main부터 같다.
- **델타 리뷰(`ef2edb6..91bae91` — push됨, CI 7개 성공) 반영** — 리뷰어 fixture(스크래치 `r4-review2-rustograph/fx/`)를
  `cargo check`로 다시 확인하고 main·`91bae91`·고친 뒤로 대조했다(blocker 없음 판정).
  - `9ce0e99` fix(harvest) — S1: `1fba4db`가 타입으로 풀린 호출을 모두 셈에서 빼, 트레이트 기본 메서드를 타입으로 부른
    `c::S::d()`·별칭을 거친 `a::A::new()`(p2, rustc 경고 없음 — 세 버전 모두 `c::Tr::d`·`a::S`·`a::S::new`를 거짓 dead로
    냄)의 미해석 셈이 ef2 2 → 91bae91 0이 됐다. 이제 같은 이름 메서드에 닿았거나 그 이름의 워크스페이스 메서드가 없을 때만
    풀린 것으로 본다(`type_relative_resolved`) — p2는 다시 2.
  - `8cae59c` fix(harvest) — S2: 호출이 아닌 모듈 앞부분 경로(`let f = m::gen;`)가 `push_associated`로 `m::S::gen`을 이었다
    (p3, rustc 경고 `S`·`gen`: 세 버전 모두 `S::gen`을 살리고 거짓 순환 `back -> m::S::gen`, `--strict` 1). 이제 타입
    앞부분에서만 메서드 후보를 찾고 모듈 앞부분은 미해석으로 센다(`type_relative_value`) — `S::gen` 보고, 순환 0.
  - `edfe39a` fix(source) — nit (a): 싣지 않은 `mod`(p11 `#[cfg_attr(unix, path = ..)] mod backend;`, p4·p12
    `#[cfg(any())] mod missing;`)를 "N mod declarations were not loaded because their file was not found ..."로 세고, 그리로
    가는 `uses` 간선을 내지 않는다. `#[cfg]` 모듈 셈("modules behind #[cfg] were included")은 실은 모듈만이다(p4는 예전에
    싣지 않은 `missing`을 included로 셌다).
  - nit (b)는 기록만 했다: 타입 자리 해석에서 값 이름공간에만 있는 아이템을 빼는 `ModTree::resolve_want` 거르기를 만들어
    p9(rustc 경고 `u8`·`u16`·`u64`·`i32`·`process`)·p10(거짓 순환 `g -> u8`)을 고쳤지만, 말뭉치에서 참 간선을 잃었다 —
    tokio `WakeList → NUM_WAKERS`(필드 `[MaybeUninit<Waker>; NUM_WAKERS]`), zerocopy `Aligned64kAllocation → _64K`
    (`[u8; _64K]`), 그리고 zerocopy type 레벨 순환 하나가 사라졌다. 배열 길이·const 제네릭 인자(`Buf<N>`)가 타입 경로로
    파싱되면서 상수를 가리키니 그 폴백이 필요하다 — 함수만 빼려면 트리가 값 아이템 가운데 함수를 따로 알아야 한다. 그
    커밋은 히스토리에서 뺐다. nit (c)(모든 빌드를 덮는 `#[cfg]` 재정의 옆에도 기본 메서드 간선 — 보존이 넓기만 함)도 기록.
  - 말뭉치(`91bae91` 대비): 정점 ID 같음, 잃은 간선 89개 — 모두 rustix의 `uses → rustix::backend`(정점 없는 경로), 새 간선
    0, dead 여섯 변형 모두 변화 0, 순환 그대로. 미해석 합계 22,814 → 23,274, 싣지 않은 mod 2개(rustix), 정점 없는 경로로
    가는 간선 89 → 0. main 대비 누적: 잃은 간선 90(rustix 89 + getrandom `truncate → u32`, 둘 다 거짓), 새 간선 3,898, 새
    루트 72, 새로 낸 dead 0, 사라진 dead는 기본 321·`--tests` 103·`--retain-public` 72·리눅스 72·windows 68.
  - `11de8d6` docs: README·README.ko·CHANGELOG·0.8 migration, 그리고 이 HANDOFF 커밋.
  - 새 테스트 3개는 `91bae91`의 `src/`로 돌리면 3개 모두 실패하고, 변이 7개(이름 무시하고 타입이면 풀림·메서드에 닿을
    때만 풀림·모듈 앞부분에도 메서드 후보·모듈 앞부분 안 세기·싣지 않은 mod limitation 끄기·그리로 가는 uses 남기기·
    `#[cfg]` 모듈 셈에 싣지 않은 것도 넣기)를 모두 잡았다.
- **재현과 근거**(스크래치 fixture, 모두 `cargo check` 통과 — rustc 경고와 대조):
  - a1·t1: `imp::S::m()`·`imp::E::V(1)`·`Local::make()`·`imp::D::default()`. rustc 경고는 `Unused`와 배리언트 필드뿐인데
    main은 `S`·`E`·`D`·`Local`(·a1의 `U` — `Self::helper()`만 부르는 `U::other()`)을 unreachable로 냈다. 고친 뒤 `Unused`만(두 바이너리 모두 내는 아이템 위치 매크로 `gen::make`는 기존
    한계).
  - a3: `const _: () = assert!(only_in_anon());`, `const _: () = { assert!(<Marker as Check>::OK); };`, `mod inner { const _: usize =
    deep(); }`. rustc 경고는 `never` 하나인데 main은 8개(상수 셋·`only_in_anon`·`Marker`·`Check`·`inner::deep`·`never`)를
    냈다. 고친 뒤 `never`만. t5: `#[cfg(test)] const _`와 `#[cfg(test)] mod tests` 안 `const _` — `cargo check`는
    `only_test`·`helper_for_mod`를 경고하고 `--tests`는 경고 없음. 고친 뒤 `dead`는 그 둘과 상수를, `dead --tests`는 0건.
  - a2: `#[cfg(unix)] use crate::plat_unix::Tr;`/windows 쪽 + `impl Tr for String`. rustc 리눅스는 `plat_win::Tr`, windows는
    `plat_unix::Tr`을 경고하는데 main은 기본·리눅스·windows 모두 `plat_unix::Tr`(리눅스에서는 거짓)을 냈다. 고친 뒤 기본 0건,
    리눅스 `plat_win::Tr`, windows `plat_unix::Tr`. t3: `impl T1 for u8 {}` + `1u8.t1()`, 부르지 않는 `T2`, 재정의한 `T3::t3`
    + 기본 `t3_default` — rustc 경고 `T2`·`t3`. main은 `T1`·`T3`도 냈고, 고친 뒤 `T2`·`T2::t2`·`T3::t3`.
  - 말뭉치에서 사라진 dead 가운데 `const _`가 아닌 것을 rustc로 확인: rustix 1.1.5 `path::dec_int::I64_MAX_STR_LEN`(`const _`
    단언에서만 씀)·zerocopy 0.8.50 `pointer::is_zeroed`(`const _` 블록의 매크로 인자에서만 씀) — 두 크레이트 `cargo check`
    경고 0. ra_ap_hir_def `ExpandProcAttrMacros`(`pub(crate)`, `ExpandProcAttrMacros::get(db)`로만 씀)·ra_ap_hir_ty
    `infer::closure::analysis::UpvarArgs`(비공개 enum, `UpvarArgs::Closure(args)`로 만듦)는 코드로 확인했다(그 크레이트
    `cargo check`는 의존이 무거워 돌리지 않았다).
- **적대적 입력**(스크래치 `r4-rustograph/fx/adv1`~`adv4`, 리눅스·windows `cargo check`와 대조, main·새 바이너리 비교 —
  `dead` 기본·`--tests`·`--target` 리눅스/windows, `cycles` symbol·type·module):
  - adv1: impl 안 `Self::helper()`, 제네릭 `T::default()` 옆의 같은 이름 struct `T`(간선이 붙지 않는다 — 그대로 dead),
    트레이트 정적 호출 `Tr::m(self)`·`<Self as Tr>::m(self)`, 별칭을 거친 배리언트 `A::V(1)`, `Self::V(3)`. main은 `A`·
    `OnlyAlias`·`S`도 냈고 새것은 내지 않는다. `OnlyAlias::W(2)`로만 쓰는 `Only`는 rustc 경고가 없는데 main·새것 모두
    dead다 — 타입 별칭 우변(`type OnlyAlias = Only;`)에 간선이 없는 기존 한계(후속 후보). 순환은 모든 레벨 0.
  - adv2: 갈린 임포트 `S::m()`(리눅스 `w::S`·`w::S::m`만 — rustc와 같음), `use colors::Color::*;` 옆 `Red(1)`(`Color`는
    main·새것 모두 dead — 글롭 배리언트는 따로 남은 한계), 함수 안 `struct L`이 모듈의 `L`을 가리는 경우(main·새것 모두
    모듈 `L`을 닿은 것으로 본다 — rustc는 경고. 지역 impl의 self 타입 경로가 모듈 `L`로 풀리는 기존 한계).
  - adv3: `#[cfg(test)] const _`, `pub const _`, `const _: fn() = helper`, `const _X`·`const __`(루트가 아니다 — 그대로 dead),
    `const _: () = { impl Marker for Wrapped { .. } };`, 켜지지 않는 feature 뒤 `const _`. rustc는 중첩 impl만 쓰는
    `Wrapped`·`inside_impl`·`Marker`를 경고하는데 새것은 `Marker`만 낸다 — 블록 안 아이템을 그 상수가 소유하는 근사(함수 안
    아이템과 같은 규칙)로 보존이 넓다. 말뭉치에서 블록 안에 아이템을 둔 `const _`는 `is_copy` 단언 같은 형태뿐이었다.
  - adv4: blanket `impl<T: Display> Shout for T {}`(기본 메서드만 — 새것은 `Shout`을 내지 않는다, rustc 경고 없음),
    `#[cfg(unix)]`/`#[cfg(not(unix))]`로 두 번 선언한 트레이트(소스 인스턴스 ID라 간선이 붙지 않는다 — main·새것 모두
    dead, 기존 한계), `#[cfg]`로 갈린 재정의 메서드, 글롭 대안 `use crate::gu::*;`/`gw::*` 옆 `impl G2 for i64 {}`(리눅스
    `gw::G2`, windows `gu::G2` — rustc와 같음). 순환은 모든 레벨 0.
- **리뷰 반영 뒤 반례 말뭉치**(같은 17개, `ef2edb6` 바이너리 대 마지막 바이너리): 정점 ID 같음, 잃은 간선 2(getrandom
  `util::truncate → u32`·`u64` — 위 BLOCKER), 새 (from,to,kind,tentative) 간선 0, 새로 낸 dead 0·사라진 dead 0(여섯 변형
  모두), 순환은 세 레벨 모두 그대로. limitation: 미해석 경로 합계 25,411 → 22,814(타입으로 풀린 호출), 중첩 아이템을 품은
  `const _` 루트 11개를 새로 센다. main 대비 누적: 잃은 간선 1(getrandom `truncate → u32`, 원시 타입 `u32::MAX` — 거짓),
  새 간선 3,898, 새 루트 72, 새로 낸 dead 0, 사라진 dead는 아래와 같다(기본 321·`--tests` 103·`--retain-public` 72·리눅스
  72·windows 68), type 레벨 큰 강연결 요소 변화(아래) 말고 순환 그대로.
- **반례 말뭉치**(3차와 같은 크레이트 17개: syn 2.0.119·tokio 1.53.1·regex-syntax 0.8.11·ra_ap_hir_def/hir_ty 0.0.349·rowan
  0.17.0·serde/serde_derive 1.0.229·clap_builder 4.6.7·zerocopy 0.8.50·rustix 1.1.5·mio 1.2.3·getrandom 0.3.4·
  parking_lot_core 0.9.12·crossbeam-utils 0.8.23·hashbrown 0.16.1·이 저장소 `54b4224` 사본. `origin/main` 바이너리와 이
  브랜치 바이너리로 `graph --level symbol --tests`, `dead`(기본·`--tests`·`--retain-public`·`--tests --retain-public`·
  `--retain-public --target` 리눅스/windows), `cycles`(symbol·type·module, `--tests` 유무)를 돌려 비교 — 스크립트는 스크래치
  `r4-rustograph/scripts/`):
  - 정점 ID는 17개 모두 같다. (from,to,kind,tentative)로 잃은 간선 0, 새 간선 3,899개(모두 references — 추정 31개는 대신
    소유 본문 안의 타입 참조), 새 루트 72개(모두 `const _`: zerocopy 53·ra_ap_hir_ty 9·ra_ap_hir_def 7·rustix 3). syn의
    1개는 같은 쌍이 다른 조건으로 더해진 것이다(`impl ParseQuote for Vec<Attribute>`의 `ParseQuote::parse(input)`).
  - 커밋마다 나누면(커밋 하나씩 쌓은 바이너리로 같은 비교): `b375d62` 간선 +3,798·`dead --tests` −8·`--retain-public` −2,
    type 레벨 강연결 요소 변화(아래)도 이 커밋이다. `44a4816` 간선 변화 없음·루트 +72·기본 −321·`--tests` −93·
    `--retain-public` −70. `5d2cbb8` 간선 +101·`dead --tests` −2. 셋 모두 잃은 간선·새로 낸 dead 0.
  - 새로 낸 dead 0(여섯 변형 모두). 사라진 것: 기본 321, `--tests` 103, `--retain-public` 72, `--tests --retain-public` 72,
    리눅스 72, windows 68 — 크레이트별은 ra_ap_hir_def·ra_ap_hir_ty·rustix·zerocopy·clap_builder(`--tests` 2). 사라진 것이
    바꾼 limitation 문장은 셈만 바뀐다(루트 없는 lib의 공개 아이템 수가 줄고, 새로 닿은 타입의 외부 트레이트 impl 메서드 수가
    늘어난다).
  - 심볼·module 레벨 순환은 17개 모두 그대로다. type 레벨 순환 수도 그대로이고 ra_ap_hir_ty의 큰 강연결 요소가 325 →
    330개로 커졌다 — 새로 든 다섯(`autoderef` 모듈 단위·`AutoderefSnapshot`·`GeneralAutoderef`·`UpvarArgs`·`TlsData`)은
    `UpvarArgs::Closure(args)`·`TlsData::default()`·`GeneralAutoderef::new_from_inference_context(..)` 같은 실제 참조다.
  - tokio의 모순 조건 간선 limitation이 34 → 39개다 — 새 타입 참조 가운데 출발 모듈 조건과 모순인 것(같은 대안을 거친
    call 간선은 이미 지워지던 것)도 지운다.
  - 같은 크레이트에서 두 번 돌린 `graph`는 바이트 단위로 같았다(rustix·zerocopy).
- **검증**(2026-11-22, 로컬 cargo 1.97, `CARGO_PROFILE_DEV_DEBUG=0`·`-j 2`, `CARGO_TARGET_DIR`는 스크래치): `cargo fmt --check`,
  `cargo clippy --locked --all-targets -- -D warnings` 0, `cargo test --locked`(26개 실행 단위 593개 통과 — 이 브랜치의 새
  통합 테스트 15개, 델타 리뷰 반영 뒤 다시 잼), `scripts/coverage.sh` 94.85%(게이트 90%, 그 안의 테스트 실패 0 — 델타 리뷰 반영 뒤 다시 잼), `scripts/verify-cli-
  contract.sh <바이너리>` OK, `rules --strict` 0, `cycles --level symbol --strict` 0(type 23·module 2 — 변경 전과 같음),
  `scripts/verify-self-dead.sh <바이너리>` 0건 일치. 리뷰 반영의 새 테스트 6개와 셈을 바꾼 1개는 `ef2edb6`의 `src/`로 돌리면
  7개 모두 실패하고, 리뷰 반영 변이 9개(앞부분 거르기 빼기·값 경로만/호출만 거르지 않기·모듈 앞부분 메서드 후보 되살리기·
  재정의가 하나라도 있으면 기본 메서드 빼기·중첩 아이템 limitation 끄기·중첩 아이템 없는 상수도 세기·파일 없는 mod 거르기
  빼기·메서드에 닿았는지로 세기)를 모두 잡았다(호출만 거르지 않는 변이는 함수 앞부분에서는 `type_item` 거르기와 같은 동작이라
  파일 없는 mod 테스트가 잡는다). 4차 첫 테스트 6개는 `origin/main`의 `src/`로 돌리면 6개 모두 실패한다. 변이 10개(타입 간선
  빼기·모듈에도 잇기·타입이 풀리면 미해석으로 안 세기, `const _` 루트 끄기·루트 있는 크레이트로 세기·테스트 전용도 늘 루트·
  테스트 전용은 절대 루트 아님, 첫 트레이트 대안만·대신 소유 본문의 임포트 조건 버리기·기본 메서드 간선 끄기)를 모두 해당
  테스트가 잡았다.
  **실행하지 않음**: `--features semantic` test·clippy(지시에 따라 빌드하지 않았다 — sem.rs는 `cargo_meta`·`fanout`·`graph`만
  쓰고 이 변경은 `graph`에 메서드·crate 안 함수를 더할 뿐 sem이 쓰는 것을 바꾸지 않는다. CI `semantic-lint`), MSRV 1.96
  check(로컬 툴체인 없음 — CI `msrv`. 새로 쓴 표준 API는 `BTreeSet::range`·`Option::is_some_and` 정도로 오래전 안정화),
  GLM 리뷰(`packet-ask` 없음).
- **주의(작업 방식)**: 변이 검사용 스크래치 사본은 대상 디렉터리를 따로 쓴다(리뷰 반영에서는 `target-mut`). 같은 `CARGO_TARGET_DIR`로 빌드하면, cargo의 메타데이터
  해시가 워크스페이스 기준 상대 경로라 둘이 같은 산출물을 나눠 쓴다 — 사본을 빌드한 뒤 본 체크아웃을 그대로 빌드하면
  사본의(변이된) 라이브러리를 신선하다고 보고 재사용한다. 이번에 한 번 그렇게 틀린 테스트 결과를 봤다. 사본 빌드 뒤에는 본
  체크아웃의 `src`·`tests`를 `touch`하거나 대상 디렉터리를 나눠라.
- 자기 분석 기준(`rules --strict` 0, `cycles --level symbol --strict` 0, type 레벨 23·module 레벨 2, `self-dead.expected`
  0건), 하지 않기로 한 것(`query` 스키마 맞추기·`edge_evidence` 이름 바꾸기, 토큰·릴리스), persistence `relation-use`
  생산자 목록은 아래 2026-11-15 절 그대로 유효하다.

### 다음 할 일

1. 코디네이터가 이 브랜치를 push하고 PR을 연다(템플릿 제목 Change / Verification / Contract and policy — 본문 초안은 이
   세션의 스크래치 `pr/rustograph-r4.md`). CI(특히 `msrv`·`semantic-lint`·coverage)를 확인한다.
2. **0.8.0 릴리스**(사용자 결정): `[Unreleased]`가 Rust 라이브러리 호출자를 깨는 변경을 담으므로 마이너 버전이다. 절차는
   아래 3차 절 "다음 할 일" 2번과 같다.
3. isthmus `compatibility.json`의 rustograph 핀(현재 0.4.1) 갱신을 isthmus 쪽에 알린다.

### 후속 후보 (사용자가 고른다)

- 아래 3차 절의 후속 후보 가운데 이번에 고친 것은 셋이다 — 밖 타입 impl의 트레이트 해석이 대상 하나이던 것
  (`foreign_impl_bodies`), `Type::assoc()`만으로 쓰인 struct, 메서드가 모두 기본 구현인 트레이트(`impl T1 for u8 {}`).
  나머지(판 모델과 2018 판 이후의 크레이트 루트 폴백, routes의 대상 하나, 글자로 보이는 모순만 거르기, `_` 이름, 추정 간선
  note 문구, 이름공간 폴백, 그룹 안 `{self}`, 큰 글롭 고리의 다시 쌓기 비용, 글롭 쪽 조건 부정, 그래프 밖 대안)는 그 절의
  설명대로 유효하다. 고르지 않은 이유: 판 모델·폴백·`_`·폴백 좁히기는 보존을 좁혀 반례 말뭉치와 실제 프로젝트 근거가
  필요하고, note 문구는 텍스트 출력을 바꾼다.
- **타입 자리의 값 이름공간 폴백**(델타 리뷰 nit b): 타입 이름공간에 묶인 것이 없으면 `-> u8`·`x: u8`·`<u64>::max`가
  같은 이름의 로컬 `fn u8`로 간다(리뷰어 p9·p10 — 거짓 dead 누락과 거짓 순환, main부터). 값 이름공간 전부를 빼면 배열
  길이·const 제네릭 인자의 상수 참조(tokio `NUM_WAKERS`, zerocopy `_64K`)를 잃는다. `Module`에 함수 이름 표(또는 아이템
  종류)를 더해 함수·static만 빼면 된다 — 공개 필드가 늘어난다.
- **모든 빌드를 덮는 `#[cfg]` 재정의**(nit c): `#[cfg(unix)] fn m`·`#[cfg(not(unix))] fn m` 둘이 기본 메서드를 늘 가려도
  기본 메서드 간선을 둔다 — 보존이 넓기만 하다.
- **트레이트 기본 메서드를 타입으로 부른 호출·별칭을 거친 호출**(`S::d()`, `A::new()`): 앞부분 타입 아래에서 메서드를 찾지
  못해 그 메서드가 거짓 dead일 수 있다(리뷰어 p2 — 이제 미해석으로 센다). 구현한 트레이트의 선언 메서드와 별칭 우변을
  따라가면 고쳐진다.
- **타입 별칭 우변**: `type A = B;`에 `A → B` 간선이 없다 — `A::V(..)`·`A`로만 쓰는 `B`가 dead에 나온다(adv1의 `Only`,
  rustc 경고 없음). 별칭 선언을 필드처럼 `DeclRef`로 수확하면 된다(간선이 늘어나는 쪽 — 별칭이 많은 크레이트의 type 레벨
  순환은 말뭉치로 확인할 것).
- **`use E::*`로 들인 배리언트**(`Red(1)`): 글롭이 enum 배리언트를 펼치지 않아 `Color`가 dead에 나온다(adv2, rustc 경고
  없음). 9a79ce5부터 같다.
- **`const _` 블록 안 아이템**: 함수 안 아이템처럼 그 상수가 간선을 소유해, rustc가 중첩 아이템만 따로 미사용으로 보는
  경우(adv3의 `Wrapped`·`inside_impl`, 리뷰어 fx6)에도 닿은 것으로 남는다 — 이제 limitation으로 센다. 정확히 하려면 중첩
  아이템에 정점을 주고 초기식이 그것을 부르는지 봐야 한다(간선만 모듈로 옮기면 초기식이 부르는 중첩 `const fn`이 쓰는 것이
  거짓 dead가 된다). 함수 루트(`fn main` 안 아이템)도 같은 근사인데 세지 않는다.
- **두 번 선언한 트레이트의 밖 타입 impl**: `#[cfg(unix)] trait Dup`/`#[cfg(not(unix))] trait Dup` 옆 `impl Dup for u8 {}`는
  선언이 소스 인스턴스 ID라 대신 소유·트레이트 간선이 붙지 않는다(adv4 — main부터 같다).

## cfg 대안 임포트·임포트 조건 전파·매크로 자리 브랜치 (2026-11-22) — PR #57로 머지됨(`54b4224`)

- 발행본은 [v0.7.0](https://github.com/ictechgy/rustograph/releases/tag/v0.7.0)(2026-11-03, 자산 6개)이다.
  v0.6.0은 2026-10-25, v0.5.0은 2026-10-13에 발행됐다. v0.4.2는 태그 없이 v0.5.0으로 이어졌다.
- 2026-11-15의 이름공간 분리·derive 해석 브랜치는 PR #56으로 `main`에 스쿼시 머지됐다(`9a79ce5`, 아래 그 날짜 절).
  dependabot PR #55(같은 `taiki-e/install-action` 핀)가 닫혔는지는 이 세션에서 확인하지 않았다.
- **브랜치 `ccr-37904670-hxdjsx`**(`origin/main` `9a79ce5` 위)에 아래 커밋이 있다. 앞의 네 커밋(`187163b`까지)은
  [PR #57](https://github.com/ictechgy/rustograph/pull/57)로 push됐고, 독립 리뷰 반영(`826e20b`까지)도 push돼 CI 7개가
  성공했다(코디네이터 확인). 델타 리뷰 반영(`d480194`까지)도 push돼 CI 7개가 성공했다. 세 번째 델타 리뷰 반영 커밋
  (`3b43087`부터)은 push하지 않았다(2026-11-22). 사용자 변화는 CHANGELOG `[Unreleased]`, 공개
  Rust API 변화는 [0.8 migration](docs/MIGRATING-0.8.md)에 있다.
  - `55644b7` fix(modtree): 같은 이름공간에 같은 이름을 들이는 서로 다른 임포트 둘(명시 `use` 둘·글롭 둘)을 둘 다
    남기고(`Binding::Alternatives` — 첫 원소가 소스 순서상 뒤의 것), 임포트를 거친 해석이 거쳐 온 `use`·재노출·글롭의
    `#[cfg]`를 싣는다(`ModTree::resolve_all` → `Resolved { target, cfg }`, `with_import_cfg`). 본문(호출·참조·구조체
    리터럴·매크로)·시그니처·필드·속성 간선과 조건부 재노출을 `use`한 모듈의 `uses` 간선이 `resolve_all`을 쓴다.
    `resolve`·`resolve_in`은 그 첫 대상이다(대안의 첫 경로가 끊기면 다음 대안). 같은 정점을 들이는 명시 `use` 둘은
    글롭처럼 약한 조건으로 합친다. `use T as _` 여럿도 모두 남는다.
  - `8196e9a` fix(modtree): 매크로 자리에서는 이름공간을 정하지 못한 `use`가 같은 이름의 로컬 fn·trait이나 정해진
    타입·값 임포트를 이긴다(`undetermined_lookup`이 자리를 받는다). `fn info` 옆 `use log::info`의 `info!()`가 로컬 fn으로
    가는 확정 call 간선을 만들어 거짓 심볼 순환(`cycles --level symbol --strict` 종료 코드 1)이 나던 것을 고쳤다. 로컬
    fn은 추정 간선이라 dead는 그대로다.
  - `35ee8d5` docs: README·README.ko 그래프 모델, AGENTS.md 수확 의미론, CHANGELOG, 0.8 migration.
  - `187163b` docs: HANDOFF.
  - **독립 리뷰(PR #57, `187163b` 대상) 반영** — 리뷰어 fixture(스크래치 `fx/` fa·fb·fi·fj·fs·fu·fx1, 모두 `cargo check`
    통과)를 9a79ce5(old)·187163b(pr)·고친 뒤(new) 바이너리로 대조했다.
    - `4341405` fix(modtree) — B1(이 브랜치의 회귀): `use a::*;` 옆 `#[cfg(test)] use a::f;`에서 명시 `use`가 글롭 칸을
      바꿔 `f()` 간선이 `cfg(test)`를 달았다 — `dead --exclude-tests`·`--target x86_64-unknown-linux-gnu`가 old 0건,
      pr `unreachable fn: fa::a::f`, new 0건(rustc 경고 없음). 자기 `#[cfg]` 없는 명시 `use`가 차지한 칸만 글롭을
      가리고, 칸의 명시 `use`가 모두 자기 조건을 달면 글롭 칸과 합친다(글롭 쪽은 자기 조건 그대로). 같은 규칙으로
      S3의 fb(`use real::*;` + `#[cfg(test)] use mock::Clock;`, old·pr 기본 dead에 `real::Clock`·`::now`)가 new 0건.
      S2: 해석되는 임포트가 해석되지 않는 임포트(std)를 순서와 무관하게 이긴다 — `#[cfg(feature="pl")] use
      extdep::Mutex;` 뒤 `#[cfg(not(..))] use std::sync::Mutex;`면 old·pr `deps`가 `unused: fj -> extdep`, new 0
      unused(리뷰 설명과 달리 리뷰어 fixture fj의 줄 순서에서는 old·pr도 맞고, std가 뒤에 올 때 틀린다 — 스크래치
      `r3-rustograph/fx-fj2`). S3 fx1: `#[cfg(feature="x")] fn sum()` + `#[cfg(not(..))] use scalar::sum;`면 old·pr 기본
      dead에 `scalar::sum`, new 0건(`Module::cfg_gated`, `Found::LocalOr`). N2: 명시 `use BExt as _`가 글롭의 `_`
      (`AExt`)를 가렸다 — old·pr `user`의 uses가 `BExt`뿐, new는 `AExt`도.
    - `4dc0c53` fix(harvest) — S1: 갈린 임포트를 self 타입으로 쓴 `impl Sys { fn extra() }`를 대안마다 수확한다.
      `--target` 리눅스: old `unix_impl::Sys`(거짓 — 리눅스에서 쓰인다), pr `win_impl::Sys`·`win_impl::Sys::extra`
      (뒤는 거짓), new `win_impl::Sys`만(rustc 리눅스 경고 5행과 같음). windows는 셋 다 `unix_impl::Sys`만.
    - `c463667` fix(modtree) — S4: 수렴 중 어느 모듈 표가 해석된 대상을 거두면 언 표로 명시 `use`·글롭 대상을 풀며
      빈 표에서 다시 수렴시킨다. fs: old는 `main → fs::foo::bar`(거짓)만 잇고 dead에 `real::foo::bar`(거짓), pr은 두
      call을 다 잇고 dead 0건, new는 `main → real::foo::bar`만 잇고 dead에 `fs::foo::bar` — rustc가 9행 `bar`를 쓰이지
      않는다고 경고하는 그 정점이다(거짓 간선으로만 살던 것이라 보존이 좁아진다).
    - `2021c62` test — N1: 살아남은 변이 셋(`attr_edges`·`decl_edges`가 임포트 조건을 버림, `push_associated`가
      `&None`)을 잡는 속성·필드 타입·값으로 넘긴 연관 함수 테스트.
    - `08d4ccf` docs — N3: "`--target`은 그 빌드의 것만 남긴다"를 실제 동작으로, 보존 축소에 거둔 대상을 더했다.
    - 새 통합 테스트 9개 가운데 7개는 187163b 코드에서 실패한다(N1 둘은 187163b에서도 통과 — 변이용). 고친 것마다
      되돌리는 변이(늘 가리기·섞이면 뒤의 것·로컬 대안 끄기·첫 대안만 impl 수확·다시 수렴 끄기·`_`도 가리기·N1 셋)를
      각각 해당 테스트가 잡았다.
  - `826e20b` docs: HANDOFF.
  - **델타 리뷰(`187163b..826e20b`) 반영** — 리뷰어 fixture(스크래치 `r3-review2-rustograph/fx/`, 모두 `cargo check`
    통과)를 9a79ce5·187163b·826e20b·고친 뒤로 대조했다. 기준: 187163b·9a79ce5보다 새로 내는 dead는 rustc가 동의할 때만,
    새 거짓 확정 간선은 없게.
    - `337a1d4` fix(modtree) — 1(826e20b 회귀): 글롭 이름을 선언 여럿인 `#[cfg]` 로컬 아이템의 대안으로 봐 g1에서 거짓
      call `m::run → sys`와 거짓 심볼 순환, g2에서 `user::run → common::open` 거짓 간선이 났다(rustc: 루트 `sys`·
      `common::open` 미사용). 대안을 `Module::local_alternatives`로 따로 고른다 — 명시 `use`는 늘, 글롭 이름은 선언 하나 +
      대상이 조건부가 아닐 때만 `not (..)`을 달고. g1·g2 결과가 187163b와 같아졌다(`dead`에 `g1::sys`·`g2::common::open`,
      순환 0). 선언 하나(g2 양성)는 `--target` 리눅스에서만 `common::open`(rustc 리눅스 경고와 같음). 2: `#[cfg] mod`가
      자식 모듈 표 때문에 늘 조건 없는 선언으로 보였다 — gm의 `other::f`·`other::S::m`이 old·187163b·826e20b 모두 dead,
      이제 아니다(rustc 경고 없음). 6: 같은 조건의 글롭은 가린다 — sc에서 826e20b의 거짓 `main → a::f`가 없어지고
      187163b처럼 `a::f`를 낸다(rustc `--features x` 경고).
    - `6cc8c51` fix(harvest) — 3: 트레이트 경로도 대안마다 implements. tp: old·187163b·826e20b는 기본·리눅스 모두
      `unix_impl::Tr`(거짓), 이제 기본 0건·리눅스 `win_impl::Tr`(rustc 리눅스 경고 7행).
    - `23e7330` fix(modtree) — 4: 2015 판에서는 거두지 않는다. e15: 826e20b가 새로 낸 `e15::foo::bar`가 사라지고
      187163b처럼 0건 + limitation. e15b는 모든 판에서 `e15b::foo::bar`(거짓 — 2015 판 rustc는 `real::foo::bar` 미사용)를
      그대로 내고 이제 limitation이 붙는다. 5: 다시 쌓기 32번 + 지금 표 확인 바퀴. s4c3~s4c5가 826e20b의 틀린
      "still changed" 문구와 남은 거짓 간선 없이 rustc처럼 루트 `foo::bar`만 낸다. 7: qring150s 826e20b 18.1초 → 약
      12초(187163b 7.0초), qring400s 144 → 91초(187163b 50초).
    - 변이: M1(다시 쌓기 한 번)·M3·M4·M5·M6·M7·M8은 새 테스트가 잡고, M2(글롭 대상 조건)는 같은 동작이라 코드를 뺐다.
      이번 고친 것마다의 변이(글롭 대안을 선언 여럿에도·부정 조건 빼기·자식 표를 앞선 선언으로·첫 트레이트만·판
      무시·함축 끄기)와 B1 변이도 다시 잡았다. 새 통합 테스트 10개 가운데 7개는 826e20b 코드에서 실패한다(나머지
      셋은 826e20b에서 통과하는 변이용).
  - `5fa8bf5` docs, `d480194` docs: HANDOFF.
  - **세 번째 델타 리뷰(`826e20b..d480194`) 반영** — 리뷰어 fixture(스크래치 `r3-review3-rustograph/fx/`, 리눅스·
    `--target x86_64-pc-windows-msvc` 모두 `cargo check` 통과)를 9a79ce5·187163b·d480194·고친 뒤로 대조했다.
    - `3b43087` fix(modtree) — BLOCKER: 보일 수 없는 대안이 확정 간선이 됐다. triple: d480194는 `m::run → a::sys`로 거짓
      순환 `a::sys → m::run`(기본·windows+`--exclude-tests` 모두, 9a79ce5·187163b는 0)을 냈다 — 이제 순환 0, `dead`는
      `a::sys`(rustc 리눅스·windows 모두 `a::sys` 미사용 — 9a79ce5·187163b는 windows에서 쓰이는 `b::sys`도 거짓으로 냈다).
      samec: 같은 조건 글롭의 `all(feature = "x" , not (feature = "x"))` 간선과 순환 `m::run → plat::sys`가 사라졌다.
      site·site2(d480194)·site3(187163b부터): 쓰는 자리 조건과 모순인 간선 `m::run → plat::sys`와 그 순환이 사라졌다.
      tokio: `mocked` 안팎의 모순 간선 34개가 빠지고 module 레벨 순환이 9a79ce5와 같은 1이다. 글롭 대안은 글롭 조건이
      아이템 조건을 함축하거나 같은 칸의 명시 `use`가 가리면 버리고(`local_alternatives`), 간선 조건·출발 정점 조건·그
      모듈과 조상 모듈의 `mod` 조건에 글자로 `P`와 `not (P)`가 있으면 수확 끝에 지우고 센다(`drop_impossible_edges`,
      `modtree::cfg_contradicts`).
    - `609d245` fix(modtree) — 2015 판 limitation이 참이 아니었다("어느 대상으로든 갈 수 있다" — e15b는 이른 대상을
      잃고, e15c는 limitation 없이 조용히 틀렸다, 둘 다 9a79ce5와 같음). 이제 루트가 아닌 모듈의 `use` 경로가 크레이트
      루트가 선언한 이름으로 시작하는 2015 판 크레이트를 모두 세고, "2018 판 규칙으로 풀어 `dead`가 그 경로로만 쓰는
      아이템을 낼 수 있다"고 쓴다. 다시 쌓을 때 2015 판 모듈도 비운다(M5가 같은 동작이었다 — 그 모듈은 지금 표로 다시 풀어
      이른 대상을 다시 얻는다).
    - 변이: M1(함축 방향 뒤집기 → sc2의 `a::f` 거짓 dead)·M2(조건부 글롭 대상 거르기 빼기 — 쓰는 자리가 `#[cfg(kani)]`인
      kani2)·M5(2015 판 모듈을 언 표로 풀기 — 2018 판 멤버가 다시 쌓기를 일으키는 혼합 워크스페이스)·M7(남은 수 안 세기 —
      상한을 받는 `fill_imports_settled_within` 단위 테스트)을 새 테스트가 잡는다. 이번 고친 것의 변이(같은 칸 명시
      `use` 가림 빼기·모순 거르기 끄기·조상 모듈 조건 빼기)도 잡고, 같은 조건 글롭 거르기 빼기는 간선 거르기와 겹쳐 표
      수준 단위 테스트로 잡는다. 새 통합 테스트 7개 가운데 4개(BLOCKER 셋, 2015 판 문구)는 d480194 코드에서 실패한다.
  - `4d8b358` docs, 이 HANDOFF 커밋.
- **재현과 근거**(스크래치 fixture, 모두 `cargo check` 통과):
  - `#[cfg(unix)] use crate::unix_impl::Sys;`/`#[cfg(windows)] use crate::win_impl::Sys;` + `Sys::open()`: main은
    `start → win_impl::Sys::open`만 조건 없이 내고 `dead --retain-public`(`--target x86_64-unknown-linux-gnu`에서도)에
    `unix_impl::Sys::open`을 냈다. 고친 뒤 두 call 간선(`unix`·`windows`), 기본 dead 0건, `--target` 리눅스에서
    `win_impl::Sys`·`::open`만 — rustc 리눅스 빌드의 dead_code 경고(11·13행)와 같다.
  - 플랫폼 글롭 둘(`#[cfg(unix)] pub(crate) use self::unix::*;`)·모듈 별칭 둘(`use unix_sys as platform;`)·
    `#[cfg(test)] use crate::fake::Clock;`/`#[cfg(not(test))] use crate::real::Clock;`도 같은 모양이었다(main은 뒤의 것만).
  - `#[cfg(feature = "pl")] use crate::pl::Condvar;` + `Condvar::new()`: main은 call·signature 간선이 조건 없고 `uses`만
    `feature = "pl"`이었다. 이제 모두 싣는다(조건부 재노출 `crate::sync::Condvar::new()`, 그것을 `use`한 모듈 포함).
  - `use log::info`(또는 `use std::println`) 옆 `fn info() { run() }`과 `run() { info!(..) }`: main은 `1 cycle(s)`·종료 코드
    1, rustc는 `function info is never used`. 고친 뒤 0 cycle(s), 추정 간선 1개를 note로 센다.
- **반례 말뭉치**(크레이트 17개: syn 2.0.119·tokio 1.53.1·regex-syntax 0.8.11·ra_ap_hir_def/hir_ty 0.0.349·rowan 0.17.0·
  serde/serde_derive 1.0.229·clap_builder 4.6.7·zerocopy 0.8.50·rustix 1.1.5·mio 1.2.3·getrandom 0.3.4·parking_lot_core
  0.9.12·crossbeam-utils 0.8.23·hashbrown 0.16.1·이 저장소 `9a79ce5` 사본, `graph --level symbol --tests` 간선 키
  from·to·kind·tentative·cfg, 마지막 바이너리 기준): main 대비 cfg를 빼고 사라진 간선 0, 새 (from,to,kind,tentative)
  92개(tokio 44·zerocopy 38·rustix 6·syn 4 — 187163b 대비로는 zerocopy `r#ref::tests`의 `_` 칸 uses 5개만 늘었다), 정점 ID는 9a79ce5·187163b와
  모두 같고, 조건 없던 간선이 조건을 얻은 것 342개(rustix 199·syn 91·clap_builder 35·tokio 6·parking_lot_core 4·
  zerocopy 4·이 저장소 3 — 이 저장소는 `#[cfg(feature = "semantic")] use crate::sem::..`를 거친 3개). `dead --tests` 차이 0,
  `dead --tests --retain-public` 2건 덜 냄(syn `ext::PunctExt`, zerocopy `util::polyfills::NonNullExt` — 둘 다 `use .. as _`
  여럿), `dead --retain-public` 13건 덜 냄(그 둘과 rustix `ioctl::linux::compose_opcode`·상수 10개). 새로 낸 것 0.
  심볼·type·module 레벨 순환 수는 모든 크레이트에서 9a79ce5와 같다(187163b·d480194의 tokio module 레벨 1 → 2는 조건이
  모순인 간선을 지워 1로 돌아왔다). 마지막 바이너리 기준 9a79ce5 대비 잃은 간선은 tokio 6개뿐이고 모두 출발 모듈의 조건과
  모순이라 어느 빌드에도 없다(9a79ce5에는 조건 없이 있었다 — `loom::std::barrier::Barrier::new → loom::mocked::sync::
  Mutex::new` 등). 새 (from,to,kind,tentative) 64개, 조건을 얻은 간선 339개, 지운 모순 간선은 tokio 34개. 매크로 자리 규칙은 말뭉치에서 바뀐 간선이 없고, 수렴을 다시 돈 크레이트도 없다. 같은 크레이트에서 두 번 돌린
  `graph`는 바이트 단위로 같았다(ra_ap_hir_def·syn·rustix).
- **검증**(2026-11-22, 로컬 cargo 1.97, `CARGO_PROFILE_DEV_DEBUG=0`·`-j 2` — 공유 디스크가 모자라 디버그 정보를 껐다):
  `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings` 0, `cargo test --locked`(26개 실행 단위
  578개 통과 — 이 브랜치의 새 통합 33·단위 10), `rules --strict` 0, `cycles --level symbol --strict` 0(type 23·module 2 — 변경 전과
  같음), `scripts/verify-self-dead.sh`(0건 일치), `scripts/verify-cli-contract.sh` OK, `scripts/coverage.sh` 94.83%(게이트 90%, 그 안의 테스트 실패 0 — 세 번째 델타 리뷰 반영 뒤 다시 잼).
  새 통합 테스트 7개는 `main`의 `src/`로 돌리면 모두 실패하고, 변이 6개(대상이 다른 임포트를 뒤의 것만, 경로 조건 버림,
  재노출 조건 버림, 대안 중 첫 경로만 걷기, 같은 대상의 조건을 뒤의 것으로, 본문 간선의 경로 조건 버림·연관 함수 호출의
  첫 타입만)와 매크로 자리 규칙 되돌리기는 모두 테스트가 잡았다.
  **실행하지 않음**: `--features semantic` test·clippy(sem.rs는 `cargo_meta`·`fanout`·`graph`만 쓰고 이 변경은 그 셋과
  feature 경로를 바꾸지 않는다. ra_ap 빌드가 남은 디스크(약 5GB)보다 클 수 있다 — CI `semantic-lint`), MSRV 1.96 check
  (로컬 툴체인 없음 — CI `msrv`. 새로 쓴 표준 API는 `bool::then_some`·`slice::from_ref` 정도로 오래전 안정화),
  GLM 리뷰(`packet-ask` 없음).
- 자기 분석 기준(`rules --strict` 0, `cycles --level symbol --strict` 0, type 레벨 23·module 레벨 2, `self-dead.expected`
  0건), 하지 않기로 한 것(`query` 스키마 맞추기·`edge_evidence` 이름 바꾸기, 토큰·릴리스), persistence `relation-use`
  생산자 목록은 아래 2026-11-15 절 그대로 유효하다.

### 다음 할 일

1. 이 브랜치를 push하고 PR을 연다(템플릿 제목 Change / Verification / Contract and policy — 본문 초안은 이 세션의
   스크래치 `pr/rustograph-r3.md`). CI(특히 `msrv`·`semantic-lint`·coverage)를 확인한다.
2. **0.8.0 릴리스**: `[Unreleased]`가 Rust 라이브러리 호출자를 깨는 변경(필드 추가, `modtree::Module::imports` 타입
   변경, `unresolved_imports` 제거)을 담으므로 마이너 버전이다. 버전 커밋 → PR CI → 머지 → 태그 → Release 자산 확인 →
   (토큰이 없으면) 수동 tap 갱신(아래 "다음 단계" 1번) → 설치 바이너리로 CLI 계약·자기 분석.
3. isthmus `compatibility.json`의 rustograph 핀(현재 0.4.1) 갱신을 isthmus 쪽에 알린다.

### 후속 후보 (사용자가 고른다)

- 2018 판 이후의 크레이트 루트 폴백: 첫 세그먼트가 그 모듈에 묶이지 않으면 크레이트 루트의 같은 이름 아이템으로
  넘어간다(`ModTree::resolve_unbound`) — 2015 판 `use` 경로의 의미다. 2018 판부터 `use`·본문 경로의 첫 세그먼트는 그
  모듈의 이름이나 외부 크레이트다. tokio `tokio::loom::mocked`의 `use loom::sync::*`(외부 크레이트 `loom`)가
  `tokio::loom`으로 읽힌다 — 그렇게 생긴 대안 간선 가운데 모순인 것은 이제 지워지지만(`3b43087`) 폴백 자체는 그대로다.
  고치려면 타깃마다 판을 알아야 하고 간선이 줄어드니 반례 말뭉치 근거가 필요하다.
- routes(`source/routes`)와 self 타입이 워크스페이스 밖인 impl의 트레이트 해석(`foreign_impl_bodies`)은 아직 대상
  하나다 — 대안 임포트를 거치면 첫 대상뿐이고 임포트 조건을 싣지 않는다. self 타입·트레이트 경로는 대안마다다.
- 판(edition) 모델: 해석은 2018 판 이후 규칙이다. 2015 판 크레이트의 크레이트 기준 `use` 경로(`use foo::bar` =
  `crate::foo::bar`)를 모른다 — 지금은 거둔 대상을 지우지 않고 limitation으로 셀 뿐이고, e15b처럼 첫 바퀴 대상을
  잃는 경우(`dead`에 거짓 `foo::bar`)는 9a79ce5부터 그대로다. 2018 판 이후의 크레이트 루트 폴백(아래)과 함께, 타깃
  판(`cargo_meta::Target::edition`)을 해석에 넘기는 것이 다음 단계다.
- 오래된 씨앗이 있는 큰 글롭 고리(합성 qring400s)는 다시 쌓기 때문에 9a79ce5·187163b보다 약 1.8배 느리다(50초 →
  94초) — 실제 크레이트 17개는 다시 쌓지 않는다.
- 대안이 없는 경우(9a79ce5와 같음): `#[cfg]` 로컬 아이템의 선언이 둘 이상이거나 글롭 대상이 스스로 조건부면 글롭 이름은
  대안이 아니다. `#[cfg_attr(p, cfg(q))]`는 조건으로 모델하지 않는다. `gated_vertex`는 소스 인스턴스 ID(`name#N`)를
  보지 못한다. 모순 거르기는 글자로 보이는 `P`와 `not (P)`만 본다(`unix`와 `windows`, `not (any (..))` 펼치기는 보지
  않는다 — 그런 간선은 남는다).
- `Type::assoc()`만으로 쓰인 struct(`S::m()` — 생성하지 않음)는 `dead`에 나온다 — rustc는 그 경로를 struct의 사용으로
  본다(gm의 `imp::S`·`other::S`, condstd의 `real::Clock`). 9a79ce5부터 같다. 연관 함수 호출에서 타입으로 references
  간선을 더하면 고쳐진다(간선이 늘어나는 쪽).
- 자기 `#[cfg]`를 단 명시 `use`와 합친 글롭 쪽은 자기 조건만 싣는다 — 명시 `use` 조건의 부정(`not(test)`)을 더하면
  그 조건의 빌드에서 가려지는 간선까지 정확해진다(보수적으로 두었다).
- 모든 선언이 `#[cfg]`를 단 로컬 아이템 옆의 이름공간을 정하지 못한 `use`(그래프 밖 대안)는 대안으로 보지 않는다 —
  본문 간선은 로컬 아이템만이고 `uses` 증거는 남는다.
- 메서드가 모두 기본 구현인 트레이트(`impl T1 for u8 {}`)는 그 메서드가 이름 팬아웃으로 닿아도 트레이트 정점이
  unreachable로 나온다(리뷰어 fg: `use c::T1 as _;` + `1u8.t1()` — rustc 경고 없음). old·pr·new 모두 같다. 기본 메서드에서
  트레이트로 가는 간선이 없어서다.
- cycles·rules의 추정 간선 note 문구("speculative method-fan-out edges")는 매크로 경로의 추정 간선도 센다 — 문구를
  바꾸면 텍스트 출력이 바뀐다.
- `_` 이름(리뷰 r2 N3): `use X as _` 여럿은 이번 브랜치에서 모두 `uses` 증거로 남는다. 남은 것은 `_` 칸이 이름 해석
  표에 있어 글롭으로 퍼지는 것(zerocopy `NumExt` uses 29)과 `const _: () = ..`가 아이템 표에 들어가는 것(`const _`
  uses 7)이다 — rustc에서 `_`는 이름을 묶지 않으니 `const _`는 표에 넣지 않는 쪽이 맞다. 간선이 줄어드니 반례 말뭉치
  대조가 필요하다.
- 이름공간 폴백이 이름공간이 정해진 바인딩으로 넘어간다(리뷰 r2 S2, 회귀 아님): 자리의 이름공간에 묶인 것이 없으면
  자리를 모르는 해석으로 넘어가, `use super::*`가 `pub mod vec`을 들인 모듈의 `vec![..]`가 모듈 `vec`으로 가는 확정
  call 간선이 된다. 폴백이 정말 필요한 것은 아이템 표가 값 이름공간에만 둔 `#[proc_macro]`·`#[proc_macro_attribute]`
  함수 정도다. 폴백을 좁히면 보존이 줄 수 있어 반례 말뭉치 근거가 필요하다.
- 그룹 안 `{self}`(리뷰 r2 N2): `use a::b::{self}`는 rustc에서 타입 이름공간만 묶지만 `self` 표시를 버린 뒤 `a::b`로
  해석해 `b`가 차지한 모든 이름공간에 묶는다(드물다).
- routes의 경로 해석과 `macro_rules!` 본문 경로는 아직 이름공간을 넘기지 않는다(`ModTree::resolve`). 같은 이름이
  이름공간마다 다른 정점에 묶인 드문 경우에만 차이가 난다.
- Homebrew tap 자동 갱신은 `HOMEBREW_TAP_TOKEN` 리포 시크릿이 있어야 돈다 — 없으면 Release가 그 단계를
  건너뛰므로 릴리스마다 로그로 확인하고, 수동 절차는 아래 "다음 단계" 1번을 따른다.
- 계열 공통: 이 저장소의 `scripts/verify-self-dead.sh`·`verify-cli-contract.sh`는 인자 없이 돌리면 `cargo build`를
  기본 프로필·기본 병렬도로 부른다. 공유 머신에서는 빌드한 바이너리 경로를 인자로 넘겨라.

## 이름공간 분리·derive 해석 브랜치 (2026-11-15) — PR #56으로 머지됨(`9a79ce5`)

- 발행본은 [v0.7.0](https://github.com/ictechgy/rustograph/releases/tag/v0.7.0)(2026-11-03, 자산 6개)이다.
  v0.6.0은 2026-10-25, v0.5.0은 2026-10-13에 발행됐다. v0.4.2는 태그 없이 v0.5.0으로 이어졌다.
- **브랜치 `ccr-37904670-hxdjsx`**(`origin/main` `e12ef90` 위)에 지난 HANDOFF(2026-11-10)의 후속 후보 셋과
  dependabot PR #55를 처리한 커밋 4개, 첫 HANDOFF 커밋(`51eb85a`), 독립 리뷰(r2) 반영 커밋 5개와 이 HANDOFF 커밋이
  있다. push·PR·머지는 하지 않았다(2026-11-15). 사용자 변화는 CHANGELOG `[Unreleased]`, 공개 Rust API 변화는
  [0.8 migration](docs/MIGRATING-0.8.md)에 있다.
  - `a3c01a6` fix(modtree): 모듈 임포트 표가 이름 → 이름공간(타입·값·매크로)별 바인딩(`PerNs<Binding>`)이다. 글롭은
    대상 모듈의 이름을 이름공간마다 따로 들이고, 명시 `use`·로컬 아이템은 같은 이름공간의 글롭만 가린다. 명시
    `use a::x`는 `x`가 `a`에서 차지한 이름공간을 모두 들인다. 해석은 자리의 이름공간이 먼저고(`ModTree::resolve_in` —
    호출·값 경로는 값, 타입 자리·구조체 리터럴·impl self 타입은 타입, 매크로 호출·속성은 매크로), 그 이름공간에 묶인
    것이 없으면 예전처럼 이름공간을 가리지 않고 찾는다(아이템 표가 모르는 이름공간에서 해석을 잃지 않으려고).
    자기 분석 dead의 알려진 2건이 사라져 `scripts/self-dead.expected`가 비었다. 라이브러리 API: `Module::imports` 타입
    변경, `unresolved_imports` 제거(`Binding::Unresolved`로), `macro_items`·`Ns`·`PerNs`·`Binding`·`resolve_in` 추가.
  - `1d3eb68` fix(harvest): 속성·derive 인자·아이템 위치 매크로 호출의 경로를 매크로 이름공간에서 해석한다 — 다른
    크레이트의 `#[derive(pm::Hello)]`가 proc-macro 크레이트의 같은 이름 비공개 struct가 아니라 derive 함수로 간다.
    그 struct의 보존은 좁아진다(근거: proc-macro 크레이트는 매크로 말고는 내보낼 수 없다 — cargo check 오류로 확인).
  - `c95c4e8` docs: README·README.ko 그래프 모델과 0.8 migration에 필드 간선을 심볼 레벨 순환은 빼고(어느 코드가
    재귀하는지) type·module 레벨은 세는(계층으로 나눌 수 있는지) 근거를 나란히 적었다. tests/recursion.rs에 module
    레벨 특성 테스트를 더했다.
  - `25b3163` ci: `taiki-e/install-action` v2.87.30 → v2.90.3(`cfdb7ed4ccc7c7dce37759eae89dc5674b469b39`, 경량 태그 —
    `git ls-remote`·`cat-file -t`로 확인). PR #55의 diff와 같은 줄이라 이 브랜치가 머지되면 #55를 닫는다.
  - **독립 리뷰 r2**(맥락을 물려받지 않은 Claude 리뷰, `51eb85a` 대상): 판정 "수정 후 머지" — B1(blocker), S1·S3·S4는
    고쳤고, S2는 후속 후보로, N1·N4는 고쳤고, N2·N3은 후속 후보로 남겼다. 지적마다 리뷰어 fixture로 이전(`e12ef90`)·
    PR(`51eb85a`)·수정 후 바이너리를 돌려 재현한 뒤 고쳤다.
    - `abea95b` fix(modtree) — B1: 이름공간을 정하지 못한 명시 `use`(외부 크레이트·std 경로, zerocopy처럼 매크로로
      등록한 derive 이름)가 `PerNs::all`로 세 칸을 다 차지해, 같은 모듈의 `pub trait Foo` 옆 `pub use foo_derive::Foo`가
      값 자리·매크로 토큰의 `Foo`를 가져갔다(`check → Foo` 간선 소실, `--deps`면 외부 크레이트로 가는 확정 간선).
      이제 `PerNs::undetermined` 칸에 두고, 같은 이름의 로컬 아이템이나 이름공간이 정해진 바인딩이 있으면 어느 자리에서도
      이기지 않는다(rustc E0255·E0252 — 그런 `use`는 다른 이름공간에 있다). 자리를 모르는 해석은 정해진 임포트를 먼저
      고르고, 문법을 모르는 매크로 인자 토큰은 값 이름공간이 아니라 자리를 모르는 해석이다(리뷰 제안 3가지 그대로).
      그런 `use`는 지금처럼 같은 이름의 글롭을 모두 가린다(`use std::fmt::Result` 규칙 유지). 덤으로 같은 이름의 정해진
      명시 `use`가 뒤의 미정 `use`에 가려지지 않는다(zerocopy `SplitAt`, syn·tokio의 `uses` 6개). 단 `#[cfg]`로 갈린
      대안 임포트(tokio의 `parking_lot::Condvar`/`std::sync::Condvar`)는 같은 이름공간이라 E0252 근거가 닿지 않는다 —
      소스 순서와 무관하게 워크스페이스 쪽이 이기고(이전에는 뒤의 것), 그 이름을 쓰는 본문 간선은 임포트 cfg 없이 확정
      간선이다(본문 간선이 임포트 cfg를 물려받지 않는 것은 전부터다 — 후속 후보).
    - `c9f6951` fix(modtree) — N1: 글롭 대상 경로를 타입 이름공간에서 해석한다(`fn harvest` 옆 `use crate::harvest`가
      있는 모듈의 `use super::harvest::*`가 fn을 잡아 아무것도 들이지 않았다).
    - `229a0a0` fix(harvest) — S1: 매크로 경로(속성·derive 인자·매크로 호출)의 매크로 이름공간 대상이 자리를 모르는
      해석과 다르면 그 예전 대상에 추정 간선을 더한다(dead·도달성 전용). `mylib`이 trait `Hello`와 `pub use
      mylib_derive::Hello`를 함께 내보내는 모양에서 trait이 새로 unreachable로 나오던 것을 막는다. 그 모듈에서 보이지
      않는 대상(proc-macro 크레이트의 비공개 struct)은 남기지 않는다(`ModTree::visible_from`) — 1d3eb68의 근거 있는 축소는
      그대로다. README·README.ko 그래프 모델에 적었다.
    - `a108a5d` docs — N4: `verify-self-dead.sh`의 "grep -v는 남는 줄이 없으면 1"이라는 주석이 틀렸다(프로세스 치환의
      종료 코드는 `set -e`가 보지 않는다). 주석만 고쳤다.
    - `09c227c` docs — S3·S4: AGENTS.md 글롭 절을 이름공간별 의미론으로, CHANGELOG 머리말과 0.8 migration을 실제 변화
      (타입 변경·필드 제거, 두 세그먼트 이상의 모든 속성·derive 경로·아이템 위치 매크로 호출·로컬 아이템과 이름이 같은
      본문 매크로 호출의 대상 변경, 남은 보존 축소 둘)로 고쳤다.
  - **델타 재리뷰**(`f54266f` push 뒤, CI 7/7 성공): 판정 MERGEABLE — 수치 재현, 거짓 확정 간선 없음, 가시성 필터
    확인. 문서 정확성 4건을 반영했다(HANDOFF와 같은 커밋, 문서·주석만): S1 `#[cfg]`로 갈린 대안 임포트는 같은 이름공간이라
    E0252 근거가 닿지 않고 워크스페이스 쪽이 순서와 무관하게 이기며 본문 간선은 임포트 cfg를 싣지 않는다(전부터 — 후속
    후보), N1 보존 축소 (2)는 "다른 이름공간의 로컬 아이템이나 임포트"(`use crate::types::Foo` 옆 글롭 `fn Foo`의
    `Foo()`), N2 본문 매크로가 매크로로 옮겨 가는 것은 매크로 임포트가 워크스페이스 매크로일 때뿐(`use log::info`면
    로컬 fn 그대로), N3 추정 간선 대상은 "자리를 모르는 해석의 대상"(대개 분리 전 대상이지만 보장은 아님).
  - **반례 말뭉치**: 크레이트 10개(syn 2.0.119·tokio 1.53.1·regex-syntax 0.8.11·ra_ap_hir_def/hir_ty 0.0.349·rowan
    0.17.0·serde·serde_derive 1.0.229·clap 4.6.7과 이 저장소 `e12ef90` 사본)의 심볼 그래프를 이름공간 분리 전(`e12ef90`)·
    후(최종 트리) 바이너리로 대조했다(`graph --level symbol --tests`, 간선 키는 from·to·kind·tentative·cfg). 사라진 간선
    0, 더해진 간선 11개(이 저장소 4·ra_ap_hir_def 1 — 글롭이 들인 다른 이름공간의 같은 이름, syn 2·tokio 4 — 정해진
    명시 `use`가 같은 이름의 미정 `use`에 가려지지 않음), `dead --tests`·`dead --tests --retain-public` 차이는 이
    저장소의 알려진 2건이 사라진 것뿐이다. 리뷰 r2가 더한 zerocopy 0.8.50 + zerocopy-derive 워크스페이스: PR 머리
    `51eb85a`는 `e12ef90` 대비 간선 99개(리뷰어 집계 97 — 키 차이)를 잃고(`unsafe_impl!`·`assert_impls!` 토큰 → trait
    추정 간선) `dead --tests`에 `unreachable trait: zerocopy::Unaligned`를 새로 냈다. 최종 트리는 `e12ef90` 대비
    사라진 간선 0, 더해진 간선 69개(`SplitAt` uses 33, `_` 이름 바인딩으로 들어온 `NumExt` uses 29·`const _` uses 7 —
    N3), `dead --tests`가 `e12ef90`과 같다. 리뷰어 fixture(sp2·sp3·sp4·serdepat·nsmisc·globsrc·extws)의 dead 결과는
    `e12ef90`과 같거나 적다.
  - **검증**(2026-11-15, 로컬 cargo 1.97, 최종 트리): `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings`,
    `cargo test --locked`(25개 실행 단위 535개 통과 — 리뷰 r2 반영 9개 추가), `rules --strict` 0, `cycles --level symbol
    --strict` 0(type 레벨 23·module 레벨 2 — 변경 전과 같음), `scripts/verify-self-dead.sh`(0건 일치),
    `scripts/verify-cli-contract.sh` OK, `scripts/coverage.sh` 94.64%(게이트 90%, 그 안의 테스트 실패 0). 리뷰 r2 회귀
    테스트 7개(통합)는 `51eb85a` 코드에서 실패하는 것을 확인했고, 변이 검사 4개(미정 칸을 로컬·정해진 바인딩이 있어도
    사용, Any에서 미정 칸 먼저, 토큰을 값 이름공간으로, 추정 간선의 가시성 필터 제거)는 모두 테스트가 잡았다. 새
    fixture는 모두 `cargo check`를 통과한다. 첫 커밋 묶음의 회귀 테스트 4개도 그대로 통과한다(그 변이 검사는 다시 돌리지
    않았다).
    **실행하지 않음**: `--features semantic` test·clippy(sem.rs·semantic 경로를 바꾸지 않았고 ra_ap 빌드가 공유 디스크에
    크다), MSRV 1.96 check(로컬 툴체인 없음 — CI `msrv` 잡. 새 API 중 가장 새 것은 1.82의 `Option::is_none_or`), GLM
    리뷰(`packet-ask` 없음 — 독립 Claude 리뷰 r2로 대체).
- [PR #52](https://github.com/ictechgy/rustograph/pull/52)는 2026-11-09에 `main`으로 스쿼시 머지됐다(`9f9217e`).
  CHANGELOG `[Unreleased]`에 있고 아직 발행하지 않았다. `main` `9f9217e`의 CI 체크 7개가 모두 성공했다
  (2026-11-10 확인). 계열 9개 저장소 검토의 후속으로, 구문 수확이 조용히 놓치던 형태를 메웠다.
  - lib 계열 crate type(cdylib·rlib·staticlib·dylib)과 proc-macro 크레이트 수확. derive 등록 이름
    (`#[derive(pm::Hello)]`)은 그 함수 정점으로 잇는다.
  - `Self` 경로, 필드 타입 간선(심볼 레벨 순환은 재귀 데이터 정의를 세지 않는다. type·module 레벨은 센다),
    함수 안 아이템(지역 impl·trait 메서드 매개변수는 지역 이름), 중첩 블록 문장 매크로, `macro_rules!` 본문,
    표현식 목록이 아닌 매크로 인자.
  - 2024 판 진입점 속성(`#[unsafe(no_mangle)]` 등)과 static·impl 메서드 루트.
  - 워크스페이스 밖 self 타입 impl 본문: 확정 간선은 impl 모듈, 구현한 워크스페이스 트레이트의 선언 메서드는
    본문으로 가는 추정 간선만(도달성 전용 — rules·cycles에 거짓 의존이 생기지 않는다).
  - UFCS(호출과 값 경로 모두), 값으로 넘긴 연관 함수.
  - 수확하지 않는 example/test/bench 타깃 limitation, deps `(kind: normal)`.
  - isthmus 벡터를 `c76e105`로 다시 벤더링하고 `sql-relations.json`을 벡터로 실행한다.
  - 리뷰: AGENTS의 GLM(`packet-ask`) 리뷰는 그 세션에서 쓸 수 없어, 맥락을 물려받지 않은 독립 Claude 리뷰로
    대체했다. 판정은 "수정 후 머지"였고 지적 B1·B2(blocker), S1, N1·N2·N4는 고쳤다. S2(type 레벨 순환이 재귀
    데이터를 센다)는 의도로 판단해 문서화했다(근거를 나란히 적는 일은 이번 브랜치 `c95c4e8`). N3(미수확 타깃 한계가
    cycles에도 붙는다)은 그 타깃의 모듈이 실제로 검사되지 않으므로 빼지 않고 문구를 정확히 했다. 처리 근거는 PR #52
    본문 "독립 리뷰 반영"에 있다. PR #52는 GLM 리뷰 없이 머지됐다.
- 하지 않기로 한 것: `query` 스키마를 cartograph·kartograph·dartograph와 맞추기, `edge_evidence` 이름 바꾸기
  (2026-11-04 결정 — 공개 JSON 계약을 깨기 때문이다. AGENTS.md 규칙을 실제 계약에 맞췄다). Homebrew 토큰 설정,
  릴리스·태그는 범위 밖이다(비밀값·발행).
- 자기 분석 기준: `rules --strict` 0, `cycles --level symbol --strict` 0, `dead`는 `scripts/verify-self-dead.sh`가
  `scripts/self-dead.expected`와 대조한다 — 지금 목록이 비어 있어 0건이 기준이다. type 레벨 순환은 23개(대부분 서로를
  필드로 담는 데이터 타입 — 의도된 보고), module 레벨은 2개다. CI의 test 잡이 셋 다 돌고, `semantic-lint` 잡이
  `--features semantic` clippy를 돈다.
- persistence `relation-use` 생산자는 7개(cartograph·dartograph·gartograph·kartograph·pythograph·rustograph·
  tsograph)이고 schemagraph가 `relation-decl` 쪽이다. SQL 관계 이름 규칙은 isthmus
  `conformance/sql-relations.json`이 정본이다.

이 절의 "다음 할 일"·"후속 후보"는 위 현재 상태 절로 옮겨 갱신했다.

## 릴리스 준비 — v0.6.0 (2026-10-22) — 2026-10-25 발행됨

PR #40은 실제 Git 변경의 독립적인 이전/이후 수확과 CLI/MCP 영향 분석을 추가한다.
staged 삭제·rename과 로컬 Git clean filter 실행 회귀를 실제 fixture에서 재현하고 수정했다.
공개 Rust 결과 필드는 docs/MIGRATING-0.6.md, 분석 한계·준비 비용은 docs/CHANGE-IMPACT.md 및
docs/CHANGE-IMPACT-EVALUATION.md에 있다. Cargo.lock은 자기 버전만 바꾸고 의존 핀은 유지했다.
8cace4a 커밋 CI는 coverage 94.71%와 CLI 계약·자기 분석·Windows 출력 검사를 통과했다.
최종 버전 커밋의 독립 리뷰·CI·머지 확인 뒤 기존 태그 workflow를 사용한다. 이 기록 자체는
태그·Release·바이너리 배포 완료를 뜻하지 않는다. 공개 자산 다운로드·깨끗한 설치 검증이 필요하다.

## 릴리스 준비 — v0.4.2 (2026-10-11) — 태그 없이 v0.5.0으로 이어짐

PR #28의 실제 머지 커밋 `d7fc6b45f2a94634cc8deec2cdc1627e9adf4302`에서 준비했다.
이 변경은 버전·설치 태그와 semantic 테스트 fixture의 자원 사용을 보강한다.
새 태그·Release·Homebrew 탭은 아직 게시하지 않았다.
`Cargo.lock`에서는 rustograph 자기 버전만 바꿨다. ra_ap 0.0.349 등 의존 핀은 그대로다.

릴리스 노트(v0.4.1 이후):

- **정확성** — impact가 공유 의존의 다른 사용자까지 잘못 포함하던 문제를 수정했다.
  incoming 의존만 따라가고 CLI/MCP에 같은 결과를 낸다.
- **질의 계약** — query/impact의 0·정확한 상한, 순환에서 반복 정점 제거, 방향별 최단
  깊이 간선 증거, dangling 간선 제외를 고쳤다. 기존 JSON 필드와 명령은 유지한다.
- **그래프 정규화** — cfg별 같은 간선을 확정 우선으로 합치며 unsafe 증거를 보존한다.
- **테스트 실행 안정성** — 같은 fixture의 rust-analyzer 중복 기동을 줄여 병렬 테스트의
  메모리 부담을 낮췄다. 기존 의미 해석 단언은 유지하고 문서 복제 격리 검사를 추가했다.
- **검증** — 기본 217개 테스트, 512개 그래프에 대한 24,576개 독립 오라클 조합,
  CLI/MCP 실바이너리 회귀, 엄격 clippy·fmt·자기 분석 통과. PR #28 CI에서 coverage
  게이트도 통과했다. 배포 전 이 버전 커밋의 CI와 5플랫폼 패키지 검증은 별도로 확인한다.




## semantic 테스트 자원 보강 (2026-10-11)

- `tests/semantic.rs`의 불변 fixture는 프로세스당 실제 semantic 수확을 한 번만 수행한다.
  `OnceLock<Document>`에서 각 테스트에 소유 데이터를 복제해 검사 간 변형은 격리한다.
  디스크 캐시는 여전히 꺼져 있으며 모든 기존 47개 테스트 본문·단언은 그대로다.
- 문서 변형 격리 테스트를 추가했다. 별도의 캐시 무효화 테스트와 신선한 syn 수확은 유지한다.
- `cargo test --locked --features semantic -- --test-threads=1` 전체 265개 통과,
  semantic 전용 48개는 직렬 47.18초·기본 병렬 52.76초에 통과했다. feature 포함 `cargo clippy --all-targets
  -- -D warnings`와 fmt도 통과했다. 앞선 SIGKILL은 단언 실패 없이 공유 환경 과부하 중
  발생했다. 중복 기동을 제거해 재검증했으며 런타임이나 ra_ap 0.0.349 핀은 바꾸지 않았다.


## 분석 정확성 보강 (2026-10-11)

- `impact`가 양방향 `query` 결과를 뒤늦게 걸러 공유 의존의 다른 사용자까지
  영향받는다고 보고하던 결함을 수정했다. 처음부터 incoming 의존만 BFS한다.
- `query`/`impact`는 시작 정점을 다시 보고하지 않고, 같은 이웃·방향의 최단
  깊이 증거를 합친다. 역방향/contains 종류를 섞지 않으며 dangling 간선은 제외한다.
- `--max 0`과 정확히 맞는 상한을 지키고 실제 생략이 있을 때만 `truncated`다.
  수정 전 실패한 단위 회귀 6개와 실제 CLI/MCP 바이너리 회귀 3개를 추가했다.
- 조건별 간선 정렬에서 tentative를 cfg보다 먼저 비교해 같은 키가 떨어지던
  문제를 수정했다. 같은 cfg 안에서만 확정을 우선하며 unsafe 증거는 투영과 같이
  합친다. 수정 전 중복이 남는 회귀 2개와 순서 불변/멱등 검증을 추가했다.
- 3정점의 모든 유향 그래프 512개에 대해 root·depth·max 조합 24,576개를
  독립 거리 행렬 오라클과 대조했다. 기본 전체 테스트는 217개 통과했다.
- 기본 전체 테스트·clippy(`-D warnings`)·fmt·CLI 종료 코드 계약·자기 분석
  rules/cycles strict 통과. 로컬 coverage 도구 설치가 취소되어 coverage는
  미검증이며 CI의 기존 90% 게이트를 확인해야 한다. semantic feature 빌드는
  성공했지만 병렬 fixture 실행이 8개 통과 뒤 SIGKILL로 종료됐다. 공유 환경의
  부하가 내려간 뒤 `--test semantic -- --test-threads=1`로 별도 재검증해야 한다.

## 릴리스 준비 — v0.4.0 (2026-09-30, release/v0.4.0) — 2026-09-30 발행됨

`Cargo.toml`·`Cargo.lock` 자기 항목(0.4.0, `cargo update` 없이 손으로 — ra_ap 핀 유지, `cargo build --locked` 확인)과
README 설치 태그를 올렸다. 태그·Release·탭 갱신은 머지 뒤 별도 승인으로 한다. 릴리스 노트(v0.3.0 이후):

- **새 명령** — `routes --role server`(axum 0.7·0.8, actix-web 4 → isthmus http `route-decl`), `routes --role client`
  (reqwest·ureq, `--wrappers` http-wrappers v1, `--service` → `route-call`), `reach`·`impact --format language-traversal`
  (language-traversal v1, 다중 root 단일 패스), 최상위 `--version`(= `version`, isthmus 설치 버전 대조용).
- **출력 변경** — `schema` 사실에 `symbol: {qualifiedName, usr}`(usr = 그래프 정점 ID). 읽지 못한 `cfg_attr` 속성 목록을
  세어 limitation으로 보고한다.
- **동작 변경** — 수확의 지역 바인딩 스코프 해석을 고쳐, 지역 변수·매개변수가 같은 이름의 모듈·함수로 읽히던 거짓
  간선이 사라졌다(자기 분석 기준 109개). `graph`·`dead`·`cycles`·`query`·`impact` 결과가 달라질 수 있다.
- **소비자 요건** — route-decl·route-call·language-traversal 문서는 isthmus `3a45450` 이후(차기 isthmus-cli 0.10.0)가
  있어야 받는다. 발행된 isthmus-cli 0.9.0은 거부한다. 벤더링한 공유 벡터 lock은 `3a45450`.
- **알려진 결함(이후 수정됨)** — 서버 `route-decl`의 위치 열(`Ctx::locate`)이 UTF-16 코드 단위로 세어졌다. GRAPH-EXCHANGE는
  UTF-8 바이트를 요구하므로 경로 인자 앞에 비ASCII 문자가 있는 줄에서 열이 어긋났다. 0.4.1(`560e529`)에서 UTF-8 바이트로
  고쳤고, `schema` relation-use 열의 같은 결함(문자 수 열을 바이트로 잘라 UTF-16으로 센 값)도 뒤에 고쳐 지금은 schema·
  routes·client가 같은 환산(`schema::utf8_column`)을 쓴다.

## 직전 — feature/client-routes (2026-09-30, PR #24 머지됨)

`rustograph routes --role client`(isthmus http `route-call` 생산자). 규칙·근거·오라클 표는 `docs/HTTP-CLIENT.md`.

- **코드** — `src/source/routes/client.rs`(멤버·의존 버전, 문서 조립, 한계) + `client/{index,scan}.rs`(색인과 두 단계
  스캐너: 구조체 필드 값 수집 → 호출 사실), `compose.rs`(url-compose 규칙의 순수 구현 — `UrlVal`, WHATWG concat/join,
  `http::Uri` concat, 마스킹), `wrappers.rs`(http-wrappers v1 파싱·동사 바인딩). 위치 열은 계약대로 UTF-8 바이트다
  (`Ctx::locate_utf8`). 당시 서버 `route-decl`의 `Ctx::locate`는 UTF-16 열을 냈다 — 0.4.1(`560e529`)에서 UTF-8 바이트로
  고쳤다.
- **인식** — reqwest(`get`·`blocking::get`·Client 동사·`request`·`Request::new`), ureq 2/3 자유 함수·Agent, 선언된
  래퍼(`owner::name` = 정점 ID, 구조체 리터럴 생성자는 이름 = 타입 이름). 수신자는 구문 타입 추론(생성자·주석·필드
  타입·반환 타입·static 타입, `Arc`·`LazyLock` 등 벗김). 증명하지 못한 수신자는 사실 없이 센다.
- **검증** — 오라클 41 시나리오: 일치 37 · dynamic 3 · 요청 없음 1 · 불일치 0
  (reqwest 0.13.5·ureq 3.4.2·url 2.5.8, scratch에서 reqwest 0.12.28도 같음). 오라클이 확인한 것: `Url::join`의 마지막
  세그먼트 교체, WHATWG `//` 보존·점 세그먼트 제거, **ureq 3의 점 세그먼트 보존**, ureq 3이 http 요청도 프록시에
  CONNECT를 연다(기록 서버가 터널을 수락). 커버리지 92.14%, stable clippy(1.98.1) 0.
- **벡터** — isthmus `3a45450`(#133) 재벤더링. url-compose `producer` 41 + `producer:rustograph` 7 = 48/48. 새 규칙에 맞춰
  미상 base `rfc3986` 상대 참조의 `..`·빈 참조를 dynamic + `ambiguous-base-join:`으로 바꿨다. `scope.dynamic-*`(dynamicScope,
  서버 선언 선택 필드)는 내지 않으므로 분류만 했다. 결합 이름은 isthmus가 `Url::join` = `rfc3986`으로 정했고 reqwest·ureq는
  base가 없어 전체 URL 규칙이다(제안했던 `whatwg-concat` 등은 불필요).
- **e2e(scratch, 무패치 isthmus `3a45450`)** — workspace trace(`server` = axum08 fixture + `reach --roots-from`,
  `client` = fixture-client + `impact --format language-traversal --roots-from`, link match hosts `api.example.com`,
  selection 3 routes): 3 routes · 3 calls · 3 handlers · 3 client symbols. `POST /api/items` — client
  `client_app::api::ApiClient::create_item`(api.rs:38, exact) → handler `axum_app::handlers::items::create` → client 역방향
  `struct_field_plus`(depth 1, candidate — syn 메서드 호출 팬아웃). `GET /api/items/{}`는 `get_item` → `items::show`,
  `GET /api/search`는 `search` → `handlers::search`. base 앵커 호출 10건은 host가 없어 `unattributed-calls-omitted`.
  isthmus `c395c59`(직전 main)는 같은 client 문서를 "Fact kind is not valid for platform"으로 거부했다.
- **GLM 리뷰** — 재현 후 수정 2건: 상수 사슬 평가가 깊이를 새로 시작해 긴 사슬(3,000개)에서 스택 넘침 → 깊이를 이어
  모르는 값으로 낮춤(회귀 테스트는 수정 전 코드에서 넘침을 확인), authority가 `_` host·빈 포트를 거부 → WHATWG대로 받음.
  반박 4건: Slack 웹훅 전체 마스킹(벡터 `mask/slack-webhook`이 `/{}/{}/{}/{}`), `--service` 없는 사실 service(계약상 유효
  service는 사실 값), 호출 0건도 `http-wrapper-unresolved:`(HTTP-WRAPPERS 생산자 의무), 중첩 서식 `{:{}}`(Rust 문법 아님).
- 알려진 근사: 필드 수정 감지는 소유 타입을 모르면 이름 단위(보수적), `Url` 값의 `set_path` 등은 지역·필드를 믿지 않게
  만들 뿐 새 값을 계산하지 않는다, 선언된 구조체 래퍼 값을 받아 다른 래퍼를 부르는 함수(`execute`)는 동사 동적 dynamic
  사실을 하나 더 낸다, ureq 3에서 `http::Uri`가 거부하는 문자(공백 등)가 든 리터럴도 사실로 낸다.

## 직전 — feature/server-routes (2026-09-30, PR #23 머지됨)

`rustograph routes --role server`(isthmus http `route-decl` 생산자)와 수확 이름 해석 결함 수정.

- **routes** — `src/source/routes.rs`(조립·자체 계약 검사) + `routes/{axum,actix,common,pattern,template,validate}.rs`.
  axum 0.7·0.8(`specificity`, 버전은 resolve의 패키지 버전으로), actix-web 4(`registration-order`, App마다 group·리소스마다
  index). 규칙과 소스 근거(axum 0.7.9/0.8.9·matchit 0.7.3/0.8.4·actix-web 4.15.0·actix-router 0.5.4·codegen 4.4.0·
  tower-http 0.6.11 줄 번호)는 `docs/HTTP-ROUTES.md`. usr는 `harvest_parts`(같은 syn 수확의 트리·아레나)로 해석한 정점 ID라
  `reach --roots-from routes.json`이 그대로 받는다(verify-cli-contract가 확인).
- **오라클** — `experiments/routes-oracle/`(독립 워크스페이스, `[lib] path`가 fixture lib.rs). 세 fixture 정밀도·재현율·
  음성·끝 슬래시 요청 전부 100%, 기록을 `tests/routes.rs`가 오프라인 대조. 오라클이 잡아 고친 것: matchit 중간 파라미터·0.8
  접두 파라미터의 빈 값 매칭(빈 값 변형), axum이 원문 경로를 비교해 리터럴 중괄호 경로에 인코딩 요청이 안 닿음, actix Trim
  아래 빈 꼬리 변형이 안 닿음. 재기록은 `run_all.sh`.
- **공유 벡터** — isthmus `76b6141`에서 `conformance/`+`conformance.lock` 벤더링. template.grammar·normalize·dispatch.validate·
  scope.validate 60건 통과, 나머지는 분류(새 ruleId면 테스트가 실패).
- **수확 수정(bea5155)** — 지역 묶음(let·매개변수·클로저·match 팔·if/while let·for) 스코프 스택(`harvest::locals`), 값 자리의
  모듈 해석 폐기, `self` 수신자. 자기 분석 가짜 간선 109개 제거(모듈 references 100 + 지역명과 같은 fn 9), tests/fixture 8개.
  type 레벨 cycles: 수정 커밋에서 13 → 8, 이 브랜치 끝에서 12(새 routes 모듈의 메서드 → 모듈 함수 투영 4개 — 게이트 아님).
- **e2e(scratch, isthmus 76b6141 무패치 + schemagraph 703a21f)** — 합성 axum+sqlx 백엔드: route 선택 trace 4 chain 모두 route →
  핸들러 → relation-use → 테이블. 경로 호출 hop은 `direct`, 메서드 호출 hop은 `candidate`(syn 이름 팬아웃 — 수확 수정과 무관,
  `--semantic`이 필요). 수정 전 바이너리로 같은 입력을 돌리면 지역 `repo`가 모듈 `crate::repo`로 읽혀 checkout·get_order가
  `audit_log`에 **direct**로 닿는 거짓 hop 2개가 생겼고, 수정 후 사라졌다.
- 남은 것 / 알려진 근사(전부 거짓 match 쪽): axum 경로 우선 405 vs isthmus method 우선, actix 라우트 수준 method 가드(405)·
  스코프 포획(뒤 서비스로 안 넘어감)·`web::get()`의 HEAD 미수용, 정규화 미들웨어가 닿지 않게 만든 선언을 strict로 냄. actix
  매크로의 리소스 수준 method 가드에 소비자 `route-decl-path-shadowed` 경고가 날 수 있다(isthmus check로 확인, warning).
  impl 메서드 안에서 만든 Router·App은 평가하지 않고 `route-coverage:`로 센다.

## 직전 — feature/schema-usr (2026-09-30, API 영향 프로그램 Phase 7c, PR #22 머지됨)

isthmus `trace`가 Rust 백엔드의 핸들러 도달을 관계 사용과 잇도록 두 가지를 더했다.

- `schema` 사실에 `symbol: {qualifiedName, usr}` — usr는 `impact`와 같은 정점 ID.
  같은 syn 수확을 돌려 정점별 소스 범위(`harvest::OwnerSpan`)를 받고, 사실 바이트
  오프셋을 감싸는 가장 안쪽 정점을 고른다(정점 집합으로 검증). 구조체 어트리뷰트·
  필드 컬럼은 구조체, 메서드 밖 연관 상수는 impl self 타입. 최상위 `table!`·orphan
  파일은 usr 없이 `missing-relation-usrs:`로 센다.
- `reach`·`impact --format language-traversal` — language-traversal v1, 다중 root
  단일 패스(`src/traversal.rs`, pythograph 알고리즘 포팅), tentative 간선 = candidate
  root별 하한, `dispatch`·`unresolvedCalls` 미신고, 사용법 오류·root-not-found 64.
- 자기 분석이 이름 충돌 거짓 순환 둘(지역명 `owner_spans`, 매개변수 `facts`)과
  cli ↔ cli::reach 모듈 순환을 잡아 고쳤다 — 새 식별자는 같은 이름의 fn과 겹치지 않게.
- 남은 것: isthmus가 rust `route-decl`을 받지 않아 route 선택 trace는 불가(relation·
  심볼 선택은 동작). `type` 레벨 cycles는 main부터 10개(투영 산물)이고 이번에 3개가
  같은 종류로 늘었다(게이트 아님).
- 합성 e2e(axum·sqlx·sea_orm 스텁, scratch): `isthmus check --pairs` 5 matches,
  relation 선택 trace는 relation-use → 역방향 순회까지 이어지고 route-decl이 없어
  `non-http-entry`. isthmus를 scratch에서 고쳐 손으로 만든 route-decl로 route 선택도
  핸들러 → 테이블 도달을 확인했다.
- 발견(기존 수확 결함, 이번 범위 밖): 본문의 단일 식별자 경로가 지역 변수여도 크레이트
  루트 모듈 이름과 같으면 모듈 정점 references 간선이 된다(`fn f(repo: &dyn R) { repo.all() }`
  → `f → crate::repo`). 순회가 그 모듈의 `uses` 간선을 따라 모듈이 import한 아이템 전부에
  닿는 과대 근사가 된다. 표현식 경로는 모듈일 수 없으므로 모듈로 해석되면 버리는 것이 후보.
  syn 모드에서 메서드 호출은 이름 팬아웃(tentative)이라 구체 수신자 호출도 candidate다.

## 직전 세션 요약 (2026-09-25)

**목표.** HANDOFF의 보류 항목을 정리하고 main에 쌓인 기능을 배포한다.

**진행.**
- PR #17 머지 — HANDOFF에 persistence 생산자 현황(자매 PR 번호는 gh로 확인).
- PR #18 머지 — verify-cli-contract에 `schema` 추가. 넣다가 `schema`가
  `--semantic`·`--graph`·`--level`·위치 인자를 종료 코드 0으로 조용히
  삼키던 결함을 발견해 허용 목록(`Args::unsupported`)으로 거부(2).
- PR #19 머지 + **v0.3.0 배포** — 태그 → release.yml(5 플랫폼·패키지
  바이너리 계약 검증) → 탭 수동 갱신(tap `d0605f6`) → `brew upgrade`
  설치본으로 계약·자기 분석 재검증. PR #20 머지 — HANDOFF 반영.
- PR #21 머지 — `fix/cfg-attr-unparsed`. `split_cfg_attr`가 `.ok()?`로
  삼키던 속성 목록 파싱 실패를 `Harvest.unparsed_attrs`로 세고
  limitation 한 줄을 낸다. 이 수정은 v0.3.0 이후라 발행본에 없다.

**효과 있던 것.**
- 새 검사는 수정 전 바이너리/코드로 돌려 실패를 확인(뮤테이션) — 공허한
  테스트를 막는다. 수정 전 바이너리는 `git worktree add <tmp> HEAD~1` +
  `CARGO_TARGET_DIR` 분리로 빌드했다.
- 인자 추가 대신 **반환값으로 카운트를 올리는** 배선 — clippy
  too_many_arguments(>7)와 타입 변경을 피했다.
- 탭 수동 갱신은 게시된 checksums.txt를 받아 `shasum -c`로 대조한 값만 쓴다.

**안 된 것 / 반복하지 말 것.**
- zsh에서 `$B $c`로 명령 문자열을 돌리면 단어 분리가 안 돼 전부
  "unknown command"(2)가 나온다 — 계약 탐침은 `bash -c`로.
- 세션 시작 시 로컬 main이 origin보다 4커밋 뒤였고 HANDOFF 미커밋
  수정분이 원격 PR #16과 겹쳤다 — 작업 전 `git fetch` + 비교부터.
- `split_cfg_attr` 실패 재현에 Rust 2024 `unsafe(...)` 속성을 먼저
  의심했으나 syn 2.0.119는 받는다. 실패는 rustc도 거부하는 입력뿐.

**다음 단계.**
1. 이 저장소 안의 보류 항목은 없다. 남은 후보는 자매 저장소 일 —
   dartograph persistence 생산자, isthmus 교차 도메인 상관·네트워크
   도메인. 어느 쪽부터 할지 사용자에게 물을 것.
2. 선택: `deps` 보고서가 harvest limitation(unresolved_paths·
   unparsed_attrs)을 싣지 않는 것은 기존 설계 — 필요 시 별도 설계.
3. `HOMEBREW_TAP_TOKEN` 시크릿이 들어오면 탭 갱신이 자동화된다.

## 2026-09-25 당시 상태 (기록)

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

persistence 생산자는 당시 다섯 개(지금은 위 "현재 상태" 참고) — schemagraph(SQL `relation-decl`
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
- `src/config.rs` — `.rustograph.yml` 파싱(serde_norway 격리), baseline 키. 설정·기준선은
  `read_bounded_text`(64MiB 상한)로 읽고, 여는 파일 종류는 호출자가 `FileKinds`로 정한다 — 분석 대상
  저장소가 고른 경로(찾은 `.rustograph.yml`·설정의 `baseline:`)와 호출마다 다시 읽는 MCP `--config`는
  정규 파일만, 실행한 사람이 준 CLI `--config`·`--baseline`은 파이프·프로세스 치환도 받는다.
- `src/source/routes.rs` + `routes/` — isthmus http `route-decl` 생산자(`rustograph routes --role server`).
  axum(라우터 값 정적 평가·nest 결합·버전별 문법·빈 값 변형)·actix(App·Scope·Resource·매크로·configure·가드·
  NormalizePath)·`pattern`(actix 패턴·정규식 분류)·`template`(정규 템플릿 문법·정규화)·`validate`(order·스코프 검사).
  `source::harvest_parts`가 같은 수확의 트리·아레나를 넘긴다.
- `src/harvest/locals.rs` — 본문 지역 묶음 스코프(지역이 아이템보다 먼저).
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
- `scripts/` — coverage.sh(llvm-cov), verify-cli-contract.sh, verify-self-dead.sh(자기 분석 dead 대조,
  기대 목록 self-dead.expected), release-build-manifest.sh·publish-release-assets.sh(릴리스 자산)와 그
  테스트(test-*.sh).
- `.rustograph.yml` — 자기 계층 규칙(graph 순수 도메인 강제).

## 알려진 의미론 한계 (설계상, limitation으로 계측됨)

- 메서드 호출 `x.m()`은 이름 팬아웃(tentative) — 타입 정보 없음.
- `dyn`/제네릭/외부 크레이트 디스패치 불가시.
- 매크로는 쉼표 표현식 인자와 `{ident}` 캡처만 — 독자 문법은 미가시.
- `#[cfg]`는 평가 없이 포함+계수.
- 병합 루트에서 같은 이름의 인라인 mod가 양쪽 파일에 있으면 한쪽만 수확.

## 다음 단계 (우선순위 순)

1. ~~공개 리포 + 릴리스~~ — 완료. `HOMEBREW_TAP_TOKEN` 리포 시크릿이
   없어 탭 갱신은 수동으로 했다(0.3.0까지 확인) — 이후 릴리스는 Release 로그의 "Update the Homebrew tap"
   단계가 건너뛰었는지 확인한다. 시크릿을 넣으면 다음 릴리스부터 자동.
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
   상세는 위 "2026-09-25 당시 상태" 첫 단락 참고. 후속(fix/schema-contract):
   schema는 --dir/--out 외 플래그·위치 인자를 허용 목록으로 거부(2)하고,
   verify-cli-contract가 종료 코드와 target null/persistence 계약을 본다.
8. ~~`split_cfg_attr` 미계수~~ — 완료. PR #21 머지됨.
9. 다음 우선순위는 사용자가 정한다(맨 위 "현재 상태"의 후속 후보).
   Codex 3차 리뷰는 사용량 한도로 GLM 리뷰가 역할을 대신했다.

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
- 셸이 zsh다 — 문자열 변수로 명령을 조립해 실행하면 단어 분리가 안
  된다. 종료 코드 탐침은 `bash -c`로 돌려라.
- 릴리스 직후 설치된 바이너리로 자기 분석(`cycles`/`rules --strict`)을
  다시 돌려라 — cli↔mcp 순환은 커밋 시점이 아니라 v0.2.0 배포 바이너리
  검증에서 잡혔고 0.2.1 패치가 됐다.
