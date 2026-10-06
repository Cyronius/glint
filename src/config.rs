//! Settings that survive a restart: `%APPDATA%\glint\config.json`.
//!
//! The reader and writer are hand-written. The file holds two integers, and a
//! serialization crate would add dependencies and binary size for no gain.
//! The reader is deliberately forgiving: a damaged file falls back to the
//! defaults rather than failing to start.

use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Config {
    /// The window position, in screen pixels. `None` means "place it above
    /// the tray icon".
    pub position: Option<(i32, i32)>,
}

/// `%APPDATA%\glint\config.json`, or `None` when `APPDATA` is unset.
pub fn path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    let mut path = PathBuf::from(appdata);
    path.push("Glint");
    path.push("config.json");
    Some(path)
}

impl Config {
    pub fn load() -> Self {
        let Some(path) = path() else {
            return Self::default();
        };
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Self::default();
        };
        Self::parse(&text)
    }

    /// Read the known keys out of a flat JSON object.
    ///
    /// Anything unknown, missing or damaged keeps its default. The parse never
    /// fails, so a bad file cannot stop the app from starting.
    ///
    /// Traces: GLINT-CONFIG-PARSE (canonical spec: specs/glint/spec.md)
    pub fn parse(text: &str) -> Self {
        let position = match (read_int(text, "x"), read_int(text, "y")) {
            (Some(x), Some(y)) => Some((x, y)),
            _ => None,
        };
        Self { position }
    }

    pub fn save(&self) {
        let Some(path) = path() else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let text = match self.position {
            Some((x, y)) => format!("{{\n  \"x\": {x},\n  \"y\": {y}\n}}\n"),
            None => "{}\n".to_owned(),
        };
        let _ = std::fs::write(&path, text);
    }
}

/// Find `"key"` and return the text that follows its colon.
fn value_after<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    let quoted = format!("\"{key}\"");
    let at = text.find(&quoted)?;
    let rest = &text[at + quoted.len()..];
    let colon = rest.find(':')?;
    Some(rest[colon + 1..].trim_start())
}

fn read_int(text: &str, key: &str) -> Option<i32> {
    let value = value_after(text, key)?;
    let end = value
        .char_indices()
        .position(|(index, c)| !(c.is_ascii_digit() || (index == 0 && c == '-')))
        .unwrap_or(value.len());
    value[..end].parse().ok()
}
