# Changelog

이 프로젝트의 주요 변경 사항을 기록한다.

## [Unreleased]

아래 변경은 같은 소스에서 dead·rules·cycles·schema·deps 결과를 바꾼다. 대부분은 정점·간선·보존 루트를 더
수확하는 쪽이다. 경로를 그 자리의 이름공간에서 해석하면서 일부 확정 간선은 대상이 바뀐다 — 두 세그먼트 이상의
속성·derive 경로, 아이템 위치 매크로 호출, 같은 이름의 로컬 아이템이 있는 본문 매크로 호출은 매크로로 가고
(`use log::info`처럼 그래프 밖 매크로면 확정 대상이 없다), 자리를 모르는 해석의 대상은 그 모듈에서 보이면 추정
간선으로 남는다. 조건부 `use`·재노출을 거쳐 해석한 간선은 그 `#[cfg]`를 싣고, `#[cfg]`로 갈린 대안 임포트는 둘 다
남아 대안마다 간선이 생긴다. 보존이 좁아지는 곳은 셋이다: 다른 크레이트에서 보이지 않는 예전 대상(proc-macro
크레이트의 비공개 struct)과, 다른 이름공간의 로컬 아이템이나 임포트로 가던 값·타입 경로(글롭이 들인 `fn Foo` 옆
`use crate::types::Foo`(이름 있는 struct)의 `Foo()`는 이제 fn이다), 임포트 표가 수렴하는 동안 거둬진 앞 바퀴의
대상(rustc가 쓰이지 않는다고 경고하는 정점으로 가던 거짓 간선)이다. 반례 말뭉치(크레이트 10개와 zerocopy 0.8.50,
대안 임포트·조건 전파는 크레이트 17개)에서 사라진 간선은 어느 빌드에도 없는 tokio 간선 6개(조건이 출발 모듈의 조건과
모순)뿐이다. Rust 라이브러리 호출자를 깨는 변경은 공개 구조체
필드 추가, `modtree::Module::imports`의 타입 변경(`Binding::Alternatives` 포함), `modtree::Module::unresolved_imports`
제거다([0.8 migration](docs/MIGRATING-0.8.md)). CLI 플래그·기본값, 그래프 문서 v1, 순회 문서 v1은 그대로다.

- `[lib] crate-type`을 적은 lib(`cdylib`·`staticlib`·`rlib`·`dylib`)과 proc-macro lib을 수확한다. 이전에는 cargo
  metadata kind가 `lib`이 아니라는 이유로 크레이트가 정점도 limitation도 없이 빠졌고, deps가 그 크레이트가 쓰는
  의존을 unused로 보고했다. derive 매크로는 등록 이름(`#[derive(pm::Hello)]`, `use pm::Hello`)으로도 그 함수 정점에
  닿는다 — 이 이름을 잃으면 deps가 쓰는 proc-macro 의존을 unused로 보고한다.
- impl 본문·시그니처의 `Self` 경로(`Self::f()`·`Self(..)`·`Self { .. }`·`-> Self`)를 self 타입으로 해석한다. 함수 안
  아이템은 바깥 impl의 `Self`를 쓰지 않는다.
- struct·enum·union 필드 타입을 references 간선으로 수확한다. 심볼 레벨 `cycles`는 두 데이터 타입 사이의 필드 참조를
  빼고 본다 — 재귀 데이터 정의는 순환으로 세지 않는다. type·module 레벨 순환과 dead·rules는 그 간선을 쓴다. 서로를
  필드로 담는 데이터 타입 쌍은 type 레벨(타입이 다른 모듈이면 module 레벨) 순환이라 `cycles --level type --strict`가
  새로 실패할 수 있다 — 그 수준에서는 타입 사이의 실제 의존이다.
- 함수 안 아이템(최상위 문장의 중첩 `fn`·지역 `impl`)과 중첩 블록 문장 매크로의 인자를 방문한다. `macro_rules!` 정의
  본문의 경로와 표현식 목록이 아닌 매크로 인자(`json!({..})`) 안의 이름은 추정 간선이 된다. 함수 안 `impl`·`trait`
  메서드의 매개변수는 그 본문에서 같은 이름의 아이템을 가린다.
- 보존 루트: `#[unsafe(no_mangle)]`·`#[unsafe(export_name = "..")]`·`#[cfg_attr(pred, no_mangle)]`, `#[used]`·
  `#[no_mangle]`·`#[global_allocator]` static, impl 메서드의 `#[no_mangle]`, `#[panic_handler]`. 이전에는 Rust 2024
  판 FFI export가 전부 unreachable이었다.
- self 타입이 워크스페이스 밖인 impl(`impl Tr for String`, blanket impl, `impl Tr for &Local`)의 본문을 버리지 않는다.
  확정 간선은 impl이 선언된 모듈이 소유해 rules·cycles가 그 모듈의 의존으로 본다. 구현한 워크스페이스 트레이트가
  그 메서드를 선언했으면 선언 메서드가 본문이 부르는 곳으로 가는 추정 간선(도달성 전용)과 트레이트로 가는
  references 간선을 갖는다 — 트레이트 모듈이 impl 모듈에 의존하는 것처럼 보이지 않는다. 그런 impl 블록 수를
  limitation으로 싣는다.
- `<T as Tr>::m()`은 `T`의 impl 메서드로, `<T>::m()`과 값으로 넘긴 연관 함수(`.map(T::new)`)는 `T`의 같은 이름
  메서드로 간선을 잇는다. 호출이 아니라 값으로 쓴 `<T>::X`·`<T as Tr>::m`(`let f = <T as Tr>::m;`)도 같은 규칙이다 —
  `<T>::X`가 같은 이름의 모듈 아이템으로 읽히지 않는다.
- 수확하지 않는 example·test·bench 타깃 수를 limitation으로 싣는다.
- `deps` 텍스트 출력이 normal 의존의 종류를 `(kind: )` 대신 `(kind: normal)`로 쓴다. JSON은 그대로다.
- isthmus 공유 적합성 벡터를 `c76e105`로 다시 벤더링하고 `sql-relations.json`을 더했다. SQL 관계 이름 단위 테스트는
  손으로 옮긴 사본 대신 벤더링한 벡터를 읽는다.
- 모듈 임포트 표가 타입·값·매크로 이름공간을 rustc처럼 나눈다. 글롭(`use m::*`)은 대상 모듈의 이름을 이름공간마다
  따로 들이고, 명시 `use`와 로컬 아이템은 같은 이름공간의 글롭만 가린다. 명시 `use a::x`는 `a`에서 `x`가 차지한
  이름공간을 모두 들인다. 이전에는 이름당 바인딩이 하나라, `fn harvest`와 `use crate::harvest::{self}`가 함께 있는
  모듈을 자식이 `use super::*`로 들이면 모듈이 빠져 `harvest::f()`가 풀리지 않았고, 글롭이 들인 `fn shared` 옆의
  `use crate::other as shared`가 그 함수까지 가렸다 — 그 경로로만 쓰는 아이템이 `dead`에 unreachable로 나왔다.
  자기 분석 dead의 알려진 2건이 사라져 `scripts/self-dead.expected`가 비었다. 호출·값 경로는 값, 타입 자리·구조체
  리터럴은 타입, 매크로 호출은 매크로 이름공간을 먼저 보고, 그 이름공간에 묶인 것이 없으면 예전처럼 찾는다. 문법을
  모르는 매크로 인자 토큰은 자리를 모르는 해석이다. 글롭 대상(`use a::b::*`의 `b`)은 타입 이름공간에서 찾는다 —
  `fn harvest`와 `use crate::harvest`가 함께 있는 모듈의 `use super::harvest::*`가 fn을 잡아 아무것도 들이지 않았다.
  이름공간을 정하지 못한 명시 `use`(외부 크레이트·std 경로, 매크로가 등록한 derive 이름 — `pub use
  zerocopy_derive::Unaligned`)는 같은 이름의 로컬 아이템이나 이름공간이 정해진 임포트를 어느 자리에서도 이기지
  않는다(rustc는 그런 공존을 E0255·E0252로 거부하니 그 `use`는 다른 이름공간에 있다) — `pub trait Unaligned` 옆의
  그런 `use`가 있어도 값 자리·매크로 토큰의 `Unaligned`는 trait이다. 그래서 `pub use crate::split_at::SplitAt` 뒤에
  `pub use zerocopy_derive::SplitAt`이 와도 trait이 가려지지 않는다 — 이전에는 소스 순서상 뒤의 것이 이겨 글롭으로
  들인 모듈의 `impl SplitAt for ..`가 풀리지 않았다. `#[cfg]`로 갈린 대안 임포트(`#[cfg(feature = "pl")] use
  crate::pl::Condvar;`와 `#[cfg(not(..))] use std::sync::Condvar;`)는 실제로는 같은 이름공간이라 위 근거가 닿지
  않는다(rustc가 둘을 한 빌드에서 보지 않을 뿐이다). 이때도 소스 순서와 무관하게 워크스페이스 쪽이 이긴다(이전에는
  뒤의 것이 이겼다). 그 이름을 쓰는 본문 간선(`Condvar::new()` 호출 등)은 `uses` 간선처럼 워크스페이스 쪽 임포트의
  cfg를 싣는다(아래). 그런 `use`는 지금처럼 같은 이름의 글롭은 모두 가린다.
  라이브러리 호출자용 `modtree::Module::imports`의 타입이 바뀌고(`PerNs<Binding>` — 이름공간을 정하지 못한 칸은
  `undetermined`) `unresolved_imports`가 빠졌다([0.8 migration](docs/MIGRATING-0.8.md)).
- 속성·derive 인자·아이템 위치 매크로 호출의 경로를 매크로 이름공간에서 해석한다. 다른 크레이트의
  `#[derive(pm::Hello)]`는 proc-macro 크레이트에 같은 이름의 struct가 있어도 `#[proc_macro_derive(Hello)]` 함수로
  간다. 이전에는 그 struct(proc-macro 크레이트는 매크로 말고는 내보낼 수 없다)로 이어져 derive 함수 대신 struct에
  거짓 사용 간선이 붙었다 — 그 경로로만 살아 있던 struct는 이제 `dead`에 나올 수 있다. 크레이트 안의
  `Hello { .. }`·`-> Hello`는 그대로 struct다. 매크로 경로(속성·derive 인자·매크로 호출)를 자리를 모르게 해석한
  대상이 다르고 그 모듈에서 보이면 추정 간선으로 남긴다(대개 이름공간 분리 전의 대상이다) — `mylib`이 trait
  `Hello`와 `pub use mylib_derive::Hello`를 함께 내보내면 `#[derive(mylib::Hello)]`의 확정 간선은 derive 함수,
  trait은 추정 간선이라 `dead`가 trait을 새로 보고하지 않는다(derive 펼침은 대개 그 trait을 구현한다).
  `use crate::logging::info`(워크스페이스 매크로) 옆 로컬 `fn info`가 있는 모듈의 `info!()`도 확정 간선은 매크로, 그
  fn은 추정 간선이다. `use log::info`처럼 그래프 밖 매크로여도 로컬 fn은 확정 대상이 아니다 — 아래.
- 같은 이름공간에 같은 이름을 들이는 서로 다른 임포트 둘(명시 `use` 둘, 글롭 둘)을 둘 다 남긴다. rustc는 한
  빌드에 그런 둘을 두지 않으므로(E0252, 글롭 이름은 쓰면 E0659) `#[cfg]`로 갈린 다른 빌드의 것이다 —
  `#[cfg(unix)] use crate::unix::Sys;`와 `#[cfg(windows)] use crate::windows::Sys;`, 플랫폼 글롭 둘(`#[cfg(unix)]
  pub use self::unix::*;`), 모듈 별칭 둘(`use linux as platform;`·`use bsd as platform;`). 이전에는 소스 순서상
  뒤의 것만 남아 다른 빌드의 대상과 그 경로로만 쓰는 아이템이 `dead`에 거짓으로 나왔다(rustix 1.1.5의 리눅스
  `ioctl::linux::compose_opcode`와 상수 10개 — `--target x86_64-unknown-linux-gnu`에서도). 이제 그 이름을 거친 경로는
  대안마다 간선을 얻고 간선은 각 임포트의 조건을 싣는다 — `--target`·`--exclude-tests`는 그 빌드에 없는 임포트를
  거친 간선을 지운다. 한 모듈의 `use T as _` 여럿(명시든 글롭으로 들인 것이든)도 모두 `uses` 의존 증거로
  남는다(syn 2.0.119의 `syn::error`가 `PunctExt`를 잃어 `dead --retain-public`에 거짓으로 나왔다). 같은 정점을
  들이는 명시 `use` 둘은 글롭처럼 약한 조건으로 합친다. 같은 칸에서 해석되는 임포트와 해석되지 않는 임포트(std
  경로)가 만나면 소스 순서와 무관하게 해석되는 쪽이 남는다 — `#[cfg(feature = "pl")] use extdep::Mutex;` 뒤의
  `#[cfg(not(..))] use std::sync::Mutex;`가 외부 크레이트 사용 증거를 지워 `deps`가 쓰는 의존을 unused로 냈다.
- 같은 갈림이 `use`와 다른 바인딩 사이에 있어도 둘 다 남긴다. 자기 `#[cfg]`를 단 명시 `use`는 같은 칸의 글롭을
  가리지 않는다 — `use a::*;` 옆 `#[cfg(test)] use a::f;`의 `f()`는 조건 없는 간선이고, `use real::*;` 옆
  `#[cfg(test)] use mock::Clock;`의 `Clock::now()`는 두 대안이다. 이전에는 명시 `use`가 늘 글롭을 가려 테스트가 아닌
  빌드에서 쓰이는 `real::Clock::now`가 `dead`에 나왔다(이 브랜치의 앞 머리에서는 `a::f` 간선에 `cfg(test)`가 붙어
  `--exclude-tests`·`--target`이 `a::f`를 냈다). 글롭 임포트의 조건이 그 `use`의 조건을 함축하면(`#[cfg(feature =
  "x")] use a::*;` 옆 `#[cfg(feature = "x")] use b::f;`) 둘은 늘 함께라 지금처럼 가린다. 남는 글롭 쪽은 자기 조건을
  그대로 싣는다(명시 `use` 조건의 부정을 더하지 않는다 — 그 조건의 빌드에서도 남는 간선이다). 모든 선언이
  `#[cfg]`를 단 로컬 아이템(`#[cfg] mod` 포함) 옆의 같은 이름공간 명시 `use`는 그 아이템이 없는 빌드의 대안이다 —
  `#[cfg(feature = "x")] fn sum()` 옆 `#[cfg(not(feature = "x"))] use scalar::sum;`의 `sum()`, `#[cfg(feature = "x")]
  mod imp` 옆 `#[cfg(not(..))] use crate::other as imp;`의 `imp::f()`는 두 정점을 부른다(이전에는 로컬 아이템만이라
  기본 빌드에서 쓰이는 `scalar::sum`·`other::f`가 `dead`에 나왔다). 글롭 이름은 로컬 아이템 옆에 합법적으로 있으면서
  가려지기만 하므로, 아이템의 선언이 하나이고 글롭 대상이 스스로 조건부가 아닐 때만 그 조건의 부정(`not (..)`)을 달고
  대안이다 — `#[cfg(unix)] fn sys`와 `#[cfg(not(unix))] fn sys`처럼 선언이 여럿이면 모든 빌드를 덮을 수 있어 대안이
  아니다. 글롭이 보이는 빌드에 늘 아이템이 있거나(같은 `#[cfg(feature = "x")]`) 아이템이 없는 빌드에서 같은 칸의 명시
  `use`가 글롭을 가리면(`#[cfg(not(unix))] use crate::b::sys;` 옆 `#[cfg(unix)] fn sys`) 역시 대안이 아니다. 대안을 거친
  간선의 조건이 쓰는 자리·출발 아이템·그 조상 모듈의 조건과 글자로 모순이면(`#[cfg(unix)] fn run`이
  `#[cfg(not(unix))] use crate::plat::sys;`를 거침, tokio의 `#[cfg(all(test, loom))] mod mocked` 안에서
  `not(all(test, loom))` 쪽으로 가는 간선) 어느 빌드에도 없는 간선이라 지우고 그 수를 limitation("edges were left out
  because their #[cfg] conditions contradict ...")으로 싣는다 — 지우지 않으면 거짓 순환이 됐다(tokio module 레벨 순환이
  9a79ce5와 같은 1로 돌아왔다). 그런 대안 임포트를 self 타입으로 쓴 impl(`impl Sys { .. }`)은 대안마다 수확해 메서드 정점이 대안 타입 아래에
  그 임포트의 조건을 달고 생기고, 트레이트 경로로 쓴 impl(`impl plat::Tr for X`)은 대안마다 implements 간선을 낸다
  — 하나만 이으면 다른 빌드의 메서드·트레이트가 거짓 unreachable이었다(`--target` 리눅스가 리눅스에서 쓰이는 것을
  냈다). routes는 아직 대상 하나다.
- 임포트 표를 수렴시키는 동안 앞 바퀴의 해석이 거둬지면(글롭이 아직 펼쳐지지 않아 크레이트 루트로 넘어간 `pub use
  foo::bar`) 빈 표에서 다시 쌓는다 — 명시 `use`와 글롭 대상은 수렴한 표로 해석하고 글롭이 펼치는 이름만 새로 쌓은
  뒤, 지금 표로 다시 수렴시켜 거두는 것이 없으면 끝이다(재노출 사슬이 길면 되풀이한다 — 상한 32번, 넘으면 남은 모듈
  수를 "may keep stale re-export targets" limitation으로 싣는다). 거둔 대상이 서로를 글롭으로 들이는 모듈 사이에 남아
  거짓 확정 간선(rustc가 쓰이지 않는다고 경고하는 루트의 `foo::bar`로 가는 call)이 되던 것을 고쳤다 — 그 거짓
  간선으로만 살아 있던 아이템은 이제 `dead`에 나온다. 2015 판 크레이트에서는 거두지 않는다 — 그 판의 `use` 경로는
  크레이트 기준이라 거둔 쪽이 제 대상일 수 있는데 이 해석기는 2018 판 규칙으로 푼다. 루트가 아닌 모듈의 `use` 경로가
  크레이트 루트가 선언한 이름으로 시작하는 2015 판 크레이트를 세어 "edition-2015 crates have `use` paths that edition
  2015 resolves from the crate root ..." limitation으로 싣는다(그 경로로만 쓰는 아이템을 `dead`가 낼 수 있다 —
  9a79ce5부터 그랬고 이제 알린다). 반례 말뭉치 17개에서는 다시 쌓은 크레이트가 없다.
- 조건부 `use`·재노출·글롭을 거쳐 해석한 간선이 그 `#[cfg]`를 싣는다 — 본문(호출·참조·구조체 리터럴·매크로)·시그니처·
  필드 타입·속성 간선과, 조건부 재노출을 `use`한 모듈의 `uses` 간선이다. `#[cfg(feature = "pl")] use
  crate::pl::Condvar;` 옆의 `Condvar::new()`는 `feature = "pl"` 아래의 `call`이고, `#[cfg(test)] use crate::fake::Clock;`
  을 거친 간선은 `--exclude-tests`가 지운다. 이전에는 `uses` 간선만 조건을 싣고 본문 간선은 조건 없는 확정 간선이라
  `--target`·`--exclude-tests`가 그 빌드에 없는 임포트를 거친 간선을 지우지 못했다. 아이템이 이미 같은 조건을 달면
  간선의 조건 표기는 그대로다. 조건 없이 돌린 결과에서는 간선이 그대로 남는다.
- 매크로 자리에서 이름공간을 정하지 못한 `use`(그래프 밖 매크로 `use log::info`, `use std::println`)가 같은 이름의
  로컬 fn·trait이나 이름공간이 정해진 타입·값 임포트를 이긴다 — 그것들은 매크로가 될 수 없다. `fn info` 옆
  `use log::info`의 `info!()`가 로컬 fn으로 가는 확정 `call` 간선이 되어(rustc는 그 fn을 미사용으로 경고한다), 그 fn이
  호출자를 다시 부르면 `cycles --level symbol --strict`가 거짓 순환으로 실패했다. 이제 로컬 fn은 추정 간선이라
  `dead`는 그대로이고 cycles·rules에서 빠진다(`--deps`면 확정 간선은 `log` 크레이트 정점으로 간다). 반례 말뭉치
  17개에서는 바뀐 간선이 없다.
- 문서: README·README.ko 그래프 모델이 두 데이터 타입 사이의 필드 간선을 심볼 레벨 `cycles`는 빼고 type·module
  레벨은 세는 근거를 나란히 적는다 — 심볼 레벨은 어느 코드가 재귀하는지, type·module 레벨은 그 단위를 계층으로
  나눌 수 있는지를 묻는다.
- CI: `semantic-lint` 잡이 `--features semantic`으로 clippy를 돌리고, test 잡이 자기 분석 dead 결과를 알려진 목록
  (`scripts/self-dead.expected`)과 대조한다(`scripts/verify-self-dead.sh`). 커버리지 잡의 `taiki-e/install-action`을
  v2.90.3(`cfdb7ed`)으로 올렸다(dependabot PR #55와 같은 핀).

## [0.7.0] - 2026-11-03

v0.7.0의 아래 변경에는 Rust 라이브러리 호출자를 깨는 공개 구조체 필드 추가가 있다
([0.7 migration](docs/MIGRATING-0.7.md)). CLI 플래그·기본값, 그래프 문서 v1, 순회 문서 v1은 그대로다.

- CI가 잠금 파일을 사용해 semantic feature를 빌드·검증하고, 커버리지 실패의 stderr를 보존한다.
- `Cargo.toml`에 최소 지원 Rust `rust-version = "1.96"`을 선언하고, CI `msrv` 잡이 rustc 1.96.0으로 기본·semantic
  빌드를 `--locked`로 확인한다. 1.96을 정하는 것은 semantic feature다 — 잠근 ra_ap_* 0.0.349는 1.95를 선언하지만
  ra_ap_tt가 1.95에서 불안정한 `std::assert_matches`를 쓴다. 더 오래된 툴체인의 `cargo install`은 컴파일 오류 대신
  요구 버전을 알린다.

- 본문 타입·제네릭 인자·qself·클로저 반환형의 참조를 수확하고, const 표현식 방문과
  타입/const 제네릭 매개변수의 이름 가림을 보존한다.
- 컴포넌트 패턴의 세그먼트 안에서 `*`·`?`를 지원한다. `::` 경계는 넘지 않는다.
- `use a::b::{self}`·`use a::b::{self as x}`가 모듈 `b`를 임포트한다. 이전에는 `b::self`를 찾다 임포트가 사라져
  `b::f()` 호출과 모듈 `uses` 간선이 빠졌다 — 최상위가 아닌 모듈의 함수가 `dead`에 unreachable로 나오고,
  `impact`가 그 호출자를, `rules`·모듈 레벨 `cycles`가 그 의존을 보지 못했다.
- `name::item` 경로의 접두사 `name`을 타입 이름공간으로 해석한다. 로컬 타입 이름공간 아이템(자식 모듈·struct·enum·
  union·trait·type)은 같은 이름의 임포트(글롭 포함)를 이기고, 로컬 이름이 값(`fn`·`const`·`static`)·매크로 아이템뿐이면
  그 이름의 임포트를 따른다. 이전에는 `fn harvest`가 `use crate::harvest`를 가려 `harvest::f()`가 미해석이 됐다.
  한 세그먼트 호출 `name()`은 지금처럼 그 fn이다. 라이브러리 호출자용 `modtree::Module`에 `type_items`가 늘었다
  ([0.7 migration](docs/MIGRATING-0.7.md)).
- 경로 해석이 재노출을 따라간다: 다른 모듈이 `use`로 들인 이름(`pub use inner::Tr`, `pub use inner::*`,
  `pub use inner::deep as shortcut`, 다른 멤버 크레이트 루트의 재노출)을 지나는 경로(`crate::Tr`, `use crate::prelude::*`가
  들인 이름, `crate::shortcut::f()`, `use a::Tr`)를 정의 위치로 해석하고, 글롭은 대상 모듈의 재노출도 펼친다. 이전에는
  그런 임포트·호출·impl의 트레이트 경로가 미해석이라 정의가 `dead`에 unreachable로 나오고 `impact`·`rules`·`cycles`에서도
  빠졌으며, 재노출로 이름 붙인 워크스페이스 트레이트의 impl이 `--retain-external-trait-impls`와 기본 보고서의 계수에서 외부
  트레이트 impl로 분류됐다. 재노출 사슬은 임포트 표가 더 바뀌지 않을 때까지 해석을 되풀이해 풀고(최대 32바퀴, 표는 바퀴마다
  다시 만든다), 그 안에 끝나지 않은 모듈 수는 limitation으로 센다. 라이브러리 호출자용 `modtree::fill_imports_settled`·
  `MAX_IMPORT_PASSES`·`Scope`를 더했다([0.7 migration](docs/MIGRATING-0.7.md)).
- 글롭 임포트(`use m::*`)가 rustc처럼 들이는 모듈에서 보이는 이름만 들인다 — 비공개 아이템·`use`는 선언한 모듈과 그 자손,
  `pub(super)`는 부모와 그 자손, `pub(crate)`는 그 크레이트 안에서만 보인다. 제한 가시성(`pub(..)`)은 가리키는 모듈을
  찾지 못해도 크레이트를 넘지 않고, 2015 판의 `pub(in top)`는 크레이트 기준 경로로 읽는다. 글롭한 모듈 자신
  (`use m::*`의 `m`)은 이름으로 묶지 않고 그 글롭을 쓴 모듈의 `uses` 의존 증거로만 남긴다 — 이전에는 `m`이 그 모듈에서 이름처럼
  해석에 쓰여, 같은 이름의 외부 크레이트 경로(`extdep::Thing`)가 로컬 모듈로 확정될 수 있었다. 이전에는 형제·다른 크레이트의 글롭이 비공개
  `use`와 아이템까지 들여 다른 글롭의 같은 이름을 덮어써, 확정 간선이 엉뚱한 정의로 가고 진짜 대상이 unreachable로 나왔으며
  `rules`가 없는 계층 위반(`ui → db`)을 냈다. 명시 임포트는 해석되지 않는 std 경로(`use std::error::Error`)여도 같은 이름의
  글롭 임포트를 소스 순서와 무관하게 가리고, 그 이름의 해석을 크레이트 루트의 같은 이름으로 넘기지 않는다. 글롭이 펼친
  재노출은 글롭의 `#[cfg]`와 재노출의 `#[cfg]`를 함께 싣는다(`--target`·`--exclude-tests`가 어느 빌드에도 없는 임포트 간선을
  지운다).
- `dead --retain-external-trait-impls`·기본 보고서의 외부 트레이트 impl 계수: 트레이트 경로가 해석되지 않아 implements
  간선이 없는 impl도, 그 이름의 워크스페이스 트레이트가 같은 이름의 메서드를 선언하면 워크스페이스 impl로 보아 보존하거나
  세지 않는다(보존을 줄이는 쪽으로만 틀린다).
- `rules`·MCP `rustograph_rules`: 간선이 정렬되지 않은 문서(`--graph`로 읽은 다른 도구의 문서 등)에서 같은
  `(rule, from, to)`의 간선 종류가 call·references·call처럼 섞여도 같은 기준선 키(`rule|from|to|kind`)의 위반을
  한 번만 보고한다. 이전에는 정렬 키에 kind가 없어 같은 위반이 두 번 나올 수 있었다. 수확한 문서의 결과는 같다.
- `dead --retain-external-trait-impls`·MCP `rustograph_dead`의 `retainExternalTraitImpls`(opt-in): 워크스페이스 밖에서
  선언된 트레이트(`Drop`·`Iterator`·`Display`·`From`·`syn` visitor 등)의 impl 메서드를 self 타입이 닿으면 함께 닿은
  것으로 보고, 그 메서드가 부르는 코드까지 따라간다. `--explain`도 같은 규칙을 쓴다. 워크스페이스 트레이트의 미사용
  impl과 닿지 않는 타입의 impl은 계속 보고한다. 기본 판정은 그대로이고, 기본 `dead` 보고서는 닿는 타입에 붙은 그런
  미도달 메서드 수를 limitation으로 센다(셀 것이 없으면 싣지 않는다). 라이브러리 호출자용 `analysis::DeadOptions`·
  `dead_with_options`·`explain_path_with_options`·`QueryIndex::dead_with_options`를 더했다(기존 함수는 그대로다).
  `DeadOptions`는 `#[non_exhaustive]`라 선택지가 늘어도 호출자가 깨지지 않는다 — `DeadOptions::default()`에서 필드를
  바꿔 만든다.
- `cycles --level module --exclude-nested-modules`(opt-in, CLI): 구성원이 한 모듈과 그 자손뿐인 순환(자식의
  `super::` 호출 등)을 빼고 판정한다(`--strict` 포함). JSON 보고서에는 뺀 수를 `excluded_nested`로 싣는다(없으면 `0`). 형제나
  다른 모듈이 끼는 순환은 계속 보고한다. 기본 판정과 JSON은 그대로이고, 기본 텍스트 출력은 그런 순환이 있을 때 그
  수와 플래그를 `note:` 줄로 알린다. 모듈 레벨이 아니면 사용법 오류다.
- `dead`·MCP `rustograph_dead`: 보존 루트가 하나도 없는 크레이트(main·`#[no_mangle]` 없는 lib)의 unreachable 공개
  아이템 수와 크레이트 이름을 limitation으로 세고, 워크스페이스 밖에서 쓰는 라이브러리라면 `--retain-public`(MCP
  `retainPublic`)을 쓰라고 조건부로 알린다(워크스페이스 안에서만 쓰는 lib이면 그 아이템은 정말 호출자가 없다). 판정과
  종료 코드는 그대로다. 이전에는 lib 크레이트의 첫 `dead`가 공개 API 전부를 unreachable로 내면서 아무 안내가 없었다.

## [0.6.0] - 2026-10-25

### Change analysis

- `change-impact --base REV [--head REV | --working-tree]`와 MCP `rustograph_change_impact`가 실제 변경을
  독립적인 이전/이후 소스 그래프로 연결한다. 삭제·rename의 과거 호출자를 보존하며 staged 삭제 경로를
  현재 캡처에 되살리지 않는다. 파일 전체 seed의 정밀도, 연결된 테스트/진입점 후보, 내용 신선도,
  미해결 경로와 깊이·출력·seed 제한을 결과에 남긴다. 서로 다른 revision의 간선을 섞지 않는다.
- 기존 ordinary impact에도 수확한 한계·provenance·실제 경로의 간선 근거를 보존한다. CLI depth=1과
  MCP depth=0 기본값 및 기존 `truncated`의 출력 개수 의미는 유지한다. 새 공개 Rust 필드의 이행은
  [0.6 migration](docs/MIGRATING-0.6.md)을 따른다. 빈 결과는 안전 판정이나 테스트 생략 허가가 아니다.
- 새 Git 흐름도 로컬 clean/smudge/process 필터·hook·fsmonitor를 실행하지 않는다. 절대 PATH의 Git을
  사용하고 원본 checkout은 수정하지 않는다. 준비 비용과 미검증 범위는 [평가 기록](docs/CHANGE-IMPACT-EVALUATION.md)에 남긴다.

### Security

- `reach`·`impact --format language-traversal`의 `revision` 판독: 분석 대상 저장소의 git 설정이 정한 명령을 더는
  실행하지 않는다. git 호출마다 바로 앞에서 설정된 filter 드라이버(clean·smudge·process)를 모아 빈 명령으로 덮고,
  fsmonitor 훅과 서명 표시를 끄며, 서브모듈 안에서는 git을 띄우지 않는다(서브모듈 저장소의 설정도 실행하지 않는다).
  이름에 `=`·제어 문자·깨진 UTF-8이 들어 덮을 수 없는 드라이버가 있으면 git을 실행하지 않는다. 이전에는 git이 stat
  캐시가 낡은 파일을 다시 읽을 때 `.gitattributes`나 `.git/info/attributes`가 고른 드라이버가 실행됐다. 자세한 내용은
  README의 Security 절에 있다.
- `reach`·`impact --format language-traversal`의 `revision` 판독: git을 `PATH`의 절대 경로 항목(`PATH`가 없으면
  `/bin:/usr/bin`)에서만 찾아 그 절대 경로로 실행한다. git은 `--dir`을 작업 디렉터리로 띄우므로, 이전에는 `PATH`의
  빈 항목이나 `.`·`bin` 같은 상대 항목이 분석 대상 디렉터리 기준으로 풀려 체크아웃에 심은 `git`이 실행됐다(체크아웃
  밖에서 rustograph를 띄워도 같았다). 절대 경로 항목에 git이 없으면 git이 없을 때처럼 `revision`을 싣지 않는다.
- 수확하는 모든 명령(`--graph` 없이 그래프를 만드는 명령과 MCP 서버, MCP `rustograph_deps`)과 `--target`·semantic 캐시
  키의 rustc 프로브: 분석 대상 체크아웃 안에서 실행해도 그 체크아웃의 `.cargo/config.toml`(`build.rustc-wrapper`·
  `build.rustc-workspace-wrapper`·`build.rustc`)과 rust-toolchain 파일이 고른 프로그램을 실행하지 않는다. `cargo`·
  `rustc`·`rustup`은 `PATH`의 절대 경로 항목(`PATH`가 없으면 `/bin:/usr/bin`)에서만 찾아 그 절대 경로로 실행하고,
  툴체인 명령마다 `RUSTC_WRAPPER`·`RUSTC_WORKSPACE_WRAPPER`를 비우고 `RUSTC`를 정해 준다. `PATH`의 `cargo`·`rustc`가
  rustup 프록시면 임시 디렉터리 아래 새 빈 디렉터리에서 rustup에 물은 툴체인으로 `RUSTUP_TOOLCHAIN`도 고정한다.
  사용자가 준 `RUSTUP_TOOLCHAIN`·`RUSTC`는 그대로 쓴다. 이전에는 체크아웃 안에서 띄우면 그 설정이 고른 래퍼·컴파일러·
  툴체인이 실행됐고, `PATH`에 빈 항목이나 `.` 같은 상대 항목이 있으면 체크아웃에 든 `cargo`도 실행됐다. 작업 디렉터리는
  그대로라 source replacement·`[patch]`·레지스트리 설정은 지금처럼 적용된다. 이 보호는 실행할 프로그램의 선택만
  막는다: 체크아웃의 `[env]`도 지금처럼 `cargo metadata`가 띄우는 rustc에 적용되므로, 체크아웃(이나 그 하위
  디렉터리) 안에서 실행하면 그 체크아웃이 `LD_PRELOAD`·`DYLD_INSERT_LIBRARIES` 같은 변수로 자기 라이브러리를 rustc에
  실어 코드를 실행할 수 있다. 신뢰하지 않는 체크아웃은 지금처럼 체크아웃 밖의 작업 디렉터리에서 `--dir`로
  분석한다. 자격 증명 공급자와 `--semantic`의 rust-analyzer가 띄우는 cargo도 이 보호 밖이다 — README의 Security 절.
- `mcp`: 서버가 1 MiB(1,048,576바이트, 줄 끝 제외)를 넘는 요청 줄을 버퍼에 쌓지 않는다. 그런 줄에는 `id`가 `null`인
  JSON-RPC `-32600` 오류로 답하고 그 줄의 나머지를 버린 뒤 다음 요청을 계속 읽는다. 이전에는 줄 길이 상한이 없어
  개행 없는 입력 하나가 서버 메모리를 끝없이 쌓았다.
- `rules`·MCP `rustograph_rules`: `.rustograph.yml`의 `baseline:`이 `--dir` 디렉터리 밖을 가리키지 못한다. 절대 경로,
  `..`로 밖에 나가는 경로, 심볼릭 링크를 따라 밖에 닿거나 대상이 없는 링크를 지나는 경로는 수확·읽기·쓰기 전에
  거부한다. 이전에는 분석 대상 저장소의 설정 한 줄로 `rules --write-baseline`이 저장소 밖 파일을 기준선 텍스트로
  덮어쓰고 중간 디렉터리까지 만들었으며, 밖의 파일을 기준선으로 읽었다. 직접 준 `--baseline FILE`은 실행한 사람이 고른
  경로라 제한하지 않는다. 바뀌는 호출은 아래 Changed 절에 있다.

### Changed

- `reach`·`impact --format language-traversal`: git-lfs처럼 filter로 정규화되던 파일은 git의 stat 캐시가 낡으면 바뀐
  파일로 보여 `revision`을 싣지 않는다(확인하지 못한 깨끗함을 주장하지 않는 fail-safe). 신뢰하는 checkout이면 먼저
  `git status`로 stat 캐시를 새로 고친다. 서브모듈이 있는 작업 트리와 덮을 수 없는 filter 드라이버 이름이 있는 설정은
  `revision`을 싣지 않고 `revision-unverified:` limitation으로 이유를 알린다. `revision`이 필요하면 `--revision`으로 준다.
- `graph --out`: 수확한 그래프를 JSON으로 저장할 때 `--dir` 작업 트리가 깨끗하면 git `HEAD`를 문서의 선택 필드
  `revision`에 기록한다. 그 판독은 순회 `revision`과 같은 판정·같은 git 격리로 `--dir`에서 git을 실행한다. 문서 버전은
  1 그대로이고(값이 없으면 키가 빠진다), 수확한 그래프를 표준 출력으로 낼 때와 Mermaid에는 싣지 않으며,
  `graph --graph`는 입력의 값을 JSON 출력(표준 출력과 `--out` 모두)에 그대로 옮긴다. 추적하지 않는 스냅샷을 작업 트리
  안에 두면 다음 `graph --out`에서 트리가 바뀐 것으로 보여 기록이 빠진다 — 스냅샷은 작업 트리 밖이나 git이 무시하는
  경로(`.rustograph/` 등)에 둔다.
- `reach`·`impact --format language-traversal`의 `--graph`: 현재 git `HEAD` 대신 스냅샷이 기록한 `revision`을 쓰고 git을
  실행하지 않는다. 기록이 없거나(옛 스냅샷, 수확한 그래프의 표준 출력을 리디렉트해 만든 스냅샷, 깨끗함을 확인하지
  못한 트리에서 저장한 스냅샷) `--revision`과 같은 규칙을 어기면 `revision`을 싣지 않고 `revision-unknown:` limitation을
  단다. 이전에는 옛 스냅샷에 현재 `HEAD`를 붙여 stale 분석을 최신처럼 보이게 했다. 필요하면 `--revision`으로 준다.
  `graphRevision`은 스냅샷의 `revision`을 빼고 해시한다(같은 커밋의 스냅샷과 새 수확이 같은 값이다).
- `schema`: SQL 리터럴의 관계 판독이 계열 생산자 공통 규칙을 따른다. 같은 `;` 문장의 CTE 이름(인용하지 않은 이름은
  대소문자 없이 비교), 함수 인자 괄호 바로 안의 `FROM`(`EXTRACT`·`TRIM`·`SUBSTRING` 등), `IS [NOT] DISTINCT FROM`의
  피연산자를 relation-use로 내지 않는다 — isthmus가 이 이름들을 선언과 잇지 못해 거짓 `relation-use-without-decl`
  error를 냈다. 그 자리의 자리표시자·문자열 때문에 나던 `dynamic` 사실과 `unjoined-dynamic-relations` 수도 준다. 규칙이
  문장 단위라 자기 본문에서 같은 이름의 실제 테이블을 읽는 CTE(`WITH users AS (SELECT * FROM users …)`)는 그 사용도
  내지 않는다.
- 수확하는 모든 명령: 작업 디렉터리나 그 조상의 rust-toolchain 파일을 쓰지 않았으면 그래프 문서와 `deps`·`schema`·
  `routes` 문서에 `toolchain-file-ignored:` limitation을 단다 — 그 파일이 고르던 툴체인 대신 rustup이 그런 파일 없는
  곳에서 고르는 툴체인으로 `cargo metadata`를 돌렸다는 뜻이다. 프로젝트 툴체인이 필요하면 `RUSTUP_TOOLCHAIN=<channel>`을
  준다. rustograph 자신의 rustc 프로브(`--target`, semantic 캐시 키)도 cargo와 같은 rustc(사용자가 준 `RUSTC` 포함)를
  쓴다.
- **Breaking**: 수확하는 모든 명령: `PATH`의 `cargo`·`rustc`가 rustup 프록시이고 `RUSTUP_TOOLCHAIN`을 주지 않았는데
  rustup이 빈 임시 디렉터리에서 툴체인을 답하지 못하거나(기본 툴체인 없이 체크아웃의 rust-toolchain 파일에만 기대던
  환경 등) 임시 디렉터리의 조상에 rust-toolchain 파일이 있으면, 분석이 `RUSTUP_TOOLCHAIN`·`RUSTC`를 직접 주라는 오류로
  끝난다. 체크아웃의 rust-toolchain 파일이 고르던 툴체인이어야 매니페스트를 읽을 수 있는 프로젝트(nightly 전용
  `cargo-features` 등)는 고정한 툴체인의 `cargo metadata`가 실패해 분석도 실패한다 — 그 오류 끝에 툴체인 파일을 쓰지
  않았다는 사실, `cargo metadata`를 돌린 툴체인 이름, `RUSTUP_TOOLCHAIN=<channel>` 안내가 붙는다. 이전에는 체크아웃
  안에서 띄우면 그 툴체인 파일을 따라 돌았다.
- `query`·`impact`, MCP `rustograph_query`: `--depth 0`(`depth: 0`)이 MCP `rustograph_impact`와 같이 깊이 제한 없는
  전체 폐포(`--max`까지)를 뜻한다. 이전에 CLI `query`·`impact`와 MCP `rustograph_query`는 같은 `depth: 0`,
  `truncated: false`로 빈 결과를 내 의존자가 없는 것처럼 보였다. 보고 `depth`는 0 그대로이고 기본 깊이도 그대로다(1,
  MCP `rustograph_impact`는 0). 순회 형식의 `--depth 0`(상한 128)과 라이브러리 `analysis::query`·`analysis::impact`의
  0(이웃 없음)은 바뀌지 않았다.
- **Breaking**: `graph`·`cycles`·`dead`·`rules`·`deps`·`query`·`impact`·`paths`·`search`·`mcp`가 쓰지 않는 플래그와
  남는 위치 인자(예: `cycles --out`, `query --format`, `paths --role`, `dead --level`, `deps --out`, `query ID EXTRA`)를
  조용히 무시하지 않고 사용법 오류(종료 2)로 거부한다. 이전에는 무시하고 0으로 끝났다(`cycles --out`은 파일을 쓰지
  않았다). 그래프를 읽는 명령은 `--dir`·`--graph`와 공유 플래그(`--deps`·`--tests`·`--retain-public`·`--semantic`·
  `--no-cache`·`--focus`·`--target`·`--exclude-tests`)를 계속 받는다. `impact --format`은 `language-traversal`만 받는다.
- **Breaking**: MCP `rustograph_deps`(`mcp --graph`): `--dir` 워크스페이스가 스냅샷의 `workspace`와 같은 디렉터리일
  때만 답하고(심볼릭 링크 표기도 같은 디렉터리로 본다), 다르거나 스냅샷에 절대 경로 `workspace`가 없으면 isError로
  답한다. 이전에는 다른 저장소의 보고서를 isError 없이 답했다. 스냅샷이 기록한 경로에서는 `cargo`를 실행하지 않는다.
  `--dir`을 스냅샷의 워크스페이스로 주거나 `--graph` 없이 띄운다.
- **Breaking**: `query`·`impact`·`search`의 `--max`(MCP `max`)는 10,000, `query`·`impact`의 `--depth`(MCP `depth`)는
  64, `paths`의 `--max`(MCP `max`)는 1,024, `--budget`(MCP `budget`)은 1,000,000까지다. 넘는 값은 조용히 깎지 않고
  CLI 사용법 오류(종료 2)와 MCP isError로 거부한다. 이전에는 그대로 받아서 `paths` 호출 하나가 메모리를 수 GB까지
  썼다. `--depth 0`은 여전히 깊이 제한 없음이고, 기본값과 순회 형식의 상한은 그대로다. MCP `tools/list` 입력 스키마가
  상한을 `maximum`으로 싣는다.
- `paths`(MCP `rustograph_paths` 포함): 탐색 상태를 경로 복제 대신 40바이트 칸으로 둬, 같은 예산에서 메모리가 약
  1/7, 시간이 약 1/3이다(출력 같음).
- **Breaking**: `rules`·MCP `rustograph_rules`: `.rustograph.yml`의 모르는 키(`components:` 대신 쓴 `component:` 같은
  오타), `components`에 없는 컴포넌트를 가리키는 `deps`·`deny`·`signature` 키, 컴포넌트가 하나도 없는 설정(빈 파일,
  `components: {}`)을 설정 오류(종료 2, MCP isError)로 거부하고 원인을 알린다. 이전에는 오타 하나로 규칙이 꺼진 채
  `rules --strict`가 0 위반·종료 0으로 통과했다. 키만 검사하므로 목록에는 지금처럼 컴포넌트 이름이나 모듈 패턴을 쓴다.
- **Breaking**: `rules`·MCP `rustograph_rules`: 설정의 `baseline:`은 설정 파일 디렉터리 기준 상대 경로로 `--dir`
  디렉터리 안이어야 하고, 어기면 수확 전에 설정 오류(종료 2, MCP isError)다 — 막는 경로는 위 Security 항목에 있다.
  안에 머무는 `..`(`--config sub/.rustograph.yml`의 `../base.txt`)는 된다. `--dir` 밖 `--config`(`--config /dev/stdin`
  포함)의 상대 `baseline:`도 이제 오류다 — 그런 기준선은 `--baseline FILE`로 준다. `baseline:`에 `..`가 있으면
  `rules --write-baseline`의 "wrote baseline" 줄이 정규화한 경로를 보인다.
- 그래프를 수확하는 명령(`graph`·`cycles`·`dead`·`rules`·`query`·`impact`·`paths`·`search`·`reach`·MCP 등): 수신자 타입을
  모르는 메서드 호출 `x.m()`의 이름 팬아웃이 고유·트레이트 선언 메서드를 호출 크레이트, 같은 패키지의 lib, 그 크레이트가
  의존하는 크레이트(전이, 종류 전부 — dev·build와 기본 feature가 켜지 않는 optional 의존 포함)에서만 고른다. 트레이트
  impl 메서드는 지금처럼 워크스페이스 전체에서 고른다. 같은 이름 메서드가 전부 닿을 수 없는 호출은 간선 없이 새
  limitation으로 센다. 이전에는 의존 관계가 없는 크레이트의 같은 이름 메서드로도 추정 간선을 내 `dead`가 그런 메서드를
  살아 있다고 보고했다 — 이제 추정 간선과 fan-out 계수가 줄고, `dead`가 그런 메서드를 unreachable로 보고할 수 있다.
  `--semantic`이 타입으로 풀지 못한 호출도 같다. `cargo metadata`가 resolve 그래프를 주지 않으면 좁히지 않는다.
- 같은 명령: 좁힌 뒤에도 같은 이름 후보가 4,096개를 넘는 호출은 간선 없이 limitation으로 센다 — 그런 호출로만 닿는
  메서드는 `dead`가 unreachable로 보고하고, 그 호출자는 `query`·`impact`·`reach`에 나오지 않는다. 그 limitation은
  그래프 문서와 함께 `dead --format json`과 MCP `rustograph_dead` 보고서의 `limitations`에도 실린다(`dead` 텍스트에는
  한 번 나온다). 실제 저장소에서는 걸리지 않는 안전 상한이다(rust-analyzer 27크레이트에서 가장 넓은 호출이 후보
  544개).
- `schema`·`routes`의 `--out`: 입력 보호 목록을 만들 때 워크스페이스를 다시 읽고 파싱하는 대신 문서를 만든 수확의
  입력을 쓴다(8만 줄 합성 워크스페이스에서 시간 −26~−41%, 최대 RSS −25~−30%, 출력 같음). `graph --out`·
  `rules --write-baseline`의 보호 목록은 같은 소스 파일을 정점마다 싣지 않아, 기존 출력을 덮어쓸 때 경로 확인이 정점
  수가 아니라 파일 수에 비례한다(rust-analyzer 5크레이트 스냅샷 덮어쓰기 0.275 → 0.084초, 출력 같음).
- 그래프를 읽는 명령의 `--focus`(`graph`·`dead`·`cycles`·`reach`·`impact --format language-traversal`·MCP 등): 포커스가
  버린 보존 루트와 포커스 경계를 넘는 의존 간선(contains 제외, 추정 간선 포함) 수를 limitation 한 문장("--focus
  dropped N retention roots and M dependency edges that cross the focus boundary; …")으로 알린다. 그래프 문서,
  `dead`·`cycles` 텍스트, 순회 문서, MCP summary가 싣고, `dead --format json`과 MCP `rustograph_dead` 보고서의
  `limitations`에도 실린다. 이전에는 포커스 밖의 루트·호출자를 잃은 `dead`·`impact`·`reach` 결과가 한계 표시 없이
  완전한 답처럼 나왔다. 버린 것이 없으면 싣지 않는다. 포커스 순회 문서의 `graphRevision`은 그 문장만큼 달라진다.
  완전한 결과는 `--focus` 없이 얻는다.
- 그래프를 만드는 모든 명령: 모듈·크레이트 정점의 `generated`가 같은 파일의 아이템·impl 메서드 정점과 같은 규칙을
  쓴다 — 파일 앞 10줄 가운데 한 줄에 `@generated`·`auto-generated`·`autogenerated`가 있거나 `generated`와
  `do not edit`가 함께 있으면 참이다(대소문자 무시). 이전에는 모듈 정점이 `do not edit`만 있어도 참이고
  `autogenerated`에는 거짓이라 같은 파일의 모듈과 함수가 서로 반대일 수 있었다. 이제 `generated` 없는 `DO NOT EDIT`
  파일(두 낱말이 다른 줄에 있는 파일 포함)의 모듈 정점은 거짓, `autogenerated` 파일의 모듈 정점은 참이다.
- 수확하는 모든 명령(`graph`·`dead`·`rules`·`deps`·`routes`·MCP 등): 64 MiB를 넘는 모듈 파일(`mod` 파일·크레이트
  루트)을 읽지 않는다. 그 모듈·크레이트 정점은 남고 그 파일이 선언하는 아이템과 하위 모듈은 빠지며, 그래프 문서
  limitation이 그런 파일 수를 센다("N module files larger than 64 MiB were not read; …"). 이전에는 상한 없이 통째로
  읽어(1 GiB sparse 파일에 최대 RSS 약 2 GB) 정상 Rust면 모두 파싱하고 아니면 종료 2였다. 그런 파일이 있는 동안
  `--out`은 기존 출력의 교체를 거부한다(새 경로·표준 출력에는 쓴다). `schema`는 그 파일을 `unparsed-sources`로 세고,
  `routes`·`deps`·`dead --format json`은 그 limitation을 싣지 않는다.
- `rules --format sarif`: `artifactLocation.uri`가 로컬 절대 경로 대신 Cargo 워크스페이스 루트 기준 상대 URI다
  (`uriBaseId: "%SRCROOT%"`, `/` 구분자, RFC 3986 퍼센트 인코딩 — `:`도 인코딩한다). 루트의 절대 경로는 어디에도
  싣지 않는다(`originalUriBaseIds` 없음). 루트 밖 파일(루트를 벗어나는 `#[path]` 모듈 등)의 위반은 location 없이
  message 끝에 " (location omitted: the file is outside the workspace root)"를 붙인다. 이전에는 정점 위치의 절대 경로를
  인코딩 없이 그대로 실어, 올린 결과에 로컬 경로가 남고 `#`·공백·`C:\` 경로가 깨졌다. `--graph`는 스냅샷이 기록한
  워크스페이스 루트가 기준이다.
- 그래프를 수확하는 모든 명령: 수확이 함수 본문 구문 트리를 복제하지 않고 파싱한 트리에서 빌린다(출력 같음 —
  rust-analyzer 5크레이트 `graph --level symbol` 최대 RSS 523 → 350MB).
- **Breaking**(Rust 라이브러리): 일부 호출자의 소스 호환이 깨진다 — `graph::Document`의 새 pub 필드 `revision`,
  `harvest::Harvest`·`cargo_meta::Package`·`sem::Stats`(semantic feature)의 새 pub 필드, `harvest::ImplBlock<'a>`와
  `BodyItem.body`(`exprs` 대신 새 `harvest::Body<'a>`), `ModuleDecls.impls`·`harvest::impls`의 수명. struct literal로
  만들거나 전 필드를 분해하거나 `exprs`를 읽던 코드는 `docs/MIGRATING-0.5.md`의 "Rust library callers"를 따른다.
  `config::load_file`·`config::load`는 위 `rules` 설정 검증을 함께 하고, `Config` 역직렬화가 모르는 키를 거부한다.
  `config::resolve_baseline`이 새로 생겼다.

## 0.5.0 이하

이 파일은 v0.5.0 발행(2026-10-13) 뒤에 생겼다. 그 전 릴리스의 변경 내용은 GitHub Release 노트에 있다
(날짜는 발행 시각의 UTC 날짜).

- [0.5.0](https://github.com/ictechgy/rustograph/releases/tag/v0.5.0) - 2026-10-13 — Rust 라이브러리 호출자를 깨는
  변경(`Document::view`·`analysis::dead`가 `Result`를 돌려준다, [0.5 migration](docs/MIGRATING-0.5.md))
- [0.4.1](https://github.com/ictechgy/rustograph/releases/tag/v0.4.1) - 2026-10-01
- [0.4.0](https://github.com/ictechgy/rustograph/releases/tag/v0.4.0) - 2026-09-30
- [0.3.0](https://github.com/ictechgy/rustograph/releases/tag/v0.3.0) - 2026-09-25
- [0.2.1](https://github.com/ictechgy/rustograph/releases/tag/v0.2.1),
  [0.2.0](https://github.com/ictechgy/rustograph/releases/tag/v0.2.0),
  [0.1.0](https://github.com/ictechgy/rustograph/releases/tag/v0.1.0) - 2026-09-22
