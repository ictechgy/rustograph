# Changelog

이 프로젝트의 주요 변경 사항을 기록한다.

## [Unreleased]

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
