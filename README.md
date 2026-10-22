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
  marked `tentative: true` to the same-name methods the call could reach,
  uses them for reachability (erring toward "alive", up to a width cap — see
  [Graph model](#graph-model)), and excludes them from cycle and rule
  evidence with a counted report.
- **Limitations are measured, not boilerplate.** Unresolved paths, macro
  bodies, `#[cfg]` items, and orphan files are counted per project and travel
  with every answer.
- **No delete verdicts.** `dead` reports `unreachable` — a graph fact — never
  "safe to delete".

## Install

```bash
brew install ictechgy/tap/rustograph
# or
cargo install --locked --git https://github.com/ictechgy/rustograph --tag v0.5.0
```

The source-install command is pinned to the published
[v0.5.0 release](https://github.com/ictechgy/rustograph/releases/tag/v0.5.0)
and its committed dependency lockfile. Homebrew availability follows the tap
formula separately; this GitHub release does not establish crates.io publication.

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

`--semantic` runs the analyzed workspace's build scripts and procedural
macros — see [Security: trust boundary](#security-trust-boundary) before
using it on code you do not trust.

## Usage

```bash
# Emit the dependency graph (deterministic JSON)
rustograph graph                          # module level
rustograph graph --level crate            # packages + dependencies
rustograph graph --level crate --deps     # include external crates
rustograph graph --level symbol           # call/references/implements/signature
rustograph graph --level symbol --semantic  # + type-resolved calls (feature build)
rustograph graph --level type --format mermaid
rustograph graph --out .rustograph/graph.json   # persist (records git HEAD if the tree is clean), then reuse:

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
rustograph impact mycrate::Type --depth 0  # no depth limit (still bounded by --max)
# Analyze an actual patch using independent before/after source snapshots
rustograph change-impact --base main --head HEAD --depth 2
rustograph change-impact --base HEAD --working-tree
rustograph paths mycrate::a mycrate::b    # bounded paths between two ids
rustograph paths mycrate::a mycrate::b --max 50 --budget 200000  # at most 1024 / 1000000
rustograph search entry                   # exact > suffix > substring
#   partial ids are refused with candidates — retry with an exact id

# Crate-level dependency health (declared vs actually referenced)
rustograph deps                           # unused deps + duplicate versions
rustograph deps --strict                  # exit 1 when findings exist

# Trim the document before analysis
rustograph graph --focus mycrate::sub     # keep one subtree only
#   the retention roots and boundary-crossing edges it drops are counted in limitations
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

A flag or argument that a command does not use is a usage error (`2`) instead
of being silently ignored — for example `cycles --out`, `query --format` or
`paths --role`. Every command that reads the graph (`graph`, `cycles`, `dead`,
`rules`, `query`, `impact`, `paths`, `search`, `mcp`) takes `--dir` or
`--graph` and the shared flags `--deps`, `--tests`, `--retain-public`,
`--semantic`, `--no-cache`, `--focus`, `--target` and `--exclude-tests`;
`rustograph help` lists each command's own flags.

Numeric limits are part of the contract: `query`, `impact` and `search` take
`--max` up to 10,000, `query` and `impact` take `--depth` up to 64 (`0` means
no depth limit), and `paths` takes `--max` up to 1,024 and `--budget` up to
1,000,000. A larger value is a usage error (`2`) instead of being clipped; the
MCP tools apply the same limits.

## Graph model

Vertices: `crate`, `module`, `struct`, `enum`, `trait`, `union`, `typealias`,
`fn`, `method`, `const`, `static`, `macro`. Edges: `depends`, `uses`,
`contains`, `call`, `references`, `implements`, `signature`.

- `contains` is ownership, not a dependency — it never feeds cycles or rules.
- Name fan-out: a method call `x.m()` whose receiver type is unknown gets
  `tentative` call edges to the same-name workspace methods it could reach.
  Inherent (`T::m`) and trait-declared (`Tr::m`) methods come only
  from the calling crate, its package's library, and the crates it depends
  on — transitively and of every kind, including dev, build, and optional
  dependencies that the default features leave off — because a crate cannot
  name another crate's types or traits without depending on it. Trait impl
  methods (`T::<Tr>::m`) come from the whole workspace: `dyn` and generic
  dispatch reach impls in crates that depend on the caller. Calls whose only
  same-name methods are out of reach get no edges and are counted in
  `limitations`. A call left with more than 4,096 candidates gets no edges
  either and is counted: methods reached only through such calls are
  reported unreachable by `dead` — whose JSON report and MCP
  `rustograph_dead` answer carry the same limitation — and their callers are
  missing from `query`, `impact`, and `reach`. The cap sits far above real workspaces (the
  widest call across 27 rust-analyzer crates has 544 candidates); it bounds
  inputs where same-name definitions grow with the workspace. Without a
  resolve graph from `cargo metadata`, fan-out is not narrowed.
- Levels are projections: the symbol graph is the source of truth; shallower
  levels fold edges onto owning modules/crates.
- `signature` edges record types leaked through a function's public
  signature — they power the `signature` rule and reachability, and let a
  rules file say "public API may not mention component X".
- `#[cfg]` conditions travel as metadata: a vertex's `cfg` holds the tokens
  of its own `#[cfg(...)]` (e.g. `feature = "x"`), and an edge's `cfg` marks
  dependencies that only exist under that condition — `use` statements,
  `contains` of gated items, bodies of gated functions.
- `generated: true` marks code a tool wrote: one of the first 10 lines of the
  file contains `@generated`, `auto-generated`, or `autogenerated`, or
  contains both `generated` and `do not edit`, in any letter case. A
  `DO NOT EDIT` line without `generated` does not count, and the two words on
  different lines do not either. Every vertex from one file — its module or
  crate, items, impl methods, and inline modules — carries the same value.
  Like `cfg`, it is a fact, not a filter.
- `unsafe` marks the boundary: vertices that are `unsafe fn`/`unsafe trait`
  or contain an `unsafe {}` block carry `unsafe: true`, and an edge made
  inside an `unsafe {}` block is an entry edge — `unsafe impl` marks its
  `implements` edge too.
- `revision` (optional) is the source revision a saved graph was harvested
  from: `graph --out` writes git `HEAD` when the `--dir` work tree is clean
  (the same check, with the same git isolation, as the traversal `revision`).
  A harvest written to standard output never carries it, so a snapshot
  redirected from a harvest's stdout has none. Projections and filters keep
  it, and `graph --graph` copies the input's value as is into its JSON, on
  standard output and with `--out` alike. Keep snapshots outside the work
  tree or in a git-ignored path such as `.rustograph/`: an untracked snapshot
  makes the next tree look modified.

## MCP server

`rustograph mcp` speaks newline-delimited JSON-RPC 2.0 on stdio and serves
nine tools — `rustograph_summary`, `rustograph_query`, `rustograph_impact`,
`rustograph_paths`, `rustograph_search`, `rustograph_cycles`,
`rustograph_dead`, `rustograph_rules`, `rustograph_deps`. The document is
harvested once at startup (or loaded via `--graph`), so every call answers
over the same snapshot. `rustograph_deps` is the exception: it harvests the
`--dir` workspace itself on its first call, because unused-dependency evidence
needs the complete workspace graph. With `--graph`, it answers only when that
workspace is the one the snapshot was harvested from (the snapshot's
`workspace`, compared as a directory, so a symbolic-link spelling matches).
Otherwise, or when the snapshot records no absolute `workspace`, it returns an
error instead of describing another repository; start the server with `--dir`
set to the snapshot's workspace, or without `--graph`. It never runs `cargo`
for a path taken from the snapshot. Partial ids are refused with a candidate
list — call `rustograph_search` or retry with an exact id.

Each request is one line of at most 1 MiB (1,048,576 bytes, not counting the
line ending). A longer line is never buffered: the server answers it with a
JSON-RPC `-32600` error whose `id` is `null`, discards the rest of that line,
and keeps reading the next request. Tool arguments have the CLI limits —
`max` up to 10,000 (`rustograph_paths`: 1,024), `depth` up to 64 and `budget`
up to 1,000,000 — and `tools/list` publishes them as JSON Schema `maximum`. A
larger value returns an `isError` result instead of being clipped. Responses
are never cut short: a tool answers with the complete report for the
arguments it accepted.

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

A rules file that would silently switch rules off is a configuration error:
an unknown key (such as `component:` for `components:`), a `deps`, `deny` or
`signature` key that names a component `components` does not define, and a
file that defines no component at all. `rules` exits 2 and the MCP
`rustograph_rules` tool answers with an error, naming the cause. Only the keys
are checked; the lists may still name components or module patterns.

A baseline file freezes violations that existed when the rules were adopted:
`rules --write-baseline` records the current set (one `rule|from|to|kind`
key per line, `#` comments allowed), and later runs suppress matching
violations while still reporting `baselined`/`stale_baseline` counts —
stale entries mean the code improved and the file can be regenerated.

`baseline:` is relative to the directory of the rules file and has to stay
inside the `--dir` directory, because the rules file comes from the analyzed
repository and `--write-baseline` creates the file and its parent directories.
An absolute path, a path whose `..` climbs out of the `--dir` directory, and one
that leaves it through a symbolic link are configuration errors, which `rules`
reports before it harvests, reads or writes anything. `..` that stays inside is fine,
so `--config sub/.rustograph.yml` can name `../base.txt`. The same holds for a
`--config` file outside `--dir` and for the MCP server's `--dir`. An explicit
`--baseline FILE` is your own choice and may point anywhere.

`rules --format sarif` writes each location as a URI relative to the Cargo
workspace root — `uriBaseId: "%SRCROOT%"`, `/` separators, RFC 3986
percent-encoding — so the log carries no local absolute path; the root's own
path is not recorded either. A violation in a file outside the workspace root,
such as a `#[path]` module that leaves it, has no location, and its message
says so. With `--graph`, locations are relative to the workspace root the
snapshot recorded.

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

SQL text is read by a token scanner, not a parser. Following the SQL-text
rules the isthmus persistence producers share, three positions are not
relation uses: `FROM` directly inside function arguments
(`EXTRACT(YEAR FROM created_at)`, `TRIM(BOTH ' ' FROM name)`,
`SUBSTRING(name FROM 2)`; a subquery inside them still counts), an
unqualified name that is a CTE of the same `;`-separated statement
(unquoted names compare case-insensitively, quoted names as written; the
rule is statement-wide, so `WITH users AS (SELECT * FROM users …)` also
drops the real `users` its body reads), and the operand of
`IS [NOT] DISTINCT FROM`.

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

## Git change review — `change-impact`

Use [the change-impact workflow](docs/CHANGE-IMPACT.md) to map a Git base/head
or working-tree patch to old/new file declarations, historical/current callers
and connected test/entry roots. Results preserve source content freshness,
harvested limitations, witness hops and depth/output restrictions. File-wide
static candidates support review; empty results are not a safety guarantee or
permission to skip tests. MCP exposes the same workflow as
`rustograph_change_impact`.

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
  is `--revision` or git `HEAD` when the harvested work tree is clean
  (omitted, with a `revision-unverified:` limitation, when a clean tree cannot
  be checked without running the repository's git configuration — a work tree
  with submodules, see [Security: trust boundary](#security-trust-boundary)).
  With `--graph`, rustograph does not look at git, whose `HEAD` may have moved
  since the snapshot was saved: `revision` is `--revision` or the one the
  snapshot records (see [Graph model](#graph-model)), and a snapshot without a
  usable one gets no `revision` and a `revision-unknown:` limitation.
  `graphRevision` is the SHA-256 of the graph JSON (with `root` normalized to
  the project and the snapshot's `revision` left out, so a snapshot and a fresh
  harvest of the same commit match).
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

`impact` follows incoming dependency edges only. Sharing a dependency with
another symbol does not make that symbol impacted. `query` walks both directions;
each neighbor/direction is reported once at its shortest discovered depth, with
all dependency edge kinds for that direction and depth. The starting symbol is
not repeated as its own neighbor. `--max` bounds the number of result records
(at most 10,000), including zero; `truncated` becomes true only when a record is
actually omitted. `--depth` is at most 64. `--depth 0` (MCP `depth: 0`) means no
depth limit: the report covers the whole closure, still bounded by `--max`, and
keeps `depth: 0`. The default depth is 1, except for MCP `rustograph_impact`,
whose default is 0.

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
generics — every such gap is counted in `limitations`. Name fan-out is
bounded by the calling crate's dependencies and by a width cap of 4,096
candidates per call; wider calls are counted instead of fanned out, so `dead`
can report methods reached only through them (see
[Graph model](#graph-model)). The optional
`semantic` feature (`cargo build --features semantic`, then `--semantic`)
augments body harvesting with rust-analyzer (`ra_ap_*`) semantics: method
calls resolve by receiver type instead of name fan-out, macro expansions are
walked, and `dyn`/generic trait calls expand to workspace impl candidates
(still tentative — the real impl is a runtime fact). The graph contract —
vertices, edge kinds, `tentative`, `limitations` — is unchanged; only
accuracy improves. Bodies the semantic engine cannot see (cfg-disabled,
macro-generated) fall back to the syntactic path with measured counters, and
calls it cannot resolve fall back to the same bounded name fan-out.

The syntactic harvest does not read a module file — a `mod` file or a crate
root — larger than 64 MiB. Its module or crate vertex stays, but the items and
submodules it declares are missing, and `limitations` counts such files, so
`dead` can report code used only from them as unreachable. `schema` counts
them under `unparsed-sources`; `routes` and `deps` do not see them. While such
a file is in the workspace, `--out` does not replace an existing output — write
to a new path or to stdout. The limit sits far above real sources: the largest
of 18,080 `.rs` files in 610 crates.io packages is 2.5 MiB.

## Security: trust boundary

rustograph parses source with syn, but it also runs the Rust toolchain.
Harvesting a workspace — every analysis that does not load a saved graph with
`--graph` — runs `cargo metadata`. Even with `--graph`, `--target` runs
`rustc --print cfg`, and the MCP `rustograph_deps` tool runs `cargo metadata`
for the `--dir` workspace on its first call; the `--semantic` cache key runs
`rustc -vV`. These commands run in the current directory, where Cargo reads
`.cargo/config.toml` (from that directory and its parents) and rustup proxies
look for a `rust-toolchain.toml` (or `rust-toolchain`) file the same way.
rustograph keeps a checkout from choosing the programs they run, even when you
run it inside that checkout — but not from running its own code inside them
through `[env]` (below), so run it inside a checkout only if you trust it:

- `cargo`, `rustc`, and `rustup` come from absolute `PATH` entries only
  (`/bin:/usr/bin` when `PATH` is unset) and run by that absolute path. Empty
  and relative entries such as `.` or `bin` are skipped, because they resolve
  against the current directory.
- Every toolchain command runs with `RUSTC_WRAPPER` and
  `RUSTC_WORKSPACE_WRAPPER` set to empty, which turns off
  `build.rustc-wrapper` and `build.rustc-workspace-wrapper`, and with `RUSTC`
  set, which takes precedence over `build.rustc`; without a `rustc` to set,
  `cargo` does not run.
- The toolchain is fixed once per run, outside the checkout. `RUSTC` is the
  `rustc` found on `PATH`, or, when that is a rustup proxy, the path
  `rustup which rustc` reports. When the `cargo` or `rustc` on `PATH` is a
  rustup proxy, `RUSTUP_TOOLCHAIN` is also set, to what
  `rustup show active-toolchain` reports. rustograph asks rustup in a new,
  empty directory under your temporary directory, so no `rust-toolchain` file
  is read. A `RUSTUP_TOOLCHAIN` or `RUSTC` that you set yourself is used as
  given: set `RUSTUP_TOOLCHAIN=<channel>` to analyze with a project's own
  toolchain.
- When this leaves a `rust-toolchain` file in the current directory or its
  parents unused, the document gets a `toolchain-file-ignored:` limitation.
  When `cargo metadata` fails instead — for example on a manifest that only
  the file's toolchain can read, such as nightly-only `cargo-features` — the
  error ends by saying that the file was not used and which toolchain ran; set
  `RUSTUP_TOOLCHAIN=<channel>` to analyze with the file's toolchain. If the
  temporary directory itself sits below such a file, or rustup cannot report a
  toolchain, the analysis fails with an error that asks you to set
  `RUSTUP_TOOLCHAIN` and `RUSTC`.

The current directory does not change, so the rest of the checkout's Cargo
configuration — source replacement, `[patch]`, registries — still applies, as
it does for `cargo build`. That includes two settings rustograph leaves alone:

- The checkout's `[env]` table still sets environment variables for the
  `rustc` that `cargo metadata` runs to learn the host and target, and with
  `force = true` it overrides your own values. That is enough to run the
  checkout's code: `LD_PRELOAD` on Linux or `DYLD_INSERT_LIBRARIES` on macOS
  can load a library from the checkout into that `rustc`. rustograph does not
  override `[env]`, so it applies whenever the current directory is the
  checkout or one of its subdirectories. Running rustograph inside a checkout
  is therefore not safe for code you do not trust; analyze such a checkout
  from outside, as described below.
- Credential providers (`registry.global-credential-providers`,
  `registries.<name>.credential-provider`), which Cargo runs to authenticate
  to registries that need it, are not overridden, because that would also
  turn off your own provider for private registries.

`--semantic` executes the analyzed code. rust-analyzer runs `cargo check` in
the analyzed workspace root, which builds and runs its build scripts
(`build.rs`) under that workspace's Cargo configuration, and then runs its
procedural macros to expand them — wherever you run rustograph from. The
protections above do not apply to the `cargo` and `rustc` that rust-analyzer
starts. Use `--semantic` only on code you would build yourself.

For untrusted repositories and pull requests from forks, run rustograph on an
isolated runner or sandbox that holds no secrets, without `--semantic`, from a
working directory outside the checkout (neither the checkout nor one of its
subdirectories, where its `[env]` table applies), and pass the checkout with
`--dir`. The checkout's `.cargo/config.toml` — `[env]` included — and
`rust-toolchain.toml` then do not apply to the toolchain commands rustograph
runs at all; configuration in the working directory, its parents, and your
Cargo home still does. `cargo metadata` resolves the checkout's dependencies
wherever it runs. It can write `Cargo.lock` into the checkout, and the
analysis fails if that write fails; it can also fetch registry and git
dependencies named in the checkout's manifests, so limit the sandbox's network
access if that matters. rustograph does not sandbox these commands itself.

`reach` and `impact --format language-traversal` without `--revision` or
`--graph`, and `graph --out` when it writes a harvested graph as JSON, also run
`git` in the `--dir` directory, only to read the source revision
(`git config`, `git ls-files`, `git status`, `git rev-parse`). These calls keep
the analyzed checkout from choosing the programs that run, whether through
`PATH` or through its git configuration:

- rustograph runs the first `git` found in an absolute `PATH` entry
  (`/bin:/usr/bin` when `PATH` is unset), by that absolute path. Empty and
  relative entries such as `.` or `bin` are skipped: because git starts in
  the `--dir` directory, such an entry can resolve inside the analyzed
  checkout and run a `git` that the checkout contains. Without `git` in an
  absolute entry, `revision` is omitted, as when git is not installed.
- Right before each call, rustograph lists the filter drivers git sees
  (`git config -z --includes --name-only --list`, which covers the repository,
  your own and the system configuration) and overrides each with empty
  `clean`, `smudge`, and `process` commands and `required=false`, so git runs
  none of them, whether `.gitattributes` or `.git/info/attributes` names the
  driver.
- `core.fsmonitor`, the untracked cache, `log.showSignature`, and optional
  locks (`GIT_OPTIONAL_LOCKS=0`) are off, so no fsmonitor hook runs and the
  index is not rewritten (rewriting it would run the `post-index-change` hook).
- git never runs inside a submodule: when the index records one, rustograph
  does not run `git status` at all (`--ignore-submodules=all` otherwise).
- When a driver name contains `=`, a control character, or invalid UTF-8,
  rustograph runs no other git command — git's `-c` splits at the first `=`,
  so such a driver cannot be overridden.

What this changes for `revision` (and for the one `graph --out` records): a
file that a filter normalizes (git-lfs and the like) is compared as raw
content, so when git's stat cache is stale for it the tree looks modified and
`revision` is omitted rather than claiming a clean `HEAD` that rustograph could
not verify; in a checkout you trust, run `git status` first to refresh git's
stat cache. A work tree with submodules, or a configuration with a driver name
that cannot be overridden, gets no automatic `revision` and a
`revision-unverified:` limitation (`graph --out` records none for it). Pass
`--revision` to record the analyzed revision yourself. The rest of your git
configuration, and absolute `PATH` entries as you set them (even one that
points into the checkout), apply as usual.

## License

MIT — see [LICENSE](LICENSE).


## 0.5.0 compatibility notes

Version 0.5.0 rejects analyses that require detail absent from a saved graph.
Regenerate snapshots with `rustograph graph --level symbol` for dead-code,
reachability, and rule analyses. Coarser projections remain available for graph
exports and cycle analysis.

The Rust library's `Document::view` and `analysis::dead` now return `Result`.
See [the 0.5 migration notes](docs/MIGRATING-0.5.md) for source changes and output
safety behavior. Version 0.5.0 was published on 2026-10-13 (UTC).
