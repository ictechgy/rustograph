#!/bin/bash
# verify-cli-contract.sh — 빌드된 바이너리로 종료 코드 계약(0/1/2)을 검증한다.
# 인자 없으면 cargo build로 새 바이너리를 만든다 — 낡은 바이너리로 검증해
# "수정이 안 먹는다"를 오해하는 사고를 막기 위함이다(gartograph 관례).
set -euo pipefail
cd "$(dirname "$0")/.."

BIN="${1:-}"
if [ -z "$BIN" ]; then
	cargo build --quiet
	BIN="$(pwd)/target/debug/rustograph"
fi
trap 'rm -rf "$FIX"' EXIT

# fixture: tests/fixture — app → core 크로스 크레이트 호출, 미도달 심볼 존재.
FIX="$(mktemp -d)"
cp -R tests/fixture "$FIX/fixture"
cat > "$FIX/rules.yml" <<'EOF'
components:
  app: ["fixture_app/**"]
  core: ["fixture_core/**"]
deps:
  app: [core]
  core: []
EOF

fails=0
check() { # check <기대 코드> <설명> <인자...>
	local want="$1" desc="$2"; shift 2
	local got=0
	# || 로 잡아야 set -e가 비0 종료에서 스크립트를 죽이지 않는다 —
	# 종료 코드 1 자체가 검증 대상이다.
	"$BIN" "$@" --dir "$FIX/fixture" >/dev/null 2>&1 || got=$?
	if [ "$got" -ne "$want" ]; then
		echo "FAIL $desc: expected $want, got $got" >&2
		fails=$((fails+1))
	fi
}

check 0 "graph"             graph
check 0 "graph crate"       graph --level crate
check 0 "graph module"      graph --level module
check 0 "graph symbol"      graph --level symbol
check 0 "graph deps"        graph --deps
check 0 "graph mermaid"     graph --format mermaid
check 0 "cycles strict"     cycles --strict
check 0 "dead"              dead
check 1 "dead strict"       dead --strict
check 0 "dead tests"        dead --tests
check 0 "dead retain-pub"   dead --retain-public
check 0 "query"             query fixture_core::entry
check 2 "query missing"     query fixture_core::missing
check 0 "impact"            impact fixture_core::Used
check 2 "impact missing"    impact fixture_core::missing
check 2 "rules no config"   rules --strict
check 0 "rules"             rules --config "$FIX/rules.yml"
check 0 "rules sarif"       rules --config "$FIX/rules.yml" --format sarif
check 0 "version"           version
check 2 "unknown command"   frobnicate
check 2 "bad level"         graph --level bogus
check 2 "bad format"        graph --format xml

# --dir를 붙이지 않는 검사 — check()는 항상 fixture dir을 뒤에 붙이므로
# 나쁜 --dir 검증은 마지막 인자가 이기는(last-wins) 구조상 여기서 따로 한다.
for c in "graph --dir /nonexistent-xyz"; do
	got=0
	# shellcheck disable=SC2086
	"$BIN" $c >/dev/null 2>&1 || got=$?
	if [ "$got" -ne 2 ]; then
		echo "FAIL $c: expected 2, got $got" >&2
		fails=$((fails+1))
	fi
done

if [ "$fails" -gt 0 ]; then
	echo "$fails contract checks failed" >&2
	exit 1
fi
echo "cli contract OK"
