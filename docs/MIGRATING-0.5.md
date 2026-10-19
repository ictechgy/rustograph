# Migrating to rustograph 0.5

Version 0.5.0 was published on 2026-10-13 (UTC). See the
[release notes and verified binary assets](https://github.com/ictechgy/rustograph/releases/tag/v0.5.0).

## Rust library callers

Two public APIs now fail explicitly rather than claim unavailable graph detail:

```rust
let modules = document.view(Level::Module)?;
let report = rustograph::analysis::dead(&document)?;
```

`Document::view` returns `Result<Document, String>`. A projection to a finer level
than the stored document returns a regeneration error. Equal and coarser views
retain their previous meaning. `analysis::dead` returns `Result<DeadReport,
String>` and requires a symbol-level document. Callers can propagate these
errors with `?` or handle them explicitly.

This source compatibility change is the reason for the 0.5 minor version rather
than a 0.4 patch. The serialized graph document remains version 1.

Three library changes after 0.5.0 are source-incompatible for some callers:

- `rustograph::modtree::ModTree` has a new public field, `skipped_mods`.
  `collect_submodules` no longer loads a `mod` declaration whose file re-enters
  an ancestor module's file (circular modules), is not a regular file, or nests
  deeper than `MAX_MODULE_DEPTH`; it records the module path with a
  `SkippedMod` reason there instead. Code that builds a `ModTree` with a struct
  literal or destructures every field must add the field or use
  `..Default::default()`.
- `rustograph::mcp::Server` now borrows the document it serves and is
  `Server<'d>`. It has no public constructor or methods, so only code that names
  the type is affected.
- `rustograph::graph::Document` has a new public field, `revision`
  (`Option<String>`): the source revision a saved graph was harvested from.
  `graph::document` leaves it `None`, and `graph --out` fills it (see below).
  Code that builds a `Document` with a struct literal must add the field, and
  code that destructures every field must add it or `..`. The serialized
  document stays version 1: the key is omitted when empty, and readers that
  ignore unknown fields still load snapshots that have it.

## Saved snapshots and MCP

Regenerate snapshots with `rustograph graph --level symbol` before requesting
symbol analyses. CLI analysis errors return 2 with empty stdout. MCP summary,
search, and supported coarse cycle views remain usable on a coarse snapshot;
symbol-level tools return an explicit error. No missing detail is manufactured.

A snapshot whose vertex `id`, `module` or `crate`, or edge `from`/`to`, contains
a control character (C0, DEL or C1, including NEL) or U+2028/U+2029 is rejected
with exit 2 before any output, and `mcp --graph` fails to start. rustograph never
writes such ids, so the error names the field and code point; regenerate the
snapshot instead of passing another tool's ids to the terminal or to Mermaid.

`graph --out` records the source revision of a harvested graph as `revision`:
git `HEAD` when the `--dir` work tree is clean. `reach` and
`impact --format language-traversal` with `--graph` use that value instead of
the current git `HEAD`, which may have moved since the snapshot was saved, and
run no git. A snapshot that records none (written before this change,
redirected from stdout, or saved from a modified work tree), or whose
`revision` breaks the `--revision` rules, gets no `revision` and a
`revision-unknown:` limitation. Pass `--revision`, or regenerate the snapshot
with `rustograph graph --level symbol --out FILE`.

## File output

Graph JSON/Mermaid, schema, route, and rules-baseline files are prepared in the
output directory and published only after the complete contents are written
and synced. A failed staged write preserves the previous output. Input/source
aliases, symbolic links, non-regular files, and read-only outputs are rejected.
Unix also rejects hard-linked output files; on other platforms staged replacement
does not truncate the original inode referenced by another hard link. Parent directories are created when needed.

Literal include inputs are protected even with non-Rust extensions. When a
direct include macro computes an unresolved path, an `include!` target is not a
regular file of at most 64 MiB (a device or FIFO is never opened), or an include
is nested in other macro tokens, replacing an existing output
is rejected conservatively; use a fresh output path or stdout. This protection
does not claim that the syntax-only graph expands include macros.

A declared module file that cannot be read or parsed no longer makes `--out`
fail for a command that still produces its document on stdout: `schema` counts
the file under `unparsed-sources` and writes the same document to a fresh output
path. The unreadable file is protected, but the modules and include inputs it
declares are unknown, so replacing an existing output is rejected conservatively
with an error that names the file. Commands that need the symbol graph, such as
`graph` and `rules`, still exit 2 on such a file.

The rename is a single-file publication, not a multi-file transaction or a
power-loss durability guarantee. On systems that reject replacing an open or
existing destination, publication fails without truncating the original.

## Rules configuration and baselines

`rules` and the MCP `rustograph_rules` tool read a rules file and its baseline up
to 64 MiB. Paths that the analyzed repository chooses, the `.rustograph.yml`
found in `--dir` and the file named by its `baseline:`, must be regular files: a
device, FIFO, or directory there is never opened, and `rules` exits 2 (the MCP
tool answers with an error) naming the path. Paths given on the command line,
`rules --config` and `rules --baseline`, may also be pipes or process
substitutions such as `--config /dev/stdin` or `--baseline <(…)`. The
`rustograph mcp --config` file must be a regular file, because the server reads
it again on every rules call.

## Mermaid output

Mermaid labels write `"`, `%`, `<`, `>` and `&` as Mermaid entity codes
(`#quot;`, `#37;`, `#lt;`, `#gt;`, `#amp;`), and an edge line that uses one ends
with `;`. Rendered labels still read as the vertex ids, and trait implementation
methods such as `Type::<Trait>::method` keep `<Trait>` instead of losing it as an
HTML tag. Lines without those characters are unchanged.

## Path budgets

The path expansion budget also limits queued partial-path copies. Completed
paths use separate bounded slots, so a known direct path remains available even
when partial paths have exhausted the budget. Dropped search states set
`truncated`; a partial search is never reported as complete.
