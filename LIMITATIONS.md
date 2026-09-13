# What does not work

A physical model is a claim, and a claim can be checked. What follows was
found by measuring the model against anechoic recordings, and the
measurement lives in the source as an ignored test under `engine::profile`
or `body::tests`, so a reader can take it again. Nothing here is hidden
behind a control.

Nothing here is a roadmap. Each item says whether it needs a mechanism or
a measurement, and what has been tried.

## Bowed

- A held note does not brighten with force as a real one does. Measured
  2026-09-13 on a stopped B flat: spectral centroid 573 / 584 / 590 Hz at
  pp / mf / ff against 618 / 762 / 1095 Hz recorded (Iowa). The
  hyperbolic one-point friction of the modal string (Demoucron) does not
  sharpen the Helmholtz corner with force. Tried and reverted, each
  measured: bow force times 3.8 (+15 % centroid), the friction curve's
  velocity scale, bowing nearer the bridge for louder notes (Askenfelt;
  darker in this model, and a two-second onset because Schelleng's minimum
  force rises as one over beta squared), and the measured plucked losses
  as the bowed losses (darker still). The path is a friction model that
  sharpens the corner (Smith and Woodhouse 2000; Woodhouse 2003), not a
  force-dependent filter.
- The bowed string has one transverse plane; the plucked one has two.
- The vibrato fades in over a fixed 0.4 s after a fixed delay; the onset of
  a real vibrato was not measured.

## Plucked

- The attack carries no finger contact and no body knock. The recordings'
  attacks are strong at 1.6-2 kHz for 50 ms where their sustained part is
  weak; the model has no source for that, so the cello and bass attacks
  are 15-24 dB short above 2 kHz and the violin A string's attack centroid
  reads 609 Hz against 1048 recorded (2026-09-13). Fitting the body's bank
  to the attack instead makes the sustained highs 10-17 dB too strong; the
  sustained fit is the body's and stays.
- The two transverse planes share one loss table; the recordings show
  two-slope decays that a per-plane table would need.
- Velocity sets the plucked displacement over about 9 dB; harp
  measurements (Chadefaux) give 23 dB. Widening it is a choice not yet
  made.
- The bass strings E, A and G keep an uncalibrated attack corner: the
  calibration ran below the fundamental there, and clamping it made the
  attack worse; the body carries the residue.

## Bodies

- The violin body fits its recording at 7.0 dB rms over the sustained
  third-octave envelope and 5.5 dB over the attack (2026-09-13), which
  leaves Duennwald's quality bands a little outside their published
  ranges (anti-nasality +1.3 dB for +4..+6, clarity +6.8 dB for +10): that
  is this recording's balance, and the two references were not
  reconciled by force.
- The viola's recordings are noise-gated, so every viola decay figure
  extrapolates a short pre-gate window; the cello's and bass's noise floor
  sits some 30 dB under the notes.

## Plugin

- On Linux the plugin pins its own library in memory at first
  instantiation (RTLD_NODELETE), because the wrapper installs a global
  logger and a panic hook that cannot be removed. A crash after unloading
  had been seen in one host and could not be reproduced with a bare loader
  on 2026-09-13; the pin is a defence, not a proven fix.
