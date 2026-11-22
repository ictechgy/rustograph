# Migrating to rustograph 0.8

These notes cover changes on `main` after the v0.7.0 release. They have not
shipped in a release yet; confirm the matching tag and assets on GitHub
Releases before relying on them. CLI flags and defaults, graph document v1 and
language-traversal v1 stay compatible: no document field is renamed or
removed. The graph built from the same source changes in four ways:

- Edges, retention roots and limitation lines are added (most of the list
  below).
- Some confirmed edges point at a different vertex, because a path now
  resolves in the namespace of its position. Every path in an attribute or a
  derive list that has two or more segments, every item-position macro call
  (`dep::mac!();`), and a macro call in a body whose name is also a local
  item (`info!()` beside `fn info` and `use crate::logging::info`) now point
  at the macro rustc picks. Beside `use log::info`, a macro outside the
  graph, the local `fn info` is no longer the confirmed target either: the
  call has no confirmed workspace edge (with `--deps`, a `references` edge to
  the `log` crate vertex). The
  target of position-agnostic resolution, usually the vertex the path pointed
  at before, keeps a `tentative` edge when it is visible from that module, so
  `dead` reports nothing new for it, while `cycles` and `rules` now see the
  macro. A value
  or type path whose name is bound to different vertices in different
  namespaces (`fn harvest` beside `use crate::harvest`) also resolves in its
  own namespace. A glob `use a::b::*` whose `b` names both a function and a
  module now targets the module.
- Some confirmed edges gain a `cfg`: a path that resolves through a
  `#[cfg]`-gated `use`, re-export or glob carries that condition, so
  `--target` and `--exclude-tests` drop the edge where the import does not
  exist. Without those flags the edge stays, as before.
- Retention narrows in three places, all following rustc. A derive or
  attribute path into another crate whose old target is not visible from
  the referencing module (a private struct of a proc-macro crate, which a
  proc-macro crate cannot export) gets no tentative edge, so such a struct
  used only by that path can now be reported by `dead`. A value or type path
  that used to resolve to a local item or an import of another namespace has
  no tentative edge either: beside a glob that brings `fn Foo`,
  `use crate::types::Foo` (a struct with named fields) no longer makes
  `Foo()` a use of the struct, which `dead` can now report. A target left over
  from an early import pass, which the settled tables no longer hold, no
  longer keeps the vertex it named alive (see the graph results below). Over
  ten crates (syn, tokio, regex-syntax, ra_ap_hir_def,
  ra_ap_hir_ty, rowan, serde, serde_derive, clap, rustograph itself) and
  zerocopy 0.8.50, the symbol graph harvested after the namespace change lost
  no edge that the graph harvested before it had.

The Rust library's `modtree::Module` import table changes type and loses a
field (see [Rust library callers](#rust-library-callers)).

## Graph and analysis results

The harvest now sees more of the code, so a graph built from the same source
can have more vertices and edges than before:

- Libraries with a `[lib] crate-type` (`cdylib`, `staticlib`, `rlib`, `dylib`)
  and proc-macro libraries are harvested like any lib target. Before, such a
  crate had no vertices at all and `deps` reported the dependencies it uses as
  unused.
- `Self` paths, `<T as Tr>::m()` / `<T>::m()`, associated functions passed as
  values (`.map(T::new)`), struct/enum/union field types, function-local items,
  statement macros inside nested blocks, `macro_rules!` bodies, macro arguments
  that are not an expression list, and the bodies of impls whose self type is
  outside the workspace now produce edges (some of them `tentative`; see the
  README graph model).
- `#[unsafe(no_mangle)]`, `#[unsafe(export_name = "…")]`, `#[cfg_attr(…, no_mangle)]`,
  `#[used]` / `#[no_mangle]` / `#[global_allocator]` statics and `#[no_mangle]`
  impl methods are retention roots.
- A module's imports keep the type, value and macro namespaces apart, as rustc
  does. A glob import brings in a name in every namespace it has in the target
  module, and an explicit `use` or a local item shadows a glob only in its own
  namespace. An explicit `use a::x` brings in every namespace `x` has in `a`.
  Before, a name held one binding, so in a module with both `fn harvest` and
  `use crate::harvest::{self}`, a child's `use super::*` dropped the module
  and `harvest::f()` did not resolve; `use crate::other as shared` beside a
  glob that also brought `fn shared` dropped that function. Calls through
  those paths now have edges, and items reached only through them stop being
  reported by `dead`.
- An explicit `use` whose namespace the item tables cannot tell (a path into
  an external crate or std, or a name a macro registers, such as
  `pub use zerocopy_derive::Unaligned`) never beats a local item or another
  import of the same name whose namespace is known: rustc rejects an explicit
  `use` that shares a namespace with either. `pub trait Foo` beside
  `pub use foo_derive::Foo` keeps `Foo` the trait in value positions and
  macro tokens, and `pub use crate::split_at::SplitAt` keeps its trait beside
  a later `pub use zerocopy_derive::SplitAt`, whatever the source order. Such
  a `use` still hides a glob import of the same name, as before. Alternative
  imports split by `#[cfg]` (`#[cfg(feature = "pl")] use crate::pl::Condvar;`
  beside `#[cfg(not(feature = "pl"))] use std::sync::Condvar;`) share one
  namespace, so the rustc rule above does not cover them; the workspace side
  now wins whatever the source order (before, the later `use` won). Edges
  from bodies that use the name (`Condvar::new()`) carry the import's
  `#[cfg]`, like the module's `uses` edge (see below). In a macro position the
  rule turns around: a local function or trait, or an import whose namespace
  is known to be type or value, cannot be a macro, so such a `use` is the only
  candidate. `info!()` beside `fn info` and `use log::info` no longer makes a
  confirmed `call` edge to `fn info` (rustc warns that `fn info` is never
  used); the function keeps a `tentative` edge, so `dead` is unchanged, and a
  false symbol-level cycle through it (`fn info` calling back into the caller)
  is gone.
- Two imports that bind one name in one namespace to different vertices both
  stay. rustc never lets two explicit `use`s (E0252) or two globs (E0659 on
  use) share a name in one build, so they belong to builds that `#[cfg]`
  keeps apart: `#[cfg(unix)] use crate::unix::Sys;` beside
  `#[cfg(windows)] use crate::windows::Sys;`, two platform globs, or
  `#[cfg(..)] use linux as platform;` beside `#[cfg(..)] use bsd as
  platform;`. Before, the later `use` won and the other build's target lost
  its edges; rustix 1.1.5 reported its Linux `ioctl::linux::compose_opcode`
  and ten constants as unreachable. Now a path through the name gets an edge
  to every alternative, each under its own condition, and `--target` and
  `--exclude-tests` drop the edges whose import does not exist in that build.
  Several `use Trait as _` in one module, which bind no name, all stay as the
  module's `uses` evidence, whether written there or brought in by a glob
  (syn 2.0.119's `syn::error` lost `PunctExt` that way). Two `use`s of the
  same vertex merge to the weaker condition, as two such globs already did.
  When an import that resolves and one that does not (a std path) bind the
  same slot, the one that resolves stays whatever the source order: before,
  `#[cfg(not(..))] use std::sync::Mutex;` after
  `#[cfg(feature = "pl")] use extdep::Mutex;` erased the evidence that the
  crate uses `extdep`, and `deps` reported it unused.
- The same holds when `#[cfg]` splits a name between a `use` and another
  binding. An explicit `use` that carries its own `#[cfg]` no longer hides a
  glob import of the same name: in builds without the condition the glob's
  name is visible. `f()` beside `use a::*;` and `#[cfg(test)] use a::f;` is an
  unconditional edge, and `Clock::now()` beside `use real::*;` and
  `#[cfg(test)] use mock::Clock;` reaches both clocks; before, `dead` reported
  `real::Clock::now`. The glob side keeps its own condition (no negation of
  the `use`'s condition is added, so that edge also stays in the `use`'s own
  build). A local item whose every declaration carries `#[cfg]` and an import
  of the same name in the same namespace are alternatives too:
  `#[cfg(feature = "x")] fn sum()` beside
  `#[cfg(not(feature = "x"))] use scalar::sum;` makes `sum()` reach both;
  before, only the local item, and `dead` reported `scalar::sum`. An impl whose
  self type is such a split import (`impl Sys { .. }`) is harvested once per
  alternative: its method vertices sit under each alternative type with that
  import's condition, so `--target` keeps the methods of the build it
  describes. An impl whose trait path is split, and route handlers, still
  resolve to one target.
- While the import tables settle, a target that an early pass resolved and a
  later pass took back (`pub use foo::bar` read through the crate-root
  fallback before the module's own glob was expanded) no longer survives
  between modules that glob each other: when a pass takes a target back, the
  tables settle once more from empty, resolving explicit `use`s and glob paths
  against the settled tables. The stale target used to give a false confirmed
  `call` to the crate-root `foo::bar`, which rustc warns is never used; an
  item kept alive only by that edge is now reported by `dead`. None of the
  seventeen corpus crates below needed the extra round.
- Over seventeen crates (syn, tokio, regex-syntax, ra_ap_hir_def,
  ra_ap_hir_ty, rowan, serde, serde_derive, clap_builder, zerocopy, rustix,
  mio, getrandom, parking_lot_core, crossbeam-utils, hashbrown and
  rustograph itself), the graph lost no edge, gained 92, and put a condition
  on 341 edges that had none; `dead --retain-public` reports 13 fewer items
  and none new.
- A path in an attribute, a derive list or a macro call resolves in the macro
  namespace. A cross-crate `#[derive(pm::Hello)]` now points at the
  `#[proc_macro_derive(Hello)]` function even when the proc-macro crate also
  has a struct named `Hello`; before, the edge went to that struct, which a
  proc-macro crate cannot export. Inside the proc-macro crate,
  `Hello { .. }` and `-> Hello` still resolve to the struct. Such a struct
  used only by the derive path can now be reported by `dead`. When resolving
  the path without a namespace reaches another vertex visible from the
  referencing module (a trait `Hello` that `mylib` exports beside
  `pub use mylib_derive::Hello`, a local `fn info` beside an imported
  `info!`), that vertex keeps a `tentative` edge.

Expect `dead` to report fewer unreachable items, `rules` to see dependencies
that a field type or a foreign-type impl body carried, and module- and
type-level `cycles` to include field-type references. Two data types that hold
each other as fields (an AST's `Expr` and `Stmt`) are a type-level dependency
cycle, so `cycles --level type --strict` (and `--level module --strict` when the
types live in different modules) can start failing on such definitions. This is
intended: those levels ask whether types and modules can be layered, and a
field is a dependency neither type can exist without. Symbol-level `cycles`
asks which code recurses, so it leaves out a field-type reference between two
data types, as it leaves out direct recursion; recursive data definitions do
not become new symbol-level cycles, while code that recurses over them still
is one. The README graph model gives both rationales. Saved baselines that list a now-reachable
item keep working; the item simply stops matching a finding.

## Rust library callers

A new field on a public struct breaks struct literals and exhaustive patterns
alike. Add the new fields, or end such patterns with `..`.

- `harvest::ModuleDecls` adds `decl_refs`, the declaration-level path
  references (field types and `macro_rules!` bodies) as the new public
  `harvest::DeclRef`. Resolve them with the new `harvest::decl_edges` after the
  imports are filled, as the source harvest does.
- `harvest::Harvest` adds `foreign_self_impls`, the number of impl blocks whose
  self type is not a workspace vertex. `Harvest::default()` starts it at zero.
- `harvest::impls` keeps its signature but now also returns the method bodies
  of such impl blocks, owned by the module that declares the impl. The source
  harvest, which knows the trait-declared methods, keeps those firm edges on the
  module and also gives the trait's declared method `tentative` edges to what
  the body calls, so reachability flows through the trait method without the
  trait's module appearing to depend on the impl's module.
- `harvest::BodyItem` derives `Clone` (additive).
- `deps::UnusedDep::kind_label` is new: the text form of `kind`, with cargo's
  empty normal-dependency kind written as `normal`. JSON keeps `kind` as cargo
  reports it.
- `modtree::Module::imports` changes type from `BTreeMap<String, Import>` to
  `BTreeMap<String, PerNs<Binding>>`: each name maps to one binding per
  namespace (`types`, `values`, `macros`) plus `undetermined`, the binding of
  an explicit `use` whose namespace the item tables cannot tell. A binding is
  `Binding::Resolved(Import)`, `Binding::Alternatives(Vec<Import>)` (two or
  more imports of different vertices that `#[cfg]` keeps apart; the first is
  the later one in source order), or `Binding::Unresolved(Scope)`.
  `Binding::imports` returns the resolved imports of any of them, so match on
  it rather than on the variants. `Import::cfg` now also carries the
  conditions of the re-exports an explicit `use` resolved through.
  `modtree::Module::unresolved_imports` is removed; its entries are the
  `Binding::Unresolved` slots, usually `undetermined`. Read a name's binding
  in one namespace with `module.imports.get(name).and_then(|b| b.get(Ns::Value))`
  (it does not return `undetermined`), iterate `PerNs::iter` for the
  namespaces that are known, or `PerNs::bound` for every binding including
  `undetermined`. `modtree::Ns`, `modtree::PerNs` and `modtree::Binding` are
  new public types.
- `modtree::Module` adds `cfg_gated`, the names whose every declaration in a
  namespace carries `#[cfg]`; such an item and a same-namespace import of the
  name resolve as alternatives. `modtree::Module` and `modtree::ModTree` now
  derive `Clone` (additive). `harvest::impls` returns one set of method
  vertices per alternative when an impl's self type is a `#[cfg]`-split
  import.
- `modtree::Module` adds `macro_items` (named `macro_rules!` items), and
  `value_items` now also holds tuple and unit structs, whose constructors live
  in the value namespace. Build modules with `Module::new`.
- `modtree::ModTree::resolve_in` is new: `resolve` with the namespace of the
  last segment given (`Ns::Type` for type positions and struct literals,
  `Ns::Value` for calls and value paths, `Ns::Macro` for macro calls and
  attributes). `resolve` keeps its signature and resolves without knowing the
  position.
- `modtree::ModTree::resolve_all` and `modtree::Resolved` are new:
  `resolve_all(from, segs, deps, ns)` returns every vertex a path names
  across builds, one per `#[cfg]`-split alternative, each with the `cfg` of
  the imports it went through. `resolve` and `resolve_in` keep their
  signatures and return the first of those targets; build edges from
  `resolve_all` so that other builds' targets keep their edges.
  `modtree::with_import_cfg` joins such a `cfg` to an edge's own condition.
- `modtree::ModTree::visible_from` is new: whether a module or item vertex is
  visible from a module, by its declared visibility only.
