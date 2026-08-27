# What must not change

Everything below is a wire format. It is written into files other programs read,
so it is not a naming choice and not refactorable. Each item needs a test; the
tests exist because a comment alone loses to a rename.

## Identifiers a host resolves the plugin by

    VST3 class id   VxArchetBow00001            (16 ASCII bytes, exactly)
    CLAP id         com.aethon-audio.archet
    Plugin NAME     Aethon Archet
    Plugin VENDOR   Aethon Audio

Lifted from the aethon monolith: yes.

The class id goes into every exported DAWproject `Vst3Plugin` device and into
the header of every `.vstpreset`. NAME and VENDOR compose the preset directory
Cubase's MediaBay indexes. Change any of them and existing projects and preset
banks point at a plugin no host can find.

aethon holds a duplicate of the class id in `interop::dawproject::devices`.
Both sides assert against the literal, so a drift still fails a build, just two
builds instead of one.

Test: `archet-plugin`, `frozen_identifiers`.

## Parameter ids and persistence keys

TODO: list every `#[id = "..."]` and `#[persist = "..."]` moved over from the
monolith. A host stores automation against the id string.

## Patch serde field names and defaults

TODO: the patch is persisted as a blob inside the host project. Field names are
the wire format; `#[serde(default)]` plus a Default impl is what lets an old
project open in a new build.

## Factory bank names and order

TODO: a host stores a preset as an index into the bank. Appending is safe;
inserting, reordering or renaming is not.
