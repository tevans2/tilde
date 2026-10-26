//! Key bindings: `[keys.go]` and `[keys.select]` in config.toml.
//!
//! Each action has default keys. Setting an action replaces its keys (`move = "M"`, or a
//! list); `[]` unbinds it. A key the user gives one action is taken away from any action
//! that only had it by default. Problems become warnings and the rest still applies.
//!
//! Keys are written as the character they type (`j`, `G`, `@`) or a name (`down`,
//! `return`, `esc`, `tab`, `space`, `backspace`), with `ctrl+`, `alt+` and (for names)
//! `shift+` in front. ⌘ shortcuts belong to the menus and can't be bound here.

use std::collections::BTreeMap;

use serde::Deserialize;

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct KeysConfig {
    pub go: BTreeMap<String, KeyList>,
    pub select: BTreeMap<String, KeyList>,
}

/// One key or several.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum KeyList {
    One(String),
    Many(Vec<String>),
}

impl KeyList {
    fn keys(&self) -> Vec<&str> {
        match self {
            KeyList::One(k) => vec![k.as_str()],
            KeyList::Many(v) => v.iter().map(String::as_str).collect(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyMode {
    /// Typing in the bar: plain characters are text, so bindings need a modifier or a
    /// named key.
    Go,
    Select,
}

/// An action that can be bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub defaults: &'static [&'static str],
}

const fn a(name: &'static str, description: &'static str, defaults: &'static [&'static str]) -> ActionSpec {
    ActionSpec { name, description, defaults }
}

pub const GO_ACTIONS: &[ActionSpec] = &[
    a("down", "Next result", &["down", "ctrl+j"]),
    a("up", "Previous result", &["up", "ctrl+k"]),
    a("parent", "Up to the parent folder (⌫ on an empty bar also does)", &["ctrl+h"]),
    a("enter", "Into the highlighted folder, or reveal a result in its folder", &["tab", "ctrl+l"]),
    a("open", "Open the highlighted item", &["return"]),
    a("back", "Back to where you were (jump list)", &["ctrl+o"]),
    a("forward", "Forward again (jump list)", &["ctrl+i"]),
    a("select_mode", "Switch to SELECT", &["esc"]),
];

pub const SELECT_ACTIONS: &[ActionSpec] = &[
    a("down", "Next item", &["j", "down", "ctrl+j"]),
    a("up", "Previous item", &["k", "up", "ctrl+k"]),
    a("parent", "Up to the parent folder", &["h", "left", "backspace", "ctrl+h"]),
    a("enter", "Into a folder, or open a file", &["l", "right", "ctrl+l"]),
    a("open", "Open the highlighted item", &["return"]),
    a("back", "Back to where you were (jump list)", &["ctrl+o"]),
    a("forward", "Forward again (jump list)", &["ctrl+i"]),
    a("top", "First item", &["g"]),
    a("bottom", "Last item", &["G"]),
    a("search", "Back to GO (search)", &["/"]),
    a("activity", "Activity view", &["@"]),
    a("home", "Home folder", &["~"]),
    a("clear", "Clear the search, then the shelf", &["esc"]),
    a("shelf", "Put on or take off the shelf", &["x", "space"]),
    a("shelf_all", "Shelf everything shown", &["a"]),
    a("move", "Move to a folder", &["m"]),
    a("copy", "Copy to a folder", &["c"]),
    a("trash", "Move to the Trash", &["d"]),
    a("tag", "Edit tags", &["t"]),
    a("rename", "Rename", &["r"]),
    a("edit", "Edit the folder as text", &["e"]),
    a("new_item", "New item below (as text)", &["o"]),
    a("new_folder", "New folder", &["n"]),
    a("compress", "Compress to zip", &["z"]),
    a("copy_path", "Copy the path", &["y"]),
    a("sort", "Sort the folder (again to reverse)", &["s"]),
    a("quick_look", "Full preview in the pane; again for the Quick Look window", &["p"]),
    a("open_with", "Open with another app", &["w"]),
    a("hide", "Leave out of search", &["X"]),
    a("undo", "Undo", &["u"]),
    a("redo", "Redo", &["U", "ctrl+r"]),
    a("history", "History: undo or redo any operation", &["H"]),
    a("filter_all", "Activity: everything", &["1"]),
    a("filter_downloads", "Activity: downloads", &["2"]),
    a("filter_screenshots", "Activity: screenshots", &["3"]),
    a("filter_edited", "Activity: edited", &["4"]),
    a("filter_airdrop", "Activity: AirDrop", &["5"]),
    a("inbox", "Activity: inbox only", &["i"]),
];

pub fn actions(mode: KeyMode) -> &'static [ActionSpec] {
    match mode {
        KeyMode::Go => GO_ACTIONS,
        KeyMode::Select => SELECT_ACTIONS,
    }
}

/// A key with its modifiers. `key` is a single character (case matters: `G` is shift+g)
/// or one of `NAMED_KEYS`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Chord {
    pub key: String,
    pub ctrl: bool,
    pub alt: bool,
    /// Only with named keys; characters carry shift in the character itself.
    pub shift: bool,
}

pub const NAMED_KEYS: &[&str] = &[
    "down",
    "up",
    "left",
    "right",
    "return",
    "tab",
    "esc",
    "space",
    "backspace",
    "delete",
    "home",
    "end",
    "pageup",
    "pagedown",
];

impl Chord {
    /// How the key reads in the UI: `⌃J`, `⇧⇥`, `G`.
    pub fn display(&self) -> String {
        let mut s = String::new();
        if self.ctrl {
            s.push('⌃');
        }
        if self.alt {
            s.push('⌥');
        }
        if self.shift {
            s.push('⇧');
        }
        let key = match self.key.as_str() {
            "down" => "↓",
            "up" => "↑",
            "left" => "←",
            "right" => "→",
            "return" => "⏎",
            "tab" => "⇥",
            "esc" => "esc",
            "space" => "space",
            "backspace" => "⌫",
            "delete" => "⌦",
            "home" => "↖",
            "end" => "↘",
            "pageup" => "⇞",
            "pagedown" => "⇟",
            k if self.ctrl || self.alt => return format!("{s}{}", k.to_uppercase()),
            k => k,
        };
        s + key
    }

    fn is_char(&self) -> bool {
        self.key.chars().count() == 1
    }
}

/// Parses `ctrl+j`, `G`, `shift+tab`, `+`.
pub fn parse_chord(spec: &str) -> Result<Chord, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err("empty key".into());
    }
    // The key is whatever follows the last `+`, so `ctrl++` and `+` mean the plus key.
    let (mods, key) = match spec.strip_suffix('+') {
        Some(rest) if rest.is_empty() || rest.ends_with('+') => (rest.trim_end_matches('+'), "+"),
        _ => match spec.rsplit_once('+') {
            Some((m, k)) => (m, k),
            None => ("", spec),
        },
    };
    let mut chord = Chord { key: String::new(), ctrl: false, alt: false, shift: false };
    for m in mods.split('+').filter(|m| !m.is_empty()) {
        match m.to_lowercase().as_str() {
            "ctrl" | "control" => chord.ctrl = true,
            "alt" | "opt" | "option" => chord.alt = true,
            "shift" => chord.shift = true,
            "cmd" | "command" => return Err(format!("“{spec}”: ⌘ shortcuts belong to the menus")),
            other => return Err(format!("“{spec}”: unknown modifier “{other}”")),
        }
    }
    let lower = key.to_lowercase();
    let named = match lower.as_str() {
        "enter" => Some("return"),
        "escape" => Some("esc"),
        "arrowdown" => Some("down"),
        "arrowup" => Some("up"),
        "arrowleft" => Some("left"),
        "arrowright" => Some("right"),
        n => NAMED_KEYS.iter().find(|k| **k == n).copied(),
    };
    if let Some(name) = named.filter(|_| key.chars().count() > 1) {
        chord.key = name.to_string();
        return Ok(chord);
    }
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => {
            if chord.shift {
                if !c.is_alphabetic() {
                    return Err(format!("“{spec}”: write the shifted character itself, like “?”"));
                }
                chord.shift = false;
                chord.key = c.to_uppercase().to_string();
            } else if chord.ctrl && c.is_alphabetic() {
                // ⌃J and ⌃j are the same chord.
                chord.key = c.to_lowercase().to_string();
            } else {
                chord.key = c.to_string();
            }
            Ok(chord)
        }
        _ => Err(format!("“{spec}”: unknown key “{key}”")),
    }
}

/// A global hotkey (works while other apps are in front), like `ctrl+alt+space`. Unlike
/// in-app keys it may use `cmd`, and needs at least one of ctrl, alt or cmd.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hotkey {
    /// One character, or a name from `NAMED_KEYS`.
    pub key: String,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub cmd: bool,
}

impl Hotkey {
    /// `⌃⌥Space`.
    pub fn display(&self) -> String {
        let mut s = String::new();
        for (on, sym) in [(self.ctrl, '⌃'), (self.alt, '⌥'), (self.shift, '⇧'), (self.cmd, '⌘')] {
            if on {
                s.push(sym);
            }
        }
        let chord = Chord { key: self.key.clone(), ctrl: false, alt: false, shift: false };
        let key = if self.key == "space" { "Space".to_string() } else { chord.display().to_uppercase() };
        s + &key
    }
}

pub fn parse_hotkey(spec: &str) -> Result<Hotkey, String> {
    let mut rest = Vec::new();
    let mut cmd = false;
    for part in spec.trim().split('+') {
        if matches!(part.trim().to_lowercase().as_str(), "cmd" | "command") {
            cmd = true;
        } else {
            rest.push(part);
        }
    }
    let chord = parse_chord(&rest.join("+"))?;
    // parse_chord turns shift+letter into an uppercase letter; a hotkey keeps the letter.
    let (key, shift) = match chord.key.chars().next() {
        Some(c) if chord.key.chars().count() == 1 && c.is_uppercase() => (c.to_lowercase().to_string(), true),
        _ => (chord.key, chord.shift),
    };
    let hotkey = Hotkey { key, ctrl: chord.ctrl, alt: chord.alt, shift, cmd };
    if !(hotkey.ctrl || hotkey.alt || hotkey.cmd) {
        return Err(format!("“{spec}” needs ctrl, alt or cmd, or it would take the key from every app"));
    }
    Ok(hotkey)
}

/// The resolved bindings for one mode, in the order of `actions(mode)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub action: &'static ActionSpec,
    pub keys: Vec<Chord>,
}

/// Bindings for both modes, plus anything in the config that couldn't be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keymap {
    pub go: Vec<Binding>,
    pub select: Vec<Binding>,
    pub warnings: Vec<String>,
}

impl KeysConfig {
    pub fn resolve(&self) -> Keymap {
        let mut warnings = Vec::new();
        let go = resolve_mode(KeyMode::Go, &self.go, &mut warnings);
        let select = resolve_mode(KeyMode::Select, &self.select, &mut warnings);
        Keymap { go, select, warnings }
    }
}

fn resolve_mode(mode: KeyMode, user: &BTreeMap<String, KeyList>, warnings: &mut Vec<String>) -> Vec<Binding> {
    let section = match mode {
        KeyMode::Go => "keys.go",
        KeyMode::Select => "keys.select",
    };
    let specs = actions(mode);
    for name in user.keys() {
        if !specs.iter().any(|s| s.name == name) {
            warnings.push(format!("{section}: unknown action “{name}”"));
        }
    }
    // The user's keys first, so they win over defaults.
    let mut claimed: BTreeMap<Chord, &str> = BTreeMap::new();
    let mut chosen: BTreeMap<&str, Vec<Chord>> = BTreeMap::new();
    for spec in specs {
        let Some(list) = user.get(spec.name) else { continue };
        let mut keys = Vec::new();
        for k in list.keys() {
            match parse_chord(k) {
                Ok(chord) if mode == KeyMode::Go && chord.is_char() && !chord.ctrl && !chord.alt => {
                    warnings.push(format!("{section}.{}: “{k}” would stop you typing it; add ctrl+ or alt+", spec.name))
                }
                Ok(chord) => match claimed.get(&chord) {
                    Some(other) => warnings.push(format!("{section}.{}: “{k}” is already bound to {other}", spec.name)),
                    None => {
                        claimed.insert(chord.clone(), spec.name);
                        keys.push(chord);
                    }
                },
                Err(e) => warnings.push(format!("{section}.{}: {e}", spec.name)),
            }
        }
        chosen.insert(spec.name, keys);
    }
    specs
        .iter()
        .map(|spec| {
            let keys = match chosen.remove(spec.name) {
                Some(keys) => keys,
                None => spec
                    .defaults
                    .iter()
                    .map(|k| parse_chord(k).expect("default keys parse"))
                    .filter(|c| !claimed.contains_key(c))
                    .collect(),
            };
            Binding { action: spec, keys }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(key: &str, ctrl: bool) -> Chord {
        Chord { key: key.into(), ctrl, alt: false, shift: false }
    }

    fn keys_of(map: &[Binding], action: &str) -> Vec<String> {
        map.iter().find(|b| b.action.name == action).unwrap().keys.iter().map(Chord::display).collect()
    }

    #[test]
    fn parses_keys() {
        assert_eq!(parse_chord("j").unwrap(), chord("j", false));
        assert_eq!(parse_chord("ctrl+J").unwrap(), chord("j", true));
        assert_eq!(parse_chord("shift+g").unwrap(), chord("G", false));
        assert_eq!(parse_chord("Enter").unwrap(), chord("return", false));
        assert_eq!(parse_chord("+").unwrap(), chord("+", false));
        assert_eq!(parse_chord("ctrl++").unwrap(), chord("+", true));
        let st = parse_chord("shift+tab").unwrap();
        assert!(st.shift && st.key == "tab");
        assert_eq!(st.display(), "⇧⇥");
        assert_eq!(parse_chord("ctrl+r").unwrap().display(), "⌃R");
        assert!(parse_chord("cmd+k").unwrap_err().contains("menus"));
        assert!(parse_chord("hyper+k").is_err());
        assert!(parse_chord("shift+1").is_err());
        assert!(parse_chord("bogus").is_err());
    }

    #[test]
    fn parses_hotkeys() {
        let h = parse_hotkey("ctrl+alt+space").unwrap();
        assert_eq!((h.key.as_str(), h.ctrl, h.alt, h.cmd), ("space", true, true, false));
        assert_eq!(h.display(), "⌃⌥Space");
        let h = parse_hotkey("cmd+shift+G").unwrap();
        assert_eq!((h.key.as_str(), h.shift, h.cmd), ("g", true, true));
        assert_eq!(h.display(), "⇧⌘G");
        assert!(parse_hotkey("space").unwrap_err().contains("needs"));
        assert!(parse_hotkey("shift+k").is_err());
    }

    #[test]
    fn defaults_are_valid_and_never_collide() {
        let map = KeysConfig::default().resolve();
        assert!(map.warnings.is_empty(), "{:?}", map.warnings);
        for bindings in [&map.go, &map.select] {
            let mut seen = std::collections::HashSet::new();
            for b in bindings {
                assert!(!b.keys.is_empty(), "{} has keys", b.action.name);
                for k in &b.keys {
                    assert!(seen.insert(k.clone()), "{k:?} bound twice");
                }
            }
        }
        assert_eq!(keys_of(&map.select, "redo"), ["U", "⌃R"]);
    }

    #[test]
    fn user_keys_replace_defaults_and_take_keys_from_them() {
        let config: KeysConfig = toml::from_str(
            r#"
            [select]
            move = "M"
            copy = ["c", "C"]
            trash = []
            down = "d"
            [go]
            down = ["ctrl+n", "j"]
            "#,
        )
        .unwrap();
        let map = config.resolve();
        assert_eq!(keys_of(&map.select, "move"), ["M"]);
        assert_eq!(keys_of(&map.select, "copy"), ["c", "C"]);
        assert!(keys_of(&map.select, "trash").is_empty());
        assert_eq!(keys_of(&map.select, "down"), ["d"]);
        assert_eq!(keys_of(&map.go, "down"), ["⌃N"]);
        assert_eq!(map.warnings.len(), 1, "{:?}", map.warnings);
        assert!(map.warnings[0].contains("stop you typing"));

        // A user key takes the key away from another action's defaults.
        let config: KeysConfig = toml::from_str("[select]\ntag = \"x\"\n").unwrap();
        let map = config.resolve();
        assert_eq!(keys_of(&map.select, "tag"), ["x"]);
        assert_eq!(keys_of(&map.select, "shelf"), ["space"]);
    }

    #[test]
    fn problems_are_warnings() {
        let config: KeysConfig =
            toml::from_str("[select]\nfly = \"f\"\nmove = [\"cmd+m\", \"m\"]\ncopy = \"m\"\n").unwrap();
        let map = config.resolve();
        assert_eq!(keys_of(&map.select, "move"), ["m"]);
        assert!(keys_of(&map.select, "copy").is_empty());
        let w = map.warnings.join("\n");
        assert!(w.contains("unknown action “fly”"), "{w}");
        assert!(w.contains("menus"), "{w}");
        assert!(w.contains("already bound to move"), "{w}");
    }
}
