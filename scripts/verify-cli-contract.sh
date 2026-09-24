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

# 신규 질의 — paths/search/deps와 문서 필터의 종료 코드 계약.
check 0 "paths"             paths fixture_app::main fixture_core::entry
check 2 "paths missing to"  paths fixture_app::main fixture_core::nope
check 2 "paths one arg"     paths fixture_app::main
check 2 "paths ambiguous"   paths nest fixture_core::entry
check 0 "search"            search entry
check 0 "deps"              deps
check 0 "deps json"         deps --format json
check 1 "deps strict"       deps --strict
check 2 "deps bad format"   deps --format xml
check 0 "graph focus"       graph --focus fixture_core::t
check 0 "graph exclude"     graph --exclude-tests
check 2 "tests xor"         dead --tests --exclude-tests
check 0 "graph target"      graph --target x86_64-pc-windows-msvc

# rules baseline — 얼리기와 억눌림 계약. app이 core를 참조하지 못하게
# 거꾸로 선 룰셋으로 위반을 만든다.
cat > "$FIX/deny.yml" <<'EOF'
components:
  app: ["fixture_app/**"]
  core: ["fixture_core/**"]
deps:
  app: []
  core: []
EOF
check 1 "rules deny"        rules --strict --config "$FIX/deny.yml"
check 0 "baseline write"    rules --config "$FIX/deny.yml" --baseline "$FIX/base.txt" --write-baseline
check 0 "baseline strict"   rules --strict --config "$FIX/deny.yml" --baseline "$FIX/base.txt"
check 2 "baseline missing"  rules --config "$FIX/deny.yml" --baseline /nonexistent-xyz.txt
grep -q "allow|fixture_app" "$FIX/base.txt" || {
	echo "FAIL baseline file: no violation key written" >&2
	fails=$((fails+1))
}

# --semantic — opt-in feature 계약: feature 빌드면 분석 성공(0), 아니면
# 무엇을 빌드해야 하는지 알려주는 명확한 오류(2)여야 한다. 조용한 syn
# 폴백은 거짓 계약이라 허용하지 않는다.
got=0
sem_err="$("$BIN" graph --semantic --dir "$FIX/fixture" 2>&1 >/dev/null)" || got=$?
if [ "$got" -eq 2 ]; then
	echo "$sem_err" | grep -q "semantic" || {
		echo "FAIL graph --semantic: exit 2 but no feature guidance" >&2
		fails=$((fails+1))
	}
elif [ "$got" -ne 0 ]; then
	echo "FAIL graph --semantic: expected 0 or 2, got $got" >&2
	fails=$((fails+1))
fi

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

# mcp — stdio 서버는 EOF까지 읽는다: initialize+tools/list를 밀어 넣고
# 정상 종료(0)와 도구 9개가 나오는지 본다.
mcp_out="$(printf '%s\n' \
	'{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
	'{"jsonrpc":"2.0","id":2,"method":"tools/list"}' \
	| "$BIN" mcp --dir "$FIX/fixture" 2>/dev/null)" || {
	echo "FAIL mcp: exit $? " >&2
	fails=$((fails+1))
}
echo "$mcp_out" | grep -q "rustograph_rules" || {
	echo "FAIL mcp: tools/list missing rustograph_rules" >&2
	fails=$((fails+1))
}
for tool in rustograph_paths rustograph_search rustograph_deps; do
	echo "$mcp_out" | grep -q "$tool" || {
		echo "FAIL mcp: tools/list missing $tool" >&2
		fails=$((fails+1))
	}
done

if [ "$fails" -gt 0 ]; then
	echo "$fails contract checks failed" >&2
	exit 1
fi
echo "cli contract OK"
