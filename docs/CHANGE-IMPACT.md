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
Git filters/hooks/fsmonitor are suppressed, and Git is resolved only from
absolute PATH entries. The capture uses raw file bytes; projects that require
filter-generated source must prepare that source explicitly before analysis.
On Unix, each new temporary source checkout is created with owner-only permissions
before Git writes into it; existing checkout permissions are never changed.

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
Tracked descendants do not bypass this rule when an ignored directory has become
a symlink: copying and fingerprinting check every path component below the
canonical repository root, and reject symlink or non-directory ancestors.
These checks assume the repository is quiescent during capture. They do not use
descriptor-relative atomic reads and do not prove safety against concurrent
hostile path replacement; keep source paths unchanged until capture finishes.

## Cargo configuration during Git capture

Git captures use a separate metadata context; the ordinary graph/library `load`
API keeps its existing caller-working-directory policy. For each captured
revision, `change-impact` reads `.cargo/config` or `.cargo/config.toml` from the
repository root through the requested project directory, parses bounded regular
UTF-8 TOML files, and passes only supported local data to offline Cargo metadata
in a new private neutral directory. Caller-CWD configuration is not mixed into
that revision's configuration. Config files in an ancestor of the neutral
directory cause an explicit error; select another `TMPDIR` instead of inheriting
those settings silently.

The supported data-only subset is `paths`, path-only `[patch.<registry>.<package>]`
tables, and local `[source.<name>]` directory/`replace-with` chains. Directory
paths are relative to the parent of the config file's `.cargo` directory; each
must exist within the captured repository with no symlink component. Root and
member config arrays accumulate in Cargo order, deeper path patches override
the same patch leaf, and source definitions cannot change kind across levels.
These path and merge rules follow the [Cargo configuration reference](https://doc.rust-lang.org/cargo/reference/config.html#config-relative-paths)
and are checked with actual offline Cargo fixtures.
Both config spellings at one level, unknown replacement targets/cycles, path
escapes, invalid or duplicate TOML keys, and unsupported combinations are errors.
This does not support every Cargo configuration setting or external vendor cache.

Environment, include, wrapper/program, registry/authentication, network, and
other unsupported project settings are rejected before Cargo starts. Limits are
32 config files, 1 MiB per file, 2 MiB total, and 256 rows per path/patch/source collection.
The exact TOML parser pin already present in `Cargo.lock` is reused; errors never
echo TOML contents or field values during validation. Cargo failures retain the
ordinary metadata error contract. No project build script or test is executed by
metadata capture. Missing offline dependency data fails rather than enabling a
network fetch or login.

The user-selected Cargo home and inherited environment remain external trusted
inputs and are not claimed as revision evidence; their secret config or
credential files are not read by this validation layer. Relative `CARGO_HOME`,
target paths and executable search entries are anchored to the original caller.
Relative Rustup/home or toolchain-directory selections that cannot safely be
preserved before isolated toolchain resolution require explicit absolute paths.
Captured metadata retains the existing pinned compiler and disabled wrapper
policy. These are supported local-data semantics and fail-closed boundaries,
not a claim of general Cargo-config completeness or an atomic filesystem snapshot.

`depth` defaults to 2 for both CLI and MCP; 0 means no depth limit. `max` defaults
to 200 and caps total impacted rows per revision; at most 256 seeds per revision
are selected, deterministically. `truncation_reasons` records depth, result and
seed limits. `preparation_ms` includes local snapshot preparation, metadata,
hashing and both source harvests; it does not imply compilation or test time.
The row/depth/seed limits do not bound preparation. Fingerprinting rejects each
file above 64 MiB, but there is no total file-count, wall-clock or memory budget
for cloning, hashing, Cargo metadata or harvesting. Large repositories can take
substantial time and memory before the bounded report is produced.

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
