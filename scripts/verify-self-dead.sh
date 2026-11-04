#!/bin/bash
# verify-self-dead.sh — 자기 분석 dead 결과를 기대 목록(scripts/self-dead.expected)과 대조한다.
#
# rules·cycles 자기 분석은 위반 0이 기준이지만, dead는 구문 분석이 끝내 보지 못하는 사용이 남는다
# (기대 목록의 주석이 이유를 적는다). 그래서 0 대신 "알려진 것만 남았는가"를 본다 — 새 unreachable이
# 생기면(수확 회귀이거나 진짜 미사용) 실패하고, 기대 목록의 항목이 사라지면(수확이 좋아졌다) 목록을
# 줄이라고 실패한다. 공개 API·테스트·외부 트레이트 impl 보존을 켠다: 라이브러리 크레이트라 공개
# 아이템의 호출자가 저장소 밖에 있고, 테스트 도우미는 테스트에서만 쓰인다.
set -euo pipefail
cd "$(dirname "$0")/.."

BIN="${1:-}"
if [ -z "$BIN" ]; then
	cargo build --quiet --locked
	BIN="$(pwd)/target/debug/rustograph"
fi
ACTUAL="$(mktemp)"
trap 'rm -f "$ACTUAL"' EXIT

"$BIN" dead --dir . --retain-public --tests --retain-external-trait-impls \
	| sed -n 's/^unreachable [a-z]*: //p' | LC_ALL=C sort > "$ACTUAL"

EXPECTED="scripts/self-dead.expected"
if ! diff -u <(grep -v '^#' "$EXPECTED" | sed '/^$/d' | LC_ALL=C sort) "$ACTUAL"; then
	echo "self-analysis dead differs from $EXPECTED (- expected, + actual)." >&2
	echo "A new finding is a harvest regression or real dead code; a vanished one means the" >&2
	echo "harvest improved — update $EXPECTED with the reason either way." >&2
	exit 1
fi
echo "self-analysis dead matches $EXPECTED ($(wc -l < "$ACTUAL" | tr -d ' ') known findings)"
