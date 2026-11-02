# Migrating to rustograph 0.6

For source builds after 0.6.0, Rust callers constructing `harvest::BodyItem`
must provide `generic_scopes` (use an empty vector when there are no enclosing
generic parameters). Callers constructing `harvest::ImplBlock` must provide its
borrowed `generics`. This preserves lexical type/const namespaces when body
references are harvested. CLI and graph document schemas are unchanged.
`modtree::Module` adds `type_items`, the names of its type-namespace items
(`mod`, `struct`, `enum`, `union`, `trait`, `type`); `Module::new` starts it
empty and `fill_items` fills it. Callers constructing `Module` by struct
literal must provide it. A path prefix (`name::item`) resolves to such a local
item before a same-named import, and to the import when the local name is only
a value or macro item.
Paths now resolve through `use` re-exports to the defining item, so one
`fill_imports` pass per module no longer settles every import: callers that
build a `ModTree` themselves should call `modtree::fill_imports_settled`, which
repeats the pass until the import tables stop changing (at most
`modtree::MAX_IMPORT_PASSES`) and returns the number of modules still changing.
`fill_imports` now rebuilds the module's import tables from its own `use`
items instead of adding to them. Glob imports bring in only the names visible
from the importing module, so the tables carry visibility: `modtree::Import`
adds `scope` (a `modtree::Scope`, `Scope::Public` for a `pub use`) and
implements `PartialEq` and `Eq`; `modtree::Module` adds `item_scopes` (declared
names to their widest `Scope`; a missing name counts as public) and
`unresolved_imports` (names an explicit `use` brings in from outside the graph,
such as `std`, mapped to their `Scope`). Callers constructing `Import` or
`Module` by struct literal must provide them; `Module::new` starts both maps
empty.

`change-impact --base REV [--head REV | --working-tree]` and the MCP tool
`rustograph_change_impact` capture actual changes into separate before/after
source graphs. See [the workflow and limitations](CHANGE-IMPACT.md).

Existing `impact ID` and `rustograph_impact` still query a single graph.
Their CLI depth 1 and MCP depth 0/full defaults are unchanged. Pass an explicit
depth to align them. `truncated` still means the output-count cap; consumers
assessing completeness must also inspect new `depth_limited`,
`truncation_reasons`, `limitations`, `state` and provenance freshness.

Graph document v1 and language-traversal v1 are unchanged. Ordinary impact JSON
adds fields without changing impacted row shapes. Older clients should ignore
unknown fields, but must not interpret empty results as a safety guarantee.

`analysis::impact` and `QueryIndex::impact` retain their signatures. Their
`ImpactResult` adds `mode`, `state`, `level`, `max_results`, `depth_limited`,
`truncation_reasons`, `limitations`, `provenance`, `witnesses` and
`interpretation`. Rust callers constructing or exhaustively destructuring this
public struct must update those fields or use the query functions. `Neighbor`
is unchanged. `change::Request` and `change::analyze` provide the new workflow;
long-lived hosts should isolate repeated source captures as MCP does.

This minor release also includes the changes already documented under
[changes after 0.5.0](MIGRATING-0.5.md), including public graph/harvest fields,
lifetimes, input validation and source-reading restrictions. The dependency
versions remain pinned by Cargo.lock; the release metadata changes only the
rustograph package version.
