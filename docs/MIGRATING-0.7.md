# Migrating to rustograph 0.7

These notes cover changes on `main` after the v0.6.0 release. Several of them
break Rust library callers, so they ship in v0.7.0 rather than in a 0.6.x
release. Confirm the matching tag and assets on GitHub Releases. CLI flags and defaults, graph
document v1 and language-traversal v1 stay compatible. Saved graphs and
ID-keyed inputs also follow the
[source identity correction](MIGRATING-SOURCE-IDENTITY.md).

## Rust library callers

A new field on a public struct breaks struct literals and exhaustive patterns
alike — `let Import { target, cfg } = import;`, or a `match` arm that names
every field. Add the new fields, or end such patterns with `..`.

- `harvest::BodyItem` adds `generic_scopes` (an empty vector when there are no
  enclosing generic parameters) and `harvest::ImplBlock` adds its borrowed
  `generics`. They keep lexical type and const namespaces when body references
  are harvested.
- `modtree::Module` adds `type_items` (the names of its `mod`, `struct`,
  `enum`, `union`, `trait` and `type` items), `item_scopes` (declared names to
  their widest `modtree::Scope`; a missing name counts as public) and
  `unresolved_imports` (names an explicit `use` brings in from outside the
  graph, such as `std`, mapped to their `Scope`) and `glob_targets` (the
  modules or external crate vertices it glob-imports, mapped to the glob's
  `#[cfg]`; dependency evidence, not names). `Module::new` starts them empty,
  and `fill_items` and `fill_imports` fill them. A glob no longer binds the
  globbed module's own name in `imports`.
- `modtree::Import` adds `scope`, the visibility of the import as a
  `modtree::Scope` (`Scope::Public` for a `pub use`), and implements
  `PartialEq` and `Eq`.
- `modtree::Scope` is new and `#[non_exhaustive]`: a `match` on it needs a
  wildcard arm.
- `modtree::fill_imports` now rebuilds the module's import tables from its own
  `use` items instead of adding to them. Because paths resolve through `use`
  re-exports, one pass per module no longer settles every import: callers that
  build a `ModTree` themselves should call `modtree::fill_imports_settled`,
  which repeats the pass until the tables stop changing (at most
  `modtree::MAX_IMPORT_PASSES`) and returns the number of modules still
  changing.
- `analysis::DeadOptions` is new and `#[non_exhaustive]`: start from
  `DeadOptions::default()` and set its fields.

## Results

The same source can produce a different graph than 0.6.0: path prefixes,
re-exports, glob visibility and unresolved explicit imports now resolve the way
rustc does, which adds missing edges and removes wrong ones. The
`0.7.0` section of the [changelog](../CHANGELOG.md#070---2026-11-03) lists each
change. Regenerate saved graphs before comparing them with new results.
