# What must not change

Everything below is a wire format. It is written into files other programs read,
so it is not a naming choice and not refactorable. Each item needs a test; the
tests exist because a comment alone loses to a rename.

## Identifiers a host resolves the plugin by

    VST3 class id   PxArchetBow00001            (16 ASCII bytes, exactly)
    CLAP id         com.phonix-audio.archet
    Plugin NAME     Archet
    Plugin VENDOR   Audio

The class id goes into every exported DAWproject `Vst3Plugin` device and into
the header of every `.vstpreset`. NAME and VENDOR compose the preset directory
Cubase's MediaBay indexes. Change any of them and existing projects and preset
banks point at a plugin no host can find.

the sequencer holds a duplicate of the class id in `interop::dawproject::devices`.
Both sides assert against the literal, so a drift still fails a build, just two
builds instead of one.

Test: `archet-plugin`, `frozen_identifiers`.

## Parameter ids and persistence keys

`preset` is the one automatable parameter; `patch` and `editor-state` are
the two persisted keys. A host stores
automation against the id string.

## Patch serde field names and defaults

The patch is persisted as a blob inside the host project. Every field is
`#[serde(default)]`, so a patch written by an older build still loads and a
new field must default to today's behaviour. Field names are
the wire format; `#[serde(default)]` plus a Default impl is what lets an old
project open in a new build.

## Factory bank names and order

`ArchetPatch::factory_presets()` is the bank, and its ORDER is the wire
format: a host stores a preset as an index into it. Appending is safe;
inserting, reordering or renaming is not.
