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
cargo install --git https://github.com/ictechgy/rustograph --tag v0.4.1
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

# Emit isthmus http route-decl facts for axum / actix-web servers
rustograph routes --role server --dir . --out routes.json
rustograph reach --roots-from routes.json   # handler usrs are graph vertices

# Emit isthmus http route-call facts for reqwest / ureq clients
rustograph routes --role client --dir . --wrappers http-wrappers.json --out calls.json
rustograph impact --format language-traversal --roots-from calls.json

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

## Server routes — `routes --role server`

`rustograph routes --role server` emits an isthmus `bridge-facts` v1
document with `platform: "rust"`, `target: "http"` and one `route-decl`
fact per (method, canonical path template) that an axum 0.7/0.8 or
actix-web 4 server declares. `isthmus trace` joins it with client calls,
`reach` (the handler `symbol.usr` is the same vertex id), `schema` and
`schemagraph facts` to answer "which tables does this endpoint touch".

- **axum** (`dispatch: "specificity"`, matchit's static > param >
  catch-all order): `Router::new().route(..)` chains, method routers
  (`get`/`post`/…/`any`/`on(MethodFilter)`), `nest` (joined like axum's
  `path_for_nested_route`), `merge`, local `let`/reassignment and crate
  functions that return routers. The path syntax follows the resolved axum
  version — `:id`/`*rest` for 0.7, `{id}`/`{*rest}`/`{{` for 0.8.
- **actix-web** (`dispatch: "registration-order"`, one `order.group` per
  `App`, one `order.index` per resource): `#[get("/x/{id}")]`-style macros,
  `web::resource().route(web::get().to(h))`, `web::scope`, `App::route`,
  `configure`, guards (`narrowed`), `{id:\d+}` → `paramConstraints`,
  `{tail}*` catch-alls, and the `NormalizePath` middleware's effect on
  `trailingSlash`.
- Whatever cannot be resolved statically (non-literal paths, routers built
  by unknown functions, fallbacks, tower services) becomes a dynamic fact or
  a scoped `route-coverage:` / `framework-provided-routes:` limitation —
  never a guessed route.

Every rule, with the axum/matchit/actix-web source lines that back it, is in
[docs/HTTP-ROUTES.md](docs/HTTP-ROUTES.md). An oracle
(`experiments/routes-oracle/`) compiles the same fixture sources against
the real crates and probes them in-process: 100% precision and recall on
all three fixtures, recorded and checked offline by `cargo test`. The
isthmus conformance vectors are vendored under `conformance/` with a lock.

## Client calls — `routes --role client`

`rustograph routes --role client` emits an isthmus `bridge-facts` v1
document with `platform: "rust"`, `target: "http"`, `roles: ["client"]` and
one `route-call` fact per HTTP request the code builds. `symbol.usr` is the
enclosing function or method — the same vertex id `impact` uses, so
`isthmus trace` can continue from a call site into the client code that
depends on it.

- **reqwest** (0.13; 0.12 checked too): `reqwest::get`, `blocking::get`,
  `Client`/`blocking::Client` verb methods, `request(Method::X, url)`,
  `Request::new`. Strings go through `url::Url::parse` (WHATWG): dot
  segments are removed, `//` is kept.
- **ureq** 3 (2.x read from source): free functions and `Agent` methods.
  ureq 3 parses with `http::Uri`, which keeps dot segments.
- **URL building**: literals, `format!` (positional, named and inline
  arguments), `concat!`, `+`, consts/statics/associated consts, locals
  (shadowing-aware, mutated names untrusted), `Url::parse(..)?.join(..)`
  (RFC 3986 merge — `…/v2/catalog` + `tags` is `/v2/tags`), and a base URL
  held in a struct field when every constructor fills it with the same
  literal or const. Anything else stays a dynamic fact (`channel: null`, a
  masked `channelPrefix` when proven) or a counted limitation.
- **Wrappers**: functions, methods and struct-literal endpoints declared in
  an isthmus `http-wrappers` v1 file (`"language": "rust"`, `owner::name` is
  the rustograph vertex id) become calls with the declared verb and anchor.
- **Measured gaps**: unmodelled clients (hyper client, surf, awc, isahc, …),
  requests sent from a receiver that is not a proven client, relative URLs
  the client rejects, undeclared wrapper sinks and unresolved declarations
  are `route-call-coverage:` / `ambiguous-base-join:` /
  `http-wrapper-undeclared:` / `http-wrapper-unresolved:` limitations.

The rules (`Url::join` is the isthmus `rfc3986` join; reqwest and ureq have
no base URL, so full-URL rules apply) and the oracle table are in
[docs/HTTP-CLIENT.md](docs/HTTP-CLIENT.md). A mock-server oracle
(`experiments/client-oracle/`) compiles the fixture against the real
reqwest/ureq/url crates and records every request at a local server: 41
scenarios, 0 mismatches, checked offline by `cargo test`. All 48
`producer`/`producer:rustograph` cases of the isthmus `url-compose` vectors
pass.

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

isthmus accepts Rust `route-decl` and (since isthmus #133) `route-call`
documents, so a workspace `trace` joins a reqwest client to an axum/actix-web
handler and continues into the client code with `impact`.

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
