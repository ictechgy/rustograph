# 릴리스 게시

버전·PR 검증이 끝난 정확한 커밋에 새 `vX.Y.Z` 태그를 만든다. 기존 Release workflow는
각 matrix에서 태그·Cargo 버전·체크아웃 SHA를 먼저 확인하고, 다섯 대상 빌드 뒤 각각의
압축 파일에 built commit·SHA-256·파일 이름을 담은 `.build-info`를 붙인다. 게시 job은
자신의 정확한 소스 커밋과 다섯 기록/파일이 모두 일치하는지 확인한다. 일부 matrix만 다른
태그 시점을 빌드했거나 다른 바이트·추가/누락 산출물이 있으면 게시하지 않는다.
그 뒤 기존 macOS arm64 압축 바이너리의 전체 CLI 계약과 체크섬 생성 관문을 통과해야 한다.
태그·버전 대조와 Homebrew 갱신 순서는 그대로다. 태그를 만들거나 옮기는 일은 게시 helper가 하지 않는다.
workflow 기본 토큰은 `contents: read`이고 게시 job만 `contents: write`를 받는다. 모든 checkout은
`persist-credentials: false`로 토큰을 남기지 않고, 액션은 커밋 SHA로 고정해 Dependabot(github-actions)이 갱신을 제안한다.

GitHub UI에서 먼저 릴리스를 만드는 경우에는 정확한 대상 커밋을 선택하고, 노트에 바이너리 게시가
아직 진행 중임을 적는다. 릴리스 페이지가 생겼다는 이유만으로 바이너리 배포 완료로 보지 않는다.
일반 공개 릴리스(`draft=false`, `prerelease=false`)에서만 누락 자산을 채우며 제목·본문·표시는 바꾸지 않는다.
이미 존재하는 예상 자산은 `uploaded` 상태·크기·SHA-256이 이번에 검증한 파일과 같아야 재사용한다.
다른 바이트·누락 digest·잘못된 커밋·API 오류는 게시를 중단한다. 다른 이름의 사용자 자산은 그대로 둔다.

helper는 여섯 예상 자산(다섯 압축 파일과 `checksums.txt`)을 모두 대조한 뒤에만 누락 이름을 올린다.
기존 자산에 `--clobber`·삭제를 적용하거나 태그·권한을 바꾸지 않는다. 실패하거나 결과가 불명확한 업로드는 재조회로
확인하며, 누락이 남으면 같은 검증 산출물로만 재시도한다. 다시 빌드한 압축 파일은 시간 정보 등으로
해시가 달라질 수 있으므로, 이름만 같다는 이유로 성공 처리하지 않는다. 태그 확인과 업로드는
원자적 트랜잭션이 아니다. 업로드 뒤 동시 태그 이동을 발견하면 실패하며, 이미 추가한 자산을
자동으로 삭제하지 않으므로 재시도 전에 해당 실행을 확인한다.

이미 공개된 immutable 릴리스는 누락 자산을 추가할 수 없다. helper는 이 경우 설정을 바꾸지 않고 실패한다.
릴리스가 없으면 기존 `gh release create --verify-tag` 경로를 유지한다. gh는 새 draft에 자산을
첨부한 뒤 공개하며, 업로드 실패 시 그 새 draft를 정리하는 동작도 그대로다. immutable 정책에서는
이처럼 자산 첨부를 마친 뒤 공개해야 한다.
릴리스 메타데이터에서 immutability 여부를 확인하지 못해도 게시를 중단한다.

최종 성공은 태그가 빌드 커밋을 가리키고 여섯 자산이 모두 검증된 바이트일 때다. 공개 다운로드·설치까지
확인한 뒤 준비 중인 노트를 갱신한다. Homebrew tap 자격 증명이 없으면 이전처럼 자동 갱신을 건너뛴다.

crate registry 발행은 이 GitHub 바이너리 workflow에 포함되지 않는다.
오프라인 matrix provenance 검사: `bash scripts/test-release-build-manifest.sh`.
Git Bash에서는 링크 fixture 두 건을 생략하며, Ubuntu/macOS가 실제 링크 거절을 검증한다.
오프라인 회귀 검사: `bash scripts/test-publish-release-assets.sh`. 모든 GitHub 요청은 가짜 `gh`가 처리한다.
