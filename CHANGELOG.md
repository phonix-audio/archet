# Unreleased

## Added

- archet-state: write a preset as the plugin's state, for a host or a score.
- bow: take the friction from the rosin's temperature, for a cleaner onset.
- bow: wander the bow's speed and carry the arm's tremor within a stroke.
- legato: slur overlapping keys and change bow on touching ones, live.
- legato: slur a section at Auto, a divided line from its nearest part.
- engine: play 128 voices on a pool of workers.
- envelope: stretch the attack and the release from an accent to a pad.
- equaliser: set the bands per preset kind, soloist, section or desk.
- wheels: bend the string and drive the vibrato from the wheels.
- editor: give every host parameter its knob.

## Changed

- bow: brighten a bowed note with its dynamic, as recorded notes do.
- bow: press harder within the window where the tone stays clean.
- body: fit the violin body to its recorded bowed strings.
- body: radiate less of the low strings' fundamental.
- vibrato: default to the recorded normal vibrato.
- editor: draw the case with the SDK's finish.

## Removed

- editor: take the playable keyboard out of the window.

## Fixed

- body: bring a held note's nasal band to the recordings.
- pizzicato: follow the bend and the vibrato.
- vst3: receive the wheels from a VST3 host.
- level: take the parameter's whole range from the level command.
- engine: allocate nothing in a block while notes move.
- editor: show a pizzicato's bow controls unlit.
- editor: draw the text from the first frame.
