# Migrating to rustograph 0.8

These notes cover changes on `main` after the v0.7.0 release. They have not
shipped in a release yet; confirm the matching tag and assets on GitHub
Releases before relying on them. CLI flags and defaults, graph document v1 and
language-traversal v1 stay compatible: the changes add edges, retention roots
and limitation lines, and no field is renamed or removed.

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

Expect `dead` to report fewer unreachable items, `rules` to see dependencies
that a field type or a foreign-type impl body carried, and module- and
type-level `cycles` to include field-type references. Two data types that hold
each other as fields (an AST's `Expr` and `Stmt`) are a type-level dependency
cycle, so `cycles --level type --strict` (and `--level module --strict` when the
types live in different modules) can start failing on such definitions; this is
intended, because at those levels the cycle is a real dependency between the
types. Symbol-level `cycles` leaves out a field-type reference between two data
types, so recursive data definitions do not become new symbol-level cycles. Saved baselines that list a now-reachable
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
