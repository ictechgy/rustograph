# Source identity correction (unreleased)

This local correction follows 0.6.0. It does not change the package version or
claim a published release. Graph document v1 remains readable.

## IDs and saved graphs

A Rust path is not a complete source declaration identity. Two implementations
such as `From<String>` and `From<&str>`, specialized inherent implementations,
cfg alternatives, anonymous constants, and different namespaces can have the
same path after syntax-only path resolution.

When several harvested declarations share a path, non-module declarations now
receive a source-instance segment before their final name:

```text
base_db::target::TargetLoadError::<From>::@crates/base-db/src/target.rs#1234#Method::from
```

The number above illustrates a declaration byte offset; it is not a real source
location. The segment contains the workspace-relative file, declaration byte
offset and vertex kind. Unsafe path bytes are percent encoded. A file outside
the workspace retains its full path. Modules and a sole type owner sharing a
name with a value retain their original paths so their children, projection
and name resolution still have an anchor. Declarations with an
already unique path keep their original ID. Repeated loading of the exact same
source occurrence is coalesced; distinct cfg alternatives remain separate.

These IDs identify source occurrences, not compiler-resolved generic types.
Inserting text before a colliding declaration changes its byte offset. Adding
a new collision can also change a formerly unique ID. Treat IDs as opaque
within one harvested revision and regenerate ID-keyed roots, bridge-facts,
traversal documents and saved query inputs together after a source update.
Do not join old and new IDs as though they were the same graph revision.
Requested root IDs that no longer exist are counted in `limitations` rather
than silently disappearing or being assigned to arbitrary source instances.

Bodies, attributes and explicitly annotated entry/test roots retain their
exact source owner. A root annotation on one cfg alternative never roots its
unannotated sibling. Type-owned methods are not attributed to a same-name
function. Semantic edges use the declaration's actual source site.
When syntax resolution cannot select among distinct declarations, all
compatible targets are preserved as `tentative: true`; they are candidates,
not independent confirmed calls. Value calls are separated from same-name
module paths. The measured collision count is included in `limitations`.
External dependency boundary vertices still represent package names and do not
inspect or distinguish the internals of different dependency versions.

Old v1 snapshots retain their original IDs when loaded. They cannot be split
reliably without their source revision. `query`, `impact` and `paths` report duplicate ID
counts in `limitations` and downgrade related evidence to `candidate`, including
zero-hop self paths. Loading a stored graph also marks edges incident to those
IDs tentative for graph exports, cycles and rules; the input file is preserved. Rebuild
such a graph to obtain independent source identities. The semantic cache key
changes to `rustograph-cache-v3-source-identities`, and cache entries containing
duplicate IDs are rejected even if their key matches. No old file is rewritten.
The key also hashes the compiled graph producer source and Cargo.lock, so an
interim local candidate cannot reuse another candidate's graph merely because
both display package version 0.6.0. The cache file layout remains unchanged.

## Neighbor evidence

The existing `id`, `edges`, `direction` and `depth` fields are unchanged. Every
`query.neighbors` / `impact.impacted` row also has:

```json
{
  "evidence": "direct",
  "edge_evidence": [
    { "relationship": "call", "evidence": "candidate" },
    { "relationship": "references", "evidence": "direct" }
  ]
}
```

`direct` means that a shortest graph path contains no tentative hop. It is
graph evidence, not a claim that the program executes that route. `candidate`
means every shortest route for that row/relation relies on a tentative hop.
An earlier candidate hop remains candidate even if a later edge is confirmed.
If several shortest paths exist, a confirmed path is preferred for that
relation. The older `edges` list alone does not communicate this distinction.
`query` additionally carries the document's `limitations`. Existing impact
witnesses keep their `direct`/`candidate` interpretation. Cycles and rules
continue excluding tentative dependencies; dead/reach continue using them
conservatively. No candidate edges are discarded to make reports look precise.
Mermaid labels tentative edges as `call (candidate)` (or the corresponding
relationship) so exporting the graph does not erase that distinction.

## Rust library callers

`Neighbor` adds `evidence` and `edge_evidence`; `NeighborEvidence` is a new public
type. `QueryResult` and `PathsReport` add `limitations`. Callers constructing these public structs
must populate those fields or call the query functions. Function signatures
are unchanged. The existing `Harvest`, `decls` and `impls` APIs are unchanged;
the declaration registry is internal to a complete source harvest. Source declarations,
query bounds, graph format and optional semantic dependency pins are unchanged.
