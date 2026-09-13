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
- A bowed note reaches its level in 0.3 to 1 s where a recorded one is
  established within 250 ms and Guettler's clean attacks form in tens of
  ms (2026-09-13). The bow now starts as a player's does, the force there
  first and the speed ramping over the attack setting, which makes the
  setting act; but the modal string with its one-point hyperbolic friction
  converges to Helmholtz motion at its own pace whatever the acceleration.
  Excluded by measurement: the slip noise, the slow force drift and the
  sympathetic strings (each silenced, the onset unchanged). The cause is
  the same missing mechanism as the brightness above.
- The bowed string has one transverse plane; the plucked one has two.
- The vibrato's onset was measured on four recorded held notes of one
  player (2026-09-13): the first cycle is there within the first period
  at about two thirds of the steady extent, and the extent is full within
  two cycles. The model follows that; a published figure for the violin
  was not found (the literature measures rate and extent, and the onset
  of singers).

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

- ATTACK and the vibrato controls do nothing to a plucked note: a finger's
  release has no ramp the model shapes, and the plucked string is not
  bent. POSITION, DAMPING and DYNAMICS act on both articulations; every
  other control acts on the articulation it is drawn for, and a diagnostic
  (`engine::profile::param_audit`) reads them all at both ends.

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

- A section is as loud as its players' incoherent sum, and the largest
  sections would peak past full scale; the engine's own fader holds them
  at it, so past full scale more players add density and no level. The
  fader comes back at 6 dB per second: a soft passage right after a loud
  one starts held down. Inter-sample overshoot is the preset ceiling's.
- The largest section, thirty-two voices holding a chord, costs about a
  third of one core at 48 kHz in a release build (2026-09-14), the modal
  string now the larger half of a voice; three such instances do not fit
  on one core. The editor's load figure is the worst block of the recent
  past.

- On Linux the plugin pins its own library in memory at first
  instantiation (RTLD_NODELETE), because the wrapper installs a global
  logger and a panic hook that cannot be removed. A crash after unloading
  had been seen in one host and could not be reproduced with a bare loader
  on 2026-09-13; the pin is a defence, not a proven fix.
