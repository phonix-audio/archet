//! Writes the plugin state that loads a factory preset, with overrides,
//! for a project file a host opens.
//!
//! ```text
//! archet-state --list
//! archet-state NAME [--patch JSON] [--vst3] [--json] [-o FILE]
//! ```
//!
//! `NAME` is a preset of the factory bank, matched without case. `--patch`
//! is a JSON object laid over the preset's patch by its serde field names;
//! a dotted key (`fx.slots`) reaches a nested field. A field the patch does
//! not have is refused. The state is the stream a CLAP host hands the
//! plugin, or with `--vst3` a VST3 host's. It goes to `FILE` or to stdout,
//! as bytes or, with `--json`, as a JSON array of bytes.

use std::io::Write as _;
use std::process::ExitCode;

use archet::patch::ArchetPatch;
use archet_plugin::state::{clap_stream, state_of};
use phonix_plugin::preset::Preset;
use serde_json::{Map, Value};

const USAGE: &str = "usage: archet-state --list | archet-state NAME [--patch JSON] [--vst3] [--json] [-o FILE]";

/// The field under which serde checks the keys itself: the chain skips
/// empty fields when it writes, so a key it omitted is still a key.
const OPEN_FIELD: &str = "fx";

/// `over` laid onto `into` at `path`: objects merge by key, anything else
/// replaces. A key `into` does not hold is refused outside the chain.
fn merge(into: &mut Value, over: &Value, path: &str, strict: bool) -> Result<(), String> {
    match (into, over) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in b {
                let at = if path.is_empty() { k.clone() } else { format!("{path}.{k}") };
                if strict && !a.contains_key(k) {
                    return Err(format!("the patch has no field {at}"));
                }
                let strict = strict && !(path.is_empty() && k == OPEN_FIELD);
                merge(a.entry(k.clone()).or_insert(Value::Null), v, &at, strict)?;
            }
            Ok(())
        }
        (a, b) => {
            *a = b.clone();
            Ok(())
        }
    }
}

/// `over` with every dotted key opened into nested objects.
fn nest(over: &Map<String, Value>) -> Value {
    let mut out = Value::Object(Map::new());
    for (k, v) in over {
        let mut at = &mut out;
        let mut parts = k.split('.').peekable();
        while let Some(p) = parts.next() {
            if !at.is_object() {
                *at = Value::Object(Map::new());
            }
            let Some(m) = at.as_object_mut() else { break };
            if parts.peek().is_none() {
                m.insert(p.to_owned(), v.clone());
                break;
            }
            at = m.entry(p.to_owned()).or_insert_with(|| Value::Object(Map::new()));
        }
    }
    out
}

/// `base` with `over` laid onto it, read back through the patch's serde.
fn apply(base: &ArchetPatch, over: &Map<String, Value>) -> Result<ArchetPatch, String> {
    let mut value = serde_json::to_value(base).map_err(|e| e.to_string())?;
    merge(&mut value, &nest(over), "", true)?;
    serde_json::from_value(value).map_err(|e| format!("the patch does not read: {e}"))
}

/// An argument's JSON object.
fn object(flag: &str, arg: Option<String>) -> Result<Map<String, Value>, String> {
    let text = arg.ok_or_else(|| format!("{flag} takes a JSON object"))?;
    match serde_json::from_str(&text) {
        Ok(Value::Object(m)) => Ok(m),
        _ => Err(format!("{flag} takes a JSON object, not {text}")),
    }
}

/// The bank's names close to `name`: those holding it, else those sharing
/// one of its words.
fn near<'a>(bank: &'a [ArchetPatch], name: &str) -> Vec<&'a str> {
    let low = name.to_lowercase();
    let names = || bank.iter().map(|p| p.name.as_str());
    let holding: Vec<&str> = names().filter(|n| n.to_lowercase().contains(&low)).take(8).collect();
    if !holding.is_empty() {
        return holding;
    }
    let words: Vec<&str> = low.split_whitespace().filter(|w| w.len() >= 3).collect();
    names().filter(|n| words.iter().any(|w| n.to_lowercase().contains(w))).take(8).collect()
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let (mut name, mut over, mut json, mut out, mut list, mut vst3) = (None, Map::new(), false, None, false, false);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--list" => list = true,
            "--patch" => over = object("--patch", args.next())?,
            "--json" => json = true,
            "--vst3" => vst3 = true,
            "-o" => out = Some(args.next().ok_or("-o takes a file")?),
            "-h" | "--help" => return Err(USAGE.into()),
            _ if name.is_none() && !a.starts_with('-') => name = Some(a),
            _ => return Err(format!("unknown argument {a}\n{USAGE}")),
        }
    }
    let bank = ArchetPatch::factory_presets();
    if list {
        let mut o = std::io::stdout().lock();
        for p in &bank {
            // A reader that closed the pipe has read all it wants.
            if writeln!(o, "{}\t{}", p.preset_category().unwrap_or_default(), p.name).is_err() {
                break;
            }
        }
        return Ok(());
    }
    let name = name.ok_or(USAGE)?;
    let Some(base) = bank.iter().find(|p| p.name.eq_ignore_ascii_case(&name)) else {
        let near = near(&bank, &name);
        return Err(if near.is_empty() {
            format!("no preset {name}; --list names them")
        } else {
            format!("no preset {name}; near: {}", near.join(", "))
        });
    };
    let patch = apply(base, &over)?;
    let bytes = if vst3 { state_of(&patch) } else { clap_stream(&state_of(&patch)) };
    let bytes = if json { serde_json::to_vec(&bytes).map_err(|e| e.to_string())? } else { bytes };
    match out {
        Some(p) => std::fs::write(&p, &bytes).map_err(|e| format!("{p}: {e}")),
        None => std::io::stdout().write_all(&bytes).map_err(|e| e.to_string()),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("archet-state: {e}");
            ExitCode::FAILURE
        }
    }
}
