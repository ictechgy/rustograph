#!/usr/bin/env bash
# 오라클 재기록: fixture에 `rustograph routes --role client`를 돌리고, 같은 소스를 진짜
# reqwest·ureq·url로 실행해 로컬 기록 서버가 받은 요청과 대조한 뒤 recorded/client.json을
# 쓴다. crates.io 의존성 내려받기 외에 네트워크를 쓰지 않는다(요청은 127.0.0.1 프록시로 간다).
# 불일치·귀속되지 않은 사실이 하나라도 있으면 0이 아닌 코드로 끝난다. 서버는 오라클
# 프로세스 안의 스레드라 프로세스와 함께 끝난다.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
tmp="$(mktemp -d)"
cargo build --quiet --manifest-path "$repo/Cargo.toml"
cargo build --quiet --release --manifest-path "$here/Cargo.toml"
fixture="$repo/tests/fixture-client"
"$repo/target/debug/rustograph" routes --role client --dir "$fixture/app" \
  --wrappers "$fixture/http-wrappers.json" --out "$tmp/client.json" >/dev/null
status=0
"$here/target/release/oracle-client" "$tmp/client.json" "$here/recorded/client.json" || status=1
rm -rf "$tmp"
exit $status
