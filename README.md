# Archet

**EXPERIMENTAL.** Under development and not yet judged in use: the sound, the factory
bank and the editor can change from one version to the next. What cannot change is
listed in `COMPAT.md`.

TODO: one paragraph saying what this instrument is and how it makes its sound.

## Layout

    crates/archet            the engine. serde is its only dependency.
    crates/archet-ui         the egui editor
    crates/archet-plugin     VST3 / CLAP, via nice-plug

The editor is a separate crate rather than a feature of the engine, and that is
load-bearing. Cargo unifies features across a resolved graph, so an optional
`egui` inside `archet` would be switched on for the engine's own tests by any
`cargo test --workspace`. A crate boundary is the only thing that makes "the
engine never sees egui" true rather than merely intended:

    cargo tree -p archet --edges normal

## Building

    cargo test
    scripts/build_plugins.sh        # VST3 + CLAP bundle for this platform
    scripts/build_plugins.sh --windows

The bundle lands in `target/bundled/<platform>/`, laid out as the VST3 spec
wants. Any host that scans a directory will find it there; for a development
tree, point the host's search path at it rather than installing.

## Shared crates

The six `phonix-*` crates are path dependencies on the sibling phonix
checkout, declared once in the workspace manifest under THE SDK SWITCH. When
they get a repository and tags, that block is the only edit.

## Compatibility

`COMPAT.md` lists what cannot change: the VST3 class id, the parameter ids,
the patch's serde field names and their defaults, and the factory bank's names
*and order*.
