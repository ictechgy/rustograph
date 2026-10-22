# Initial change-impact evidence

This is bounded evidence for review usefulness, not an application-wide recall,
precision, productivity or failure-prediction claim.

The nine new Git-flow regressions exercise real source capture: deleted
declarations retain old callers; a before-only call and after-only call cannot
form a hybrid path; rename retains independent old/new identities; body-only
edits change content identity; untracked configuration remains visible; CLI
and MCP agree on bounded captures; trait/callback source compiles and its
connected test root remains a review candidate; invalid/conflicting input
fails instead of returning empty success. Staged deletion and rename remove
the old HEAD files from working captures, including when their old module
declarations remain; unresolved references remain limitations. A harmless marker
also reproduced repository clean-filter execution during working diff; the
corrected capture suppresses those commands. The trait/callback fixture is run by
Cargo, but these tests do not establish that all dynamic callbacks are resolved.

Ordinary impact regression preserves harvested unresolved-path limitations,
revision labels, unknown content freshness, via/evidence and depth boundaries
while retaining the old output-count `truncated` meaning. Existing traversal
and graph contracts are exercised by the repository's full suite.

## Real repository patch

On Windows GNU Rust 1.99.1, the new flow was run against Rustograph's actual
`14b31f9d4b2764724cbb148f8c3350220ed88dfe` →
`e00de16b1bb820a6f5123b762b0da2fb4bf40428` change (fan-out limitation reporting):

```sh
rustograph change-impact --base 14b31f9d4b2764724cbb148f8c3350220ed88dfe \
  --head e00de16b1bb820a6f5123b762b0da2fb4bf40428 --depth 2 --max 1000
```

Preparation plus analysis took 2,737 ms in that local run. Before/after had
237/240 file-wide seeds and both reached the 1,000-row report budget. The
`analysis::dead` seed preserved a direct path to `cli::cmd_dead`, and its after
snapshot included `dead_carries_the_fan_out_cap_limitation` as a direct test
caller. Before lacked that newly added test. Both recorded content capture,
depth limits, unresolved references, omitted external crates/macros, cfg and
tentative method fan-out.

The large candidate sets show the cost of file-wide precision: unrelated
declarations in those files are deliberately retained. This patch is not an
exhaustive ground-truth corpus, so aggregate missed/extra-candidate rates are
not claimed. The controlled deletion/hybrid tests provide explicit positive
and negative assertions; the real patch demonstrates usable witnesses and
preparation cost under a visible output cap.

The initial implementation PR's Ubuntu CI passed the required 90% coverage
gate at 94.68%, CLI contract, clippy, formatting and self-analysis. Windows
snapshot/export checks and release-helper checks on Windows, macOS and Linux
also passed. These are build/test evidence, not a release installation claim.
Semantic-feature/macOS runtime analysis and broader application corpora remain
outside this initial local run; the syn limitations remain part of the output.
