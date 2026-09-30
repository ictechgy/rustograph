#!/usr/bin/env bash
# 오라클 재기록: fixture마다 `rustograph routes`를 돌리고 진짜 프레임워크로 요청해
# recorded/<fixture>.json을 쓴다. crates.io 의존성 내려받기 외에 네트워크를 쓰지 않는다.
# 정밀도·재현율·음성·끝 슬래시 요청이 하나라도 실패하면 0이 아닌 코드로 끝난다.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
tmp="$(mktemp -d)"
cargo build --quiet --manifest-path "$repo/Cargo.toml"
cargo build --quiet --release --manifest-path "$here/Cargo.toml"
status=0
for fx in axum08 axum07 actix; do
  "$repo/target/debug/rustograph" routes --role server --dir "$repo/tests/fixture-routes/$fx" --out "$tmp/$fx.json" >/dev/null
  "$here/target/release/oracle-$fx" "$tmp/$fx.json" "$here/recorded/$fx.json" || status=1
done
rm -rf "$tmp"
exit $status
