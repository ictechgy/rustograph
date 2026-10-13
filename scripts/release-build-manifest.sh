#!/bin/bash
# 각 matrix 빌드의 커밋·압축 파일 바이트를 묶어, 서로 다른 태그 시점의 산출물 혼합을 막는다.
set -euo pipefail
fail() { printf 'Release build manifest stopped: %s\n' "$*" >&2; exit 1; }
[[ $# -ge 3 ]] || fail 'expected record|verify, version, built commit, and optional target'
mode=$1 version=$2 commit=$3
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail 'invalid stable version'
[[ $commit =~ ^[0-9a-f]{40}$ ]] || fail 'expected a full built commit SHA'
files=("rustograph-$version-darwin-arm64.tar.gz" "rustograph-$version-darwin-amd64.tar.gz"
       "rustograph-$version-linux-amd64.tar.gz" "rustograph-$version-linux-arm64.tar.gz"
       "rustograph-$version-windows-amd64.zip")

archive_hash() {
  [[ -f $1 && ! -L $1 && -s $1 ]] || fail "missing regular archive: $1"
  local result
  # Git Bash/Ubuntu의 coreutils와 macOS의 기존 Perl 도구만 사용한다.
  if command -v sha256sum >/dev/null 2>&1; then result=$(sha256sum "$1")
  else result=$(shasum -a 256 "$1"); fi
  hash=${result%% *}
  [[ $hash =~ ^[0-9a-f]{64}$ ]] || fail 'invalid SHA-256 result'
}

case "$mode" in
  record)
    [[ $# == 4 ]] || fail 'record requires exactly one target'
    case "$4" in
      darwin-arm64|darwin-amd64|linux-amd64|linux-arm64) file="rustograph-$version-$4.tar.gz";;
      windows-amd64) file="rustograph-$version-$4.zip";;
      *) fail 'unsupported build target';;
    esac
    archive_hash "$file"
    # 기존 기록을 조용히 덮어쓰지 않는다. 재빌드는 새 runner에서 검증한다.
    (set -o noclobber; printf '%s\t%s\t%s\n' "$commit" "$hash" "$file" > "$file.build-info") ||
      fail "build manifest already exists or could not be written: $file.build-info"
    ;;
  verify)
    [[ $# == 3 ]] || fail 'verify accepts no target'
    shopt -s nullglob
    archives=(rustograph-*.tar.gz rustograph-*.zip)
    manifests=(rustograph-*.build-info)
    # 빈 배열을 nounset로 확장하지 않도록, 길이는 안전한 대체 확장으로 센다.
    set -- ${archives[@]+"${archives[@]}"}
    [[ $# == 5 ]] || fail 'expected exactly five archives'
    set -- ${manifests[@]+"${manifests[@]}"}
    [[ $# == 5 ]] || fail 'expected exactly five build manifests'
    for file in "${files[@]}"; do
      archive_hash "$file"
      [[ -f $file.build-info && ! -L $file.build-info ]] || fail "missing regular build manifest: $file"
      expected="$commit"$'\t'"$hash"$'\t'"$file"
      [[ $(cat "$file.build-info") == "$expected" ]] || fail "build commit, archive name, or SHA-256 mismatch: $file"
    done
    printf 'Verified five build manifests at %s.\n' "$commit"
    ;;
  *) fail 'expected record or verify';;
esac
