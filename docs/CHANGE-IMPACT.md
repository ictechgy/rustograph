# Review an actual Git change

```sh
# Compare two commits (head defaults to HEAD).
rustograph change-impact --dir ./my-crate --base main --head feature/change
# Include staged, unstaged, deleted, renamed and untracked non-ignored files.
rustograph change-impact --dir ./my-crate --base HEAD --working-tree
# Follow the whole static reverse closure, with a bounded output.
rustograph change-impact --base HEAD~1 --depth 0 --max 1000
```

The command resolves refs to exact commits, prepares temporary local snapshots,
and harvests each revision separately. Working source files are copied into the
temporary after snapshot; Cargo preparation does not write into the original
checkout. Old and new paths are retained for renames. A deleted declaration can
therefore retain its historical callers. A path never combines a before edge
with an after edge. Git and Cargo must be available, and both captured projects
must support Cargo metadata. Capture failures are errors, not empty reports.

`before` and `after` contain their own seeds, impact reports, witness hops,
connected `review_roots`, unmapped files and limitations. `review_roots` are
harvested test/entry roots connected by graph edges (or selected as seeds), not
predicted failing tests. HTTP endpoint and body/query/header/DB compatibility
are not inferred by this command. Run the appropriate tests and bridge analysis.

Seeds select declarations in the changed files, not just modified lines.
Cargo manifests, lockfiles, build scripts and `.cargo` changes select broad
declaration candidates and say so. Other configuration files stay explicit in
`unmapped_files`: runtime configuration and callback registration are not
automatically interpreted. Dynamic dispatch, proc macros and other harvested
limitations remain visible. Empty candidates never prove safety or authorize
skipping tests. `state` remains `incomplete`.

`provenance.revision` identifies the resolved commit; working-tree provenance
explicitly says that the source may differ from that commit. `content_digest`
hashes tracked and untracked non-ignored regular-file bytes, including body-only
changes. It is not the topology hash. The digest scope is explicit. Changes
during capture yield `stale`; external/generated/unresolved inputs yield
`unknown` when the collector cannot establish coverage. If Cargo creates or
changes a missing lockfile in the isolated snapshot, capture also stays stale:
commit a reproducible lockfile and rerun. A digest is not a claim of complete
semantic analysis. Working-tree symlinks/submodules/non-regular inputs require
an explicit supported capture and are rejected instead of silently copied.

`depth` defaults to 2 for both CLI and MCP; 0 means no depth limit. `max` defaults
to 200 and caps total impacted rows per revision; at most 256 seeds per revision
are selected, deterministically. `truncation_reasons` records depth, result and
seed limits. `preparation_ms` includes local snapshot preparation, metadata,
hashing and both source harvests; it does not imply compilation or test time.

## MCP

Start `rustograph mcp --dir ./my-crate`, then call:

```json
{"name":"rustograph_change_impact","arguments":{"base":"HEAD~1","working_tree":true,"depth":2,"max":200}}
```

This tool explicitly captures fresh source on each request in a short-lived
process. It does not reuse the server's frozen graph and cannot be called in a
`--graph` session. `head` and `working_tree: true` are mutually exclusive.

Existing `impact ID` and `rustograph_impact` remain current-snapshot queries.
They retain their existing defaults (CLI depth 1, MCP depth 0/full closure).
Use an explicit depth to match existing workflows across transports. They now
retain harvested limitations, revision provenance, witness hops and evidence.
A graph document alone does not prove live content freshness; these results say
`content_freshness: unverified`. Use the Git change command for fresh captures.

## Compatibility

Existing impacted rows and `truncated` keep their original meaning;
`truncated` is the output-count cap. Additive `depth_limited` and
`truncation_reasons` expose depth restriction. Consumers deciding completeness
must inspect the new fields, limitations, freshness and `state`; they must not
treat `truncated: false` as a pass. Graph document v1 and language-traversal v1
are unchanged. Unknown JSON fields remain safe to ignore for older consumers.

Rust library `analysis::impact` and `QueryIndex::impact` keep their signatures.
`ImpactResult` has new public fields; callers constructing it with struct
literals must populate them or, preferably, obtain results through the query
functions. `Neighbor` is unchanged. New `change::Request`/`change::analyze`
provide the same Git workflow for library callers; this uses the existing
source harvester's process-lifetime AST storage, so long-lived hosts should
isolate repeated captures as the MCP command does.
