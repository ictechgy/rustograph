#!/bin/bash
# 컴파일러·네트워크 없이 matrix 커밋 및 바이트 결합을 검증한다.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
scratch=$(mktemp -d)
trap 'rm -rf "$scratch"' EXIT
commit=1111111111111111111111111111111111111111
other=2222222222222222222222222222222222222222
passed=0
helper="$root/scripts/release-build-manifest.sh"
run_case() {
  local scenario=$1 expect=$2 result=0 target file
  mkdir "$scratch/$scenario"
  (
    cd "$scratch/$scenario"
    for target in darwin-arm64 darwin-amd64 linux-amd64 linux-arm64 windows-amd64; do
      file="rustograph-0.5.0-$target.tar.gz"
      [[ $target != windows-amd64 ]] || file="rustograph-0.5.0-$target.zip"
      printf 'synthetic %s\n' "$target" > "$file"
      "$BASH" "$helper" record 0.5.0 "$commit" "$target"
    done
    file=rustograph-0.5.0-linux-amd64.tar.gz
    case "$scenario" in
      complete) ;;
      missing-archive) rm "$file";;
      missing-manifest) rm "$file.build-info";;
      changed-bytes) printf 'changed' >> "$file";;
      wrong-commit) sed "s/$commit/$other/" "$file.build-info" > tmp; mv tmp "$file.build-info";;
      wrong-name) sed 's/linux-amd64/linux-arm64/' "$file.build-info" > tmp; mv tmp "$file.build-info";;
      extra-archive) cp "$file" rustograph-extra.tar.gz;;
      extra-manifest) cp "$file.build-info" rustograph-extra.build-info;;
      linked-archive) mv "$file" original; ln -s original "$file";;
      linked-manifest) mv "$file.build-info" original; ln -s original "$file.build-info";;
      empty-archive) : > "$file";;
      extra-record) cat "$file.build-info" >> "$file.build-info.copy"; cat "$file.build-info.copy" >> "$file.build-info";;
    esac
    "$BASH" "$helper" verify 0.5.0 "$commit" > output 2>&1 || result=$?
    if [[ $expect == pass ]]; then [[ $result == 0 ]] || { cat output >&2; exit 1; }
    else [[ $result != 0 ]] || { echo "Unexpected success: $scenario" >&2; exit 1; }; fi
    if [[ $scenario == complete ]]; then
      # 같은 경로의 기존 manifest 재기록도 허용하지 않는다.
      ! "$BASH" "$helper" record 0.5.0 "$commit" linux-amd64 > /dev/null 2>&1
      ! "$BASH" "$helper" verify 0.5.0 "$other" > /dev/null 2>&1
      ! "$BASH" "$helper" record 0.5.0 "$commit" ../bad > /dev/null 2>&1
      ! "$BASH" "$helper" verify '0.5.0;false' "$commit" > /dev/null 2>&1
      ! "$BASH" "$helper" verify 0.5.0 short > /dev/null 2>&1
    fi
  )
  passed=$((passed+1)); printf 'PASS build manifest %s\n' "$scenario"
}
run_case complete pass
for scenario in missing-archive missing-manifest changed-bytes wrong-commit wrong-name extra-archive extra-manifest linked-archive linked-manifest empty-archive extra-record; do
  case "$(uname -s)/$scenario" in
    MINGW*/linked-*) printf 'SKIP %s: Git Bash may emulate symbolic links.\n' "$scenario"; continue;;
  esac
  run_case "$scenario" fail
done
# 파일이 전혀 없을 때도 Bash 3.2에서 명확한 실패를 유지한다.
mkdir "$scratch/empty"
(cd "$scratch/empty"; ! "$BASH" "$helper" verify 0.5.0 "$commit" > /dev/null 2>&1)
printf '%s build-manifest scenarios and six input/empty controls passed.\n' "$passed"
