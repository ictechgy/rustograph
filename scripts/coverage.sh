#!/bin/bash
# coverage.sh — 테스트 + 커버리지 게이트(기준 90%).
# cargo llvm-cov가 필요하다: cargo install cargo-llvm-cov
# 최신 Rust는 llvm-tools-preview가 llvm-tools로 이름이 바뀌어
# cargo-llvm-cov가 컴포넌트를 못 찾는다 — rustlib/bin에서 직접 지정한다.
# 사용: scripts/coverage.sh [--report]
set -euo pipefail
cd "$(dirname "$0")/.."

GATE=90
HOST="$(rustc -vV | awk '/^host:/ {print $2}')"
TOOLCHAIN_BIN="$(rustc --print sysroot)/lib/rustlib/$HOST/bin"
# PATH의 rustc가 Homebrew처럼 llvm-tools를 안 싣는 배포면 rustup 툴체인에서 찾는다.
if [ ! -x "$TOOLCHAIN_BIN/llvm-cov" ] && command -v rustup >/dev/null; then
	TOOLCHAIN_BIN="$(rustup show home)/toolchains/$(rustup show active-toolchain | awk '{print $1}')/lib/rustlib/$HOST/bin"
fi
export LLVM_COV="${LLVM_COV:-$TOOLCHAIN_BIN/llvm-cov}"
export LLVM_PROFDATA="${LLVM_PROFDATA:-$TOOLCHAIN_BIN/llvm-profdata}"

mkdir -p coverage
OUT=$(cargo llvm-cov --workspace --summary-only 2>/dev/null)

TOTAL=$(echo "$OUT" | awk '/^TOTAL/ {for(i=1;i<=NF;i++) if ($i ~ /%$/ && $i+0 > 0) t=$i; gsub(/%/,"",t); print t+0}')

if [ "${1:-}" = "--report" ]; then
	echo "$OUT"
fi

echo "total coverage: ${TOTAL}% (gate ${GATE}%)"
awk -v t="$TOTAL" -v g="$GATE" 'BEGIN{exit (t+0 >= g+0) ? 0 : 1}' || {
	echo "coverage below gate" >&2
	exit 1
}
