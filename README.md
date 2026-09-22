# rustograph

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

From source:

```bash
git clone https://github.com/ictechgy/rustograph.git
cd rustograph
cargo build --release
# binary at target/release/rustograph
```

## Usage

```bash
# Emit the dependency graph (deterministic JSON)
rustograph graph                          # module level
rustograph graph --level crate            # packages + dependencies
rustograph graph --level crate --deps     # include external crates
rustograph graph --level symbol           # call/references/implements/signature
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

# Ask about one symbol (agent-oriented JSON)
rustograph query mycrate::module::f --depth 2
rustograph impact mycrate::Type --depth 3  # reverse transitive closure
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
```

Unmapped modules are reported separately — a rule's blind spot is not a pass.

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
generics — every such gap is counted in `limitations`. The roadmap replaces
or augments this with rust-analyzer (`ra_ap_*`) semantics; the graph contract
will not change.

## License

MIT — see [LICENSE](LICENSE).
