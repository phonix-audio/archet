//! Color palette for Phonix — Design system tokens from doc/GUI.md

use egui::Color32;

// ── Core design tokens (doc/GUI.md § Design System) ─────────────────

/// Main panel background — #1A1A1E
pub const BG_DARK: Color32 = Color32::from_rgb(26, 26, 30);
/// Section/module background — #26262C
pub const BG_PANEL: Color32 = Color32::from_rgb(38, 38, 44);
/// Knob body, raised elements — #2F2F38
pub const BG_RAISED: Color32 = Color32::from_rgb(47, 47, 56);
/// Section borders, dividers — #3A3A45
pub const BORDER: Color32 = Color32::from_rgb(58, 58, 69);
/// Labels, values — #E8E8EE
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(232, 232, 238);
/// Units, secondary labels — #7A7A8A
pub const TEXT_DIM: Color32 = Color32::from_rgb(122, 122, 138);
/// Active states, LED glow, focus ring — Elektron-style cyan #00C8FF
pub const ACCENT_RGB: Color32 = Color32::from_rgb(0, 200, 255);
/// Warnings, clip indicators — #FF6B35
pub const ACCENT_WARM: Color32 = Color32::from_rgb(255, 107, 53);
/// VU meter safe zone — #4ADE80
pub const ACCENT_GREEN: Color32 = Color32::from_rgb(74, 222, 128);

// ── Module accent colors (per-section identity) ─────────────────────

pub const ACCENT_OSC: Color32 = Color32::from_rgb(255, 140, 50); // Orange
pub const ACCENT_FILTER: Color32 = Color32::from_rgb(50, 180, 255); // Blue
pub const ACCENT_ENV: Color32 = Color32::from_rgb(180, 255, 100); // Green
pub const ACCENT_LFO: Color32 = Color32::from_rgb(255, 100, 200); // Pink
pub const ACCENT_FX: Color32 = Color32::from_rgb(200, 150, 255); // Purple
pub const ACCENT_SEQ: Color32 = Color32::from_rgb(100, 255, 200); // Cyan
pub const ACCENT_VOICE: Color32 = Color32::from_rgb(255, 200, 100); // Gold

// ── LED colors ──────────────────────────────────────────────────────

pub const LED_RED: Color32 = Color32::from_rgb(255, 50, 50);
pub const LED_GREEN: Color32 = Color32::from_rgb(50, 255, 100);
pub const LED_YELLOW: Color32 = Color32::from_rgb(255, 220, 50);
pub const LED_BLUE: Color32 = Color32::from_rgb(50, 150, 255);

// ── Derived / structural colors ─────────────────────────────────────

/// Highlight for sub-sections (slightly lighter than BG_RAISED)
pub const MOOG_BLUE: Color32 = Color32::from_rgb(50, 120, 180);

// ── Layer accent colors ──────────────────────────────────────────────

/// Jade green
pub const ACCENT_JADE: Color32 = Color32::from_rgb(80, 200, 140);
/// Warm rosin amber
pub const ACCENT_ROSIN: Color32 = Color32::from_rgb(214, 158, 78);
/// Chaos layer — hot orange-red
pub const ACCENT_CHAOS: Color32 = Color32::from_rgb(255, 90, 60);
/// Resonator layer — cobalt blue
pub const ACCENT_RESONATOR: Color32 = Color32::from_rgb(80, 140, 255);
/// Grain layer — warm amber
pub const ACCENT_GRAIN: Color32 = Color32::from_rgb(240, 170, 50);
/// Spectral layer — violet
pub const ACCENT_SPECTRAL: Color32 = Color32::from_rgb(180, 100, 255);

// ── Teal accent colors ───────────────────────────────────────────────

/// Teal
pub const ACCENT_TEAL: Color32 = Color32::from_rgb(0, 210, 185);
/// A voice section, in teal
pub const ACCENT_TEAL_VOICE: Color32 = Color32::from_rgb(0, 180, 220);
/// An FX section, in teal
pub const ACCENT_TEAL_FX: Color32 = Color32::from_rgb(120, 230, 200);
