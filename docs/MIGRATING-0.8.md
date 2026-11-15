# Migrating to rustograph 0.8

These notes cover changes on `main` after the v0.7.0 release. They have not
shipped in a release yet; confirm the matching tag and assets on GitHub
Releases before relying on them. CLI flags and defaults, graph document v1 and
language-traversal v1 stay compatible: the changes add edges, retention roots
and limitation lines, and no document field is renamed or removed. One kind of
edge changes its target (a cross-crate `#[derive(..)]` path, below), and the
Rust library's `modtree::Module` import table changes shape (see
[Rust library callers](#rust-library-callers)).

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
- A path in an attribute, a derive list or an item-position macro call
  resolves in the macro namespace. A cross-crate `#[derive(pm::Hello)]` now
  points at the `#[proc_macro_derive(Hello)]` function even when the
  proc-macro crate also has a struct named `Hello`; before, the edge went to
  that struct, which a proc-macro crate cannot export. Inside the proc-macro
  crate, `Hello { .. }` and `-> Hello` still resolve to the struct. Such a
  struct used only by the derive path can now be reported by `dead`.

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
- `modtree::Module::imports` changes type from `BTreeMap<String, Import>` to
  `BTreeMap<String, PerNs<Binding>>`: each name maps to one binding per
  namespace (`types`, `values`, `macros`), and a binding is either
  `Binding::Resolved(Import)` or `Binding::Unresolved(Scope)`.
  `modtree::Module::unresolved_imports` is removed; its entries are the
  `Binding::Unresolved` slots. Read a name's binding with
  `module.imports.get(name).and_then(|b| b.get(Ns::Value))`, or iterate
  `PerNs::iter` for every namespace. `modtree::Ns`, `modtree::PerNs` and
  `modtree::Binding` are new public types.
- `modtree::Module` adds `macro_items` (named `macro_rules!` items), and
  `value_items` now also holds tuple and unit structs, whose constructors live
  in the value namespace. Build modules with `Module::new`.
- `modtree::ModTree::resolve_in` is new: `resolve` with the namespace of the
  last segment given (`Ns::Type` for type positions and struct literals,
  `Ns::Value` for calls and value paths, `Ns::Macro` for macro calls and
  attributes). `resolve` keeps its signature and resolves without knowing the
  position.
