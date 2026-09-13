# Archet

**EXPERIMENTAL.** Under development and not yet judged in use: the sound, the
factory bank and the editor can change from one version to the next. What
cannot change is listed in `COMPAT.md`. What is known to be missing is listed
below.

A bowed string built from the acoustics literature and calibrated on anechoic
recordings, not from samples. The string is modal (Demoucron), driven by a
one-point friction interaction that solves the stick-slip of the bow; the
bridge force runs into a body of measured signature modes over a statistical
bank laid out at a measured modal overlap. Four bodies, violin, viola, cello
and double bass, each with its own measured modes; sympathetic open strings
behind them; a pizzicato that releases the same string from a finger and lets
it ring with the per-partial decays measured on each open string, then mutes
it with the hand; and a section mode that renders a decorrelated desk of
players rather than phase-locked clones. `NOTICE` lists the sources.

## Layout

    crates/archet            the engine. serde is its only dependency.
    crates/archet-ui         the egui editor
    crates/archet-plugin     VST3 / CLAP, via nice-plug
    vendor/phonix-sdk        the shared phonix crates, vendored

The editor is a separate crate rather than a feature of the engine, and that
is load-bearing. Cargo unifies features across a resolved graph, so an
optional `egui` inside `archet` would be switched on for the engine's own
tests by any `cargo test --workspace`. A crate boundary is the only thing
that makes "the engine never sees egui" true rather than merely intended:

    cargo tree -p archet --edges normal

## Building

    cargo test
    scripts/build_plugins.sh        # VST3 + CLAP bundle for this platform
    scripts/build_plugins.sh --windows

The bundle lands in `target/bundled/<platform>/`, laid out as the VST3 spec
wants. Any host that scans a directory will find it there; for a development
tree, point the host's search path at it rather than installing.

## Measuring

The engine carries its measurements as ignored tests under
`engine::profile` and `body::tests`: held bowed notes, plucked open strings,
scored passages read from files, the bodies' impulse responses. They write
to `/tmp` and print what a recording would be compared against.

    cargo test --lib engine::profile::bow_held -- --ignored --nocapture
    cargo test --lib engine::profile::pizz_family -- --ignored --nocapture
    ARCHET_SCORE=<dir> cargo test --lib engine::profile::score_render -- --ignored --nocapture

Three tests pin the rendered audio bit for bit (the engine sweep, the bowed
section, the body); a change that moves the sound moves a pin, and the commit
that moves it says so.

## Known limits

Measured against the recordings, and left open because each needs a
mechanism rather than a figure:

- A bowed note does not brighten with force as a real one does (centroid
  at ff about 600 Hz against 1100 recorded): the hyperbolic one-point
  friction does not sharpen the Helmholtz corner. The path is a friction
  model that does (Smith and Woodhouse 2000; Woodhouse 2003).
- A plucked attack carries no finger contact or body knock, so the low
  strings' attacks are 15-24 dB short above 2 kHz on the cello and bass,
  and the violin A string's attack is duller than recorded.
- The two transverse planes of a plucked string share one loss table; the
  bowed string has one plane.
- Velocity changes the plucked displacement over a narrower range than a
  harp player's measured one.

## Compatibility

`COMPAT.md` lists what cannot change: the VST3 class id, the parameter ids,
the patch's serde field names and their defaults, and the factory bank's
names *and order*.

## Licence

MIT, see `LICENSE`. Attributions and the licences of dependencies are in
`NOTICE`.
