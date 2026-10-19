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

### Changed

- `reach`·`impact --format language-traversal`: git-lfs처럼 filter로 정규화되던 파일은 git의 stat 캐시가 낡으면 바뀐
  파일로 보여 `revision`을 싣지 않는다(확인하지 못한 깨끗함을 주장하지 않는 fail-safe). 신뢰하는 checkout이면 먼저
  `git status`로 stat 캐시를 새로 고친다. 서브모듈이 있는 작업 트리와 덮을 수 없는 filter 드라이버 이름이 있는 설정은
  `revision`을 싣지 않고 `revision-unverified:` limitation으로 이유를 알린다. `revision`이 필요하면 `--revision`으로 준다.
