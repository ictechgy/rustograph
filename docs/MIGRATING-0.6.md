# Migrating to rustograph 0.6

For source builds after 0.6.0, Rust callers constructing `harvest::BodyItem`
must provide `generic_scopes` (use an empty vector when there are no enclosing
generic parameters). Callers constructing `harvest::ImplBlock` must provide its
borrowed `generics`. This preserves lexical type/const namespaces when body
references are harvested. CLI and graph document schemas are unchanged.

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
