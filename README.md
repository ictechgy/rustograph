# rustograph

<img src="icon.png" alt="rustograph's bird mascot" width="112" height="112" align="right">

Rust dependency graph tool — read a Cargo workspace, build its dependency
graph, and run queries on top: cycles, reachability, impact, layer rules.

The design in one sentence: **the graph is the artifact; everything else is
a query over it.**

[한국어](README.ko.md)

Sibling projects: [cartograph](https://github.com/ictechgy/cartograph) (Swift) ·
[gartograph](https://github.com/ictechgy/gartograph) (Go) ·
kartograph (Kotlin/Android) · dartograph (Dart/Flutter) ·
[schemagraph](https://github.com/ictechgy/schemagraph) (databases) ·
isthmus (cross-language joins).

## Why

Rust already has `cargo-modules`, `cargo deps`, and a wave of syn/tree-sitter
call-graph tools — but they either stop at module level or guess call edges by
name without saying so. rustograph instead builds **one versioned graph** at
four levels (crate/module/type/symbol) and expresses every analysis as a query
over it, with a deterministic JSON contract meant for coding agents:

- **Tentative edges are labeled, not hidden.** A method call `x.m()` has no
  type information in syntax — rustograph emits speculative fan-out edges
  marked `tentative: true`, uses them for reachability (erring toward
  "alive"), and excludes them from cycle and rule evidence with a counted
  report.
- **Limitations are measured, not boilerplate.** Unresolved paths, macro
  bodies, `#[cfg]` items, and orphan files are counted per project and travel
  with every answer.
- **No delete verdicts.** `dead` reports `unreachable` — a graph fact — never
  "safe to delete".

## Install

```bash
brew install ictechgy/tap/rustograph
# or
cargo install --git https://github.com/ictechgy/rustograph --tag v0.2.1
```

Or from source:

```bash
git clone https://github.com/ictechgy/rustograph.git
cd rustograph
cargo build --release
# binary at target/release/rustograph
```

For type-resolved analysis, build with the optional `semantic` feature
(rust-analyzer `ra_ap_*` crates — heavy, opt-in):

```bash
cargo build --release --features semantic
rustograph graph --level symbol --semantic
```

## Usage

```bash
# Emit the dependency graph (deterministic JSON)
rustograph graph                          # module level
rustograph graph --level crate            # packages + dependencies
rustograph graph --level crate --deps     # include external crates
rustograph graph --level symbol           # call/references/implements/signature
rustograph graph --level symbol --semantic  # + type-resolved calls (feature build)
rustograph graph --level type --format mermaid
rustograph graph --out .rustograph/graph.json   # persist, then reuse:

rustograph cycles --graph .rustograph/graph.json --strict

# Detect dependency cycles (tentative edges excluded, count reported)
rustograph cycles --level symbol --strict

# Report symbols unreachable from retention roots (fn main, #[no_mangle])
rustograph dead                           # symbol level, always
rustograph dead --retain-public           # libraries: keep exported API
rustograph dead --tests                   # also retain #[test]/#[bench]
rustograph dead --root mycrate::setup     # extra retention root
rustograph dead --explain mycrate::f      # why alive? show a reachability path

# Check layer rules from .rustograph.yml
rustograph rules --strict
rustograph rules --format sarif           # GitHub code scanning ready
rustograph rules --write-baseline         # freeze current violations
rustograph rules --baseline base.txt      # only *new* violations fail

# Ask about one symbol (agent-oriented JSON)
rustograph query mycrate::module::f --depth 2
rustograph impact mycrate::Type --depth 3  # reverse transitive closure
rustograph paths mycrate::a mycrate::b    # bounded paths between two ids
rustograph search entry                   # exact > suffix > substring
#   partial ids are refused with candidates — retry with an exact id

# Crate-level dependency health (declared vs actually referenced)
rustograph deps                           # unused deps + duplicate versions
rustograph deps --strict                  # exit 1 when findings exist

# Trim the document before analysis
rustograph graph --focus mycrate::sub     # keep one subtree only
rustograph dead --exclude-tests           # drop #[cfg(test)] subtrees
rustograph graph --target x86_64-pc-windows-msvc  # evaluate cfg(triple)
rustograph dead --semantic --no-cache     # bypass the semantic cache

# Serve the graph over MCP (stdio JSON-RPC) for coding agents
rustograph mcp                            # harvests once, serves a snapshot
rustograph mcp --graph .rustograph/graph.json
```

Exit codes: `0` ok · `1` strict violation/finding · `2` usage or analysis
error.

## Graph model

Vertices: `crate`, `module`, `struct`, `enum`, `trait`, `union`, `typealias`,
`fn`, `method`, `const`, `static`, `macro`. Edges: `depends`, `uses`,
`contains`, `call`, `references`, `implements`, `signature`.

- `contains` is ownership, not a dependency — it never feeds cycles or rules.
- Levels are projections: the symbol graph is the source of truth; shallower
  levels fold edges onto owning modules/crates.
- `signature` edges record types leaked through a function's public
  signature — they power the `signature` rule and reachability, and let a
  rules file say "public API may not mention component X".
- `#[cfg]` conditions travel as metadata: a vertex's `cfg` holds the tokens
  of its own `#[cfg(...)]` (e.g. `feature = "x"`), and an edge's `cfg` marks
  dependencies that only exist under that condition — `use` statements,
  `contains` of gated items, bodies of gated functions.
- `unsafe` marks the boundary: vertices that are `unsafe fn`/`unsafe trait`
  or contain an `unsafe {}` block carry `unsafe: true`, and an edge made
  inside an `unsafe {}` block is an entry edge — `unsafe impl` marks its
  `implements` edge too.

## MCP server

`rustograph mcp` speaks newline-delimited JSON-RPC 2.0 on stdio and serves
nine tools — `rustograph_summary`, `rustograph_query`, `rustograph_impact`,
`rustograph_paths`, `rustograph_search`, `rustograph_cycles`,
`rustograph_dead`, `rustograph_rules`, `rustograph_deps`. The document is
harvested once at startup (or loaded via `--graph`), so every call answers
over the same snapshot. Partial ids are refused with a candidate list —
call `rustograph_search` or retry with an exact id.

Semantic-mode documents are cached under `.rustograph/semantic-cache.json`,
keyed by a fingerprint of workspace sources and manifests — a stale or
corrupt cache silently falls back to a fresh harvest.

## Rules — .rustograph.yml

```yaml
components:
  core: ["mycrate::core/**"]
  ui:   ["mycrate::ui/**"]
deps:
  ui: [core]
  core: []            # allowlist — anything not listed is a violation
deny:
  core: [ui]          # deny beats allow
signature:
  ui: [ui, core]      # exported API signatures may only mention these
baseline: .rustograph/rules-baseline.txt  # frozen violations (optional)
```

Unmapped modules are reported separately — a rule's blind spot is not a pass.

A baseline file freezes violations that existed when the rules were adopted:
`rules --write-baseline` records the current set (one `rule|from|to|kind`
key per line, `#` comments allowed), and later runs suppress matching
violations while still reporting `baselined`/`stale_baseline` counts —
stale entries mean the code improved and the file can be regenerated.

## Agent output contract

`query`/`impact` JSON reports `level`, `depth`, `truncated`, all edge kinds
between neighbors, and per-project `limitations`. Optional fields that have
no value are omitted — an absent `tentative` means confirmed, an absent
`truncated` means complete.

## Development

```bash
cargo build
cargo test                      # unit + fixture integration tests
scripts/coverage.sh             # tests + coverage gate (90%)
scripts/verify-cli-contract.sh  # exit-code contract against the real binary

# dogfooding — analyze this repository with itself
cargo run -- rules --strict
cargo run -- cycles --level symbol --strict
cargo run -- dead --retain-public
```

Coverage needs `cargo-llvm-cov`; the script auto-detects `llvm-tools` from
rustup or the active sysroot.

## Limitations of the MVP

Syntactic analysis (syn) cannot see through macros, `dyn` dispatch, or
generics — every such gap is counted in `limitations`. The optional
`semantic` feature (`cargo build --features semantic`, then `--semantic`)
augments body harvesting with rust-analyzer (`ra_ap_*`) semantics: method
calls resolve by receiver type instead of name fan-out, macro expansions are
walked, and `dyn`/generic trait calls expand to workspace impl candidates
(still tentative — the real impl is a runtime fact). The graph contract —
vertices, edge kinds, `tentative`, `limitations` — is unchanged; only
accuracy improves. Bodies the semantic engine cannot see (cfg-disabled,
macro-generated) fall back to the syntactic path with measured counters.

## License

MIT — see [LICENSE](LICENSE).
