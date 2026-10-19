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
check 0 "--version"         --version
# --version은 isthmus 설치 버전 대조가 읽는 별칭이다 — 하위 명령과 출력이 같아야 한다.
if [ "$("$BIN" --version)" != "$("$BIN" version)" ]; then
	echo "FAIL --version: output differs from version" >&2
	fails=$((fails+1))
fi
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

# 깊은 입력 — 재귀 Tarjan은 DFS 경로 길이만큼 스택을 써서 수만 정점 고리에서
# 프로세스가 abort(134)했다. 계약 밖 종료 코드라 실제 바이너리로 본다 — 고리는
# 순환 하나라 --strict는 1이다. --graph가 있으면 check()가 붙이는 --dir는 무시된다.
awk -v n=100000 'BEGIN {
	printf "{\"version\":1,\"tool\":\"rustograph\",\"level\":\"symbol\",\"root\":\".\",\"roots\":[\"r0\"],\"vertices\":["
	for (i = 0; i < n; i++) printf "%s{\"id\":\"r%d\",\"kind\":\"fn\",\"crate\":\"r\",\"module\":\"r\"}", (i ? "," : ""), i
	printf "],\"edges\":["
	for (i = 0; i < n; i++) printf "%s{\"from\":\"r%d\",\"to\":\"r%d\",\"kind\":\"call\"}", (i ? "," : ""), i, (i + 1) % n
	print "]}"
}' > "$FIX/ring.json"
check 1 "cycles deep ring"  cycles --level symbol --strict --graph "$FIX/ring.json"
# cfg 평가기도 괄호 중첩마다 재귀했다 — not( 10만 겹 cfg 하나로 --exclude-tests가
# abort(134)했다. 상한을 넘는 조건은 문법 오류처럼 미지라 정점이 남는다 — 루트에서
# 안 닿는 정점이 남으니 dead --strict는 1이다(지워졌다면 0).
awk -v n=100000 'BEGIN {
	printf "{\"version\":1,\"tool\":\"rustograph\",\"level\":\"symbol\",\"root\":\".\",\"roots\":[\"c::root\"],\"vertices\":["
	printf "{\"id\":\"c::root\",\"kind\":\"fn\",\"crate\":\"c\",\"module\":\"c\"},"
	printf "{\"id\":\"c::kept\",\"kind\":\"fn\",\"crate\":\"c\",\"module\":\"c\",\"cfg\":\""
	for (i = 0; i < n; i++) printf "not("
	printf "test"
	for (i = 0; i < n; i++) printf ")"
	print "\"}],\"edges\":[]}"
}' > "$FIX/deep-cfg.json"
check 1 "dead deep cfg"     dead --strict --exclude-tests --graph "$FIX/deep-cfg.json"
# 다른 도구가 만든 문서의 ID에 제어 문자(JSON \u001b = ESC)가 있으면 텍스트·순회 출력에
# 옮기지 않고 입력 오류(2)다 — reach도 root 인자 오류(64)가 아니라 문서 오류라 2다.
printf '%s' '{"version":1,"tool":"rustograph","level":"symbol","root":".","roots":["c::r"],"vertices":[{"id":"c::r","kind":"fn","crate":"c","module":"c"},{"id":"c::\u001b[2Jx","kind":"fn","crate":"c","module":"c"}],"edges":[{"from":"c::r","to":"c::\u001b[2Jx","kind":"call"},{"from":"c::\u001b[2Jx","to":"c::r","kind":"call"}]}' > "$FIX/tainted.json"
check 2 "cycles tainted id" cycles --level symbol --graph "$FIX/tainted.json"
check 2 "reach tainted id"  reach c::r --graph "$FIX/tainted.json"

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
# 명시 --baseline·--config는 실행한 사람이 고른 입력이라 파이프·프로세스 치환도 읽는다 —
# 장치를 가리켜도 크기 상한에서 끝나 2다. 설정의 baseline:처럼 저장소가 고른 경로는 정규
# 파일만 연다(tests/integration.rs가 FIFO로 본다).
check 2 "baseline device"   rules --config "$FIX/deny.yml" --baseline /dev/zero
check 1 "config pipe"       rules --strict --config <(cat "$FIX/deny.yml")
check 0 "baseline pipe"     rules --strict --config "$FIX/deny.yml" --baseline <(cat "$FIX/base.txt")
got=0
cat "$FIX/deny.yml" | "$BIN" rules --strict --config /dev/stdin --dir "$FIX/fixture" >/dev/null 2>&1 || got=$?
if [ "$got" -ne 1 ]; then
	echo "FAIL config stdin: expected 1, got $got" >&2
	fails=$((fails+1))
fi
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

# schema — isthmus bridge-facts 교환 문서. 종료 코드에 더해 계약 필드를
# 본다: 사실이 없으면 target은 null이어야 isthmus가 persistence 문서로
# 오인하지 않는다. 쓰지 않는 플래그는 조용히 삼키지 않고 거부(2)한다.
check 0 "schema"            schema
check 2 "schema semantic"   schema --semantic
check 2 "schema graph"      schema --graph "$FIX/base.txt"
check 2 "schema level"      schema --level symbol
check 2 "schema positional" schema stray
check 2 "schema bad out"    schema --out /nonexistent-xyz/facts.json
schema_field() { # schema_field <설명> <패턴> <dir>
	"$BIN" schema --dir "$3" 2>/dev/null | grep -q "$2" || {
		echo "FAIL $1: missing $2" >&2
		fails=$((fails+1))
	}
}
cp -R tests/fixture-schema "$FIX/fixture-schema"
schema_field "schema empty target"  '"target": null'           "$FIX/fixture"
schema_field "schema format"        '"format": "bridge-facts"' "$FIX/fixture-schema"
schema_field "schema platform"      '"platform": "rust"'       "$FIX/fixture-schema"
schema_field "schema target"        '"target": "persistence"'  "$FIX/fixture-schema"
schema_field "schema relation-use"  '"kind": "relation-use"'   "$FIX/fixture-schema"
got=0
"$BIN" schema --dir "$FIX/fixture-schema" --out "$FIX/facts.json" >/dev/null 2>&1 || got=$?
if [ "$got" -ne 0 ] || ! grep -q '"target": "persistence"' "$FIX/facts.json" 2>/dev/null; then
	echo "FAIL schema --out: exit $got or document not written" >&2
	fails=$((fails+1))
fi

# reach·impact --format language-traversal — isthmus 순회 문서. 계열 계약대로
# 사용법 오류는 표준 출력을 비운 채 64, 정점이 아닌 root가 섞이면 문서를
# 쓰고 64다. schema 사실의 usr는 전부 root로 받아져야 한다(0).
check 0  "reach"                reach fixture_app::main
check 0  "impact traversal"     impact --format language-traversal fixture_core::Used fixture_core::entry
check 64 "reach root-not-found" reach fixture_app::main no_such::root
check 64 "reach no roots"       reach
check 64 "reach control char"   reach "$(printf 'a\007b')"
check 64 "reach bad depth"      reach fixture_app::main --max-depth 129
check 64 "reach bad flag"       reach fixture_app::main --max 3
check 64 "impact trav no roots" impact --format language-traversal
trav_out="$("$BIN" reach --dir "$FIX/fixture" 2>/dev/null)" || true
if [ -n "$trav_out" ]; then
	echo "FAIL reach usage error: stdout must be empty" >&2
	fails=$((fails+1))
fi
# pipefail이라 64를 파이프로 넘기면 grep 결과와 무관하게 실패한다 — 먼저 담는다.
rnf_out="$("$BIN" reach --dir "$FIX/fixture" fixture_app::main no_such::root 2>/dev/null)" || true
echo "$rnf_out" | grep -q '"root-not-found"' || {
	echo "FAIL reach root-not-found: document not written" >&2
	fails=$((fails+1))
}
got=0
"$BIN" impact --format language-traversal --dir "$FIX/fixture-schema" \
	--roots-from "$FIX/facts.json" >/dev/null 2>&1 || got=$?
if [ "$got" -ne 0 ]; then
	echo "FAIL schema usr roots: expected 0 (every usr is a vertex), got $got" >&2
	fails=$((fails+1))
fi

# routes --role server|client — isthmus http 문서. --role은 필수다(생략을 한쪽으로
# 읽으면 명령의 뜻이 역할에 따라 흔들린다). 사실 0건이어도 roles가 있으니 target은
# http다(계약의 http 예외).
check 0 "routes"              routes --role server
check 2 "routes no role"      routes
check 2 "routes bad role"     routes --role proxy
check 0 "routes client"       routes --role client
check 2 "routes client fw"    routes --role client --framework axum
check 2 "routes server wrap"  routes --role server --wrappers x.json
check 2 "routes client wrap"  routes --role client --wrappers /nonexistent-xyz.json
check 2 "routes client svc"   routes --role client --service ""
check 2 "routes bad fw"       routes --role server --framework rocket
check 2 "routes semantic"     routes --role server --semantic
check 2 "routes positional"   routes --role server stray
routes_field() { # routes_field <설명> <패턴> <dir>
	"$BIN" routes --role server --dir "$3" 2>/dev/null | grep -q "$2" || {
		echo "FAIL $1: missing $2" >&2
		fails=$((fails+1))
	}
}
cp -R tests/fixture-routes "$FIX/fixture-routes"
routes_field "routes empty target" '"target": "http"'                 "$FIX/fixture"
routes_field "routes roles"        '"server"'                          "$FIX/fixture"
routes_field "routes platform"     '"platform": "rust"'                "$FIX/fixture-routes/axum08"
routes_field "routes axum"         '"dispatch": "specificity"'         "$FIX/fixture-routes/axum08"
routes_field "routes actix"        '"dispatch": "registration-order"'  "$FIX/fixture-routes/actix"
routes_field "routes decl"         '"kind": "route-decl"'              "$FIX/fixture-routes/actix"
# 클라이언트 문서 — reqwest·ureq 호출과 선언된 래퍼. 선언 오류는 사용법 오류(2)다.
cp -R tests/fixture-client "$FIX/fixture-client"
client_field() { # client_field <설명> <패턴> <추가 인자...>
	local desc="$1" pat="$2"; shift 2
	"$BIN" routes --role client --dir "$FIX/fixture-client/app" "$@" 2>/dev/null | grep -q "$pat" || {
		echo "FAIL $desc: missing $pat" >&2
		fails=$((fails+1))
	}
}
client_field "routes client roles"   '"client"'
client_field "routes client call"    '"kind": "route-call"'
client_field "routes client wrapper" '"/w/orders"' --wrappers "$FIX/fixture-client/http-wrappers.json"
client_field "routes client service" '"service": "mobile"' --service mobile
echo '{"format":"http-wrappers","version":1,"wrappers":[],"extra":1}' > "$FIX/bad-wrappers.json"
got=0
"$BIN" routes --role client --dir "$FIX/fixture-client/app" --wrappers "$FIX/bad-wrappers.json" >/dev/null 2>&1 || got=$?
if [ "$got" -ne 2 ]; then
	echo "FAIL routes client bad wrappers: expected 2, got $got" >&2
	fails=$((fails+1))
fi
# 호출 usr도 그래프 정점이어야 impact로 이어진다.
"$BIN" routes --role client --dir "$FIX/fixture-client/app" --out "$FIX/client.json" >/dev/null 2>&1 || {
	echo "FAIL routes client --out: document not written" >&2
	fails=$((fails+1))
}
got=0
"$BIN" impact --format language-traversal --dir "$FIX/fixture-client/app" --roots-from "$FIX/client.json" >/dev/null 2>&1 || got=$?
if [ "$got" -ne 0 ]; then
	echo "FAIL routes client usr roots: expected 0 (every call usr is a vertex), got $got" >&2
	fails=$((fails+1))
fi
# 핸들러 usr는 그래프 정점이어야 reach로 이어진다 — 문서의 usr를 root로 준다.
"$BIN" routes --role server --dir "$FIX/fixture-routes/axum08" --out "$FIX/routes.json" >/dev/null 2>&1 || {
	echo "FAIL routes --out: document not written" >&2
	fails=$((fails+1))
}
got=0
"$BIN" reach --dir "$FIX/fixture-routes/axum08" --roots-from "$FIX/routes.json" >/dev/null 2>&1 || got=$?
if [ "$got" -ne 0 ]; then
	echo "FAIL routes usr roots: expected 0 (every handler usr is a vertex), got $got" >&2
	fails=$((fails+1))
fi

# --dir를 붙이지 않는 검사 — check()는 항상 fixture dir을 뒤에 붙이므로
# 나쁜 --dir 검증은 마지막 인자가 이기는(last-wins) 구조상 여기서 따로 한다.
for c in "graph --dir /nonexistent-xyz" "schema --dir /nonexistent-xyz" "routes --role server --dir /nonexistent-xyz" "routes --role client --dir /nonexistent-xyz"; do
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
