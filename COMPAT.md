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

## The chain a patch carries

    slots   equaliser, space, ceiling

Every factory preset carries its own settings for those three, inside its
patch, as the field `fx`: the spaces differ with what plays (a soloist in
a chamber, a section in a hall, a full string body in a concert hall,
plucked strings in a room). What no preset and no user can change is WHICH
effects run and in what order. That is what curated means here, and it is
why the order is listed above.

The field's serde default is an EMPTY chain, and an empty chain is a real
no-op: a project saved before the field existed keeps sounding as it did.
`ArchetPatch::default()` carries the chamber, so a fresh instance plays
what its window says; `Init` (preset zero) runs no chain at all.

The chain is a `phonix_fx::ChainSpec`: every slot names its kind and its
parameters by string id (`"parametric-eq"`, `"reverb"`, `"type": "hall"`,
`"brickwall-limiter"`, `"ceiling"`), and the slot's mix is the slot's.
Those ids are the wire format. The ranges and units of every parameter
live with the effect in `phonix_fx::effects`.

Tests: `archet`, `fx::tests` (the recipe names only what the build has,
the order and the kinds are frozen, every factory preset carries its own
chain, the default patch is a soloist in a chamber, a patch written before
the chain still loads).

## Factory bank names and order

`ArchetPatch::factory_presets()` is the bank, and its ORDER is the wire
format: a host stores a preset as an index into it. Appending is safe;
inserting, reordering or renaming is not.
