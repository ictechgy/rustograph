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

The rename is a single-file publication, not a multi-file transaction or a
power-loss durability guarantee. On systems that reject replacing an open or
existing destination, publication fails without truncating the original.

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
