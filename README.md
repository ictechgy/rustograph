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
cargo install --git https://github.com/ictechgy/rustograph --tag v0.3.0
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

# Emit bridge-facts for isthmus' persistence join (SQL relation uses)
rustograph schema --dir . --out schema-facts.json

# isthmus language-traversal v1 for `isthmus trace` (many roots, one pass)
rustograph reach mycrate::api::list_users mycrate::api::create_user
rustograph impact --format language-traversal --roots-from schema-facts.json
```

Exit codes: `0` ok · `1` strict violation/finding · `2` usage or analysis
error. The traversal commands (`reach`, `impact --format
language-traversal`) follow the isthmus family contract instead: `64` for a
usage error (stdout stays empty) or when some roots are not graph vertices
(the document is still written).

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

## Persistence facts — `schema`

`rustograph schema` emits an isthmus `bridge-facts` v1 document
(`platform: "rust"`, `target: "persistence"`) describing how the code
references SQL relations — isthmus joins it with `schemagraph facts`
output to report missing/unused schema objects and column drift.
Extracted references:

- SQL-looking string literals anywhere (also inside `format!`-style
  macros), scanned for `FROM`/`JOIN`/`INTO`/`UPDATE`/`TABLE`/`TRUNCATE`
  relations — `schema.table` qualifiers and quoted identifiers preserved
- `sqlx::query*` macros and functions (`query!`, `query_as!`,
  `query_scalar!`, …) — literal SQL scanned, non-literal arguments kept
  as `dynamic` facts so isthmus can count the gap
- `sqlx::query*_file!` — SQL lives in a file, reported as `dynamic`
- `diesel::table!` macro bodies — relation plus column uses
- `#[diesel(table_name = …)]`, `#[sea_orm(table_name = "…")]` structs and
  their field/`column_name`/`sqlx::rename` columns
- diesel DSL paths — `users::table`, `users::dsl::id`,
  `users::columns::name` — matched against the workspace's declared
  `table!` names; same-shaped paths that match nothing stay `dynamic`

Each fact carries `symbol: {qualifiedName, usr}` where `usr` is the id of
the enclosing graph vertex — the same id `impact`/`reach` use, so isthmus
`trace` can join handler reach to relation uses:

- `fn`, impl method (`crate::m::Type::method`, trait impls
  `crate::m::Type::<Trait>::method`), trait default method, and
  `const`/`static` initializer → that vertex
- struct-level facts (`table_name` attributes, field columns) → the struct
- impl-level facts outside any method (associated consts) → the impl's
  self type
- no enclosing vertex (top-level `table!` invocations, files outside the
  module tree) → no `symbol`; counted as `missing-relation-usrs:`

The ids come from the same syn harvest `impact` uses (not re-derived), and
every attached `usr` is checked against the graph's vertex set.

Unqualified names (`query!`, `sql_query`, `table!`) count only when the
file imports them from `sqlx`/`diesel`. Unparseable files, off-grammar
`table!` bodies,
and column attributes without a table binding surface as `limitations`,
not silence. The name-based scan never guesses: what cannot be resolved
statically is counted, not invented.

## Traversal documents — `reach` / `impact --format language-traversal`

```bash
rustograph reach  ID... [--roots-from FILE|-] [--max-depth N] [--max-reached N]
                  [--revision REV] [--generated-at TIMESTAMP] [-- ID...]
rustograph impact --format language-traversal ID... (same options)
```

Both write an isthmus
[`language-traversal` v1](https://github.com/ictechgy/isthmus/blob/main/docs/LANGUAGE-TRAVERSAL.md)
document: `reach` the symbols the roots depend on (`dependencies`),
`impact` the symbols that depend on them (`dependents`). Ids are the same
strings as `symbol.usr` in `schema`.

- **Roots**: positional ids, then `--roots-from` (a JSON array of strings or
  a bridge-facts document — its facts' `symbol.usr`; `-` reads stdin),
  deduplicated in first-seen order (that order is the meaning of
  `reached[].roots`). Empty or control-character ids and more than 10,000
  roots are usage errors (`64`, empty stdout).
- **One pass over all roots**: every reached symbol lists every root that
  reaches it (`roots`, first 64 plus `rootsTruncated`), the nearest depth,
  and a shortest-path witness (`via`). A root reached from another root is
  listed without its own index. Checked against a per-root BFS oracle on
  random graphs.
- **Evidence**: `tentative` edges (name fan-out, `dyn`/generic trait impl
  candidates) are `candidate`, all other edges `direct`; each symbol carries
  the per-root lower bound. `dispatch` and `unresolvedCalls` are not emitted
  — rustograph cannot claim its unresolved-call counts are complete.
- **Limits**: `--max-depth` 1–128 (default 128; `--depth 0` means 128),
  `--max-reached` 1–100,000; cuts set `truncated` with `depth` /
  `max-reached`.
- **Identity**: `project` is the same realpath `schema` writes; `revision`
  is `--revision` or git `HEAD` when the work tree is clean; `graphRevision`
  is the SHA-256 of the graph JSON (with `root` normalized to the project).
- **root-not-found**: ids that are not graph vertices are listed without
  `symbol`, a `root-not-found:` limitation is added, and the command exits
  `64` after writing the document.

isthmus rejects `route-decl` facts from `platform: "rust"` documents, so
Rust handlers cannot yet start a route-selection `trace`; relation and
symbol selections (reverse traversal) work today.

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
