//! Colours: a preset (with light and dark variants) plus the user's overrides.

use std::collections::BTreeMap;

use serde::Deserialize;

/// Every colour that can be set, in `[theme.colors]`, `[theme.light]` or `[theme.dark]`.
pub const COLOR_KEYS: &[&str] = &[
    "background",
    "bar",
    "border",
    "text",
    "text_dim",
    "selection",
    "selection_text",
    "match",
    "match_selected",
    "go",
    "select",
    "shelf",
    "badge_label",
    "badge_folder",
    "badge_app",
    "badge_image",
    "badge_video",
    "badge_audio",
    "badge_pdf",
    "badge_document",
    "badge_code",
    "badge_text",
    "badge_archive",
    "badge_other",
];

pub const PRESETS: &[&str] = &["default", "catppuccin", "gruvbox", "nord", "solarized"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Appearance {
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeConfig {
    /// `system`, `light` or `dark`.
    pub appearance: String,
    /// One of `PRESETS`; `default` uses the system's own colours.
    pub preset: String,
    /// Overrides for both appearances: colour key → `#RRGGBB` or `#RRGGBBAA`.
    pub colors: BTreeMap<String, String>,
    /// Overrides for light appearance only.
    pub light: BTreeMap<String, String>,
    /// Overrides for dark appearance only.
    pub dark: BTreeMap<String, String>,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        ThemeConfig {
            appearance: "system".into(),
            preset: "default".into(),
            colors: BTreeMap::new(),
            light: BTreeMap::new(),
            dark: BTreeMap::new(),
        }
    }
}

/// Colours as `0xRRGGBBAA` per key. A key missing from both maps means "use the system
/// colour".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTheme {
    pub appearance: Appearance,
    pub light: BTreeMap<String, u32>,
    pub dark: BTreeMap<String, u32>,
}

impl ThemeConfig {
    /// The preset with overrides applied. Problems (unknown preset, key or colour) are
    /// returned as messages and the offending setting is skipped.
    pub fn resolve(&self) -> (ResolvedTheme, Vec<String>) {
        let mut problems = Vec::new();
        let appearance = match self.appearance.to_lowercase().as_str() {
            "system" | "auto" | "" => Appearance::System,
            "light" => Appearance::Light,
            "dark" => Appearance::Dark,
            other => {
                problems.push(format!("theme.appearance “{other}” isn't system, light or dark"));
                Appearance::System
            }
        };
        let (mut light, mut dark) = match preset(&self.preset.to_lowercase()) {
            Some((l, d)) => (parse_all(l), parse_all(d)),
            None => {
                problems.push(format!("theme.preset “{}” isn't one of {}", self.preset, PRESETS.join(", ")));
                (BTreeMap::new(), BTreeMap::new())
            }
        };
        let mut apply = |table: &str, map: &BTreeMap<String, String>, targets: &mut [&mut BTreeMap<String, u32>]| {
            for (key, value) in map {
                if !COLOR_KEYS.contains(&key.as_str()) {
                    problems.push(format!("theme.{table}.{key} isn't a colour Tilde knows"));
                    continue;
                }
                match parse_hex(value) {
                    Some(c) => targets.iter_mut().for_each(|t| {
                        t.insert(key.clone(), c);
                    }),
                    None => problems.push(format!("theme.{table}.{key} = “{value}” isn't a #RRGGBB colour")),
                }
            }
        };
        apply("colors", &self.colors, &mut [&mut light, &mut dark]);
        apply("light", &self.light, &mut [&mut light]);
        apply("dark", &self.dark, &mut [&mut dark]);
        (ResolvedTheme { appearance, light, dark }, problems)
    }
}

/// `#RGB`, `#RRGGBB` or `#RRGGBBAA` (the `#` is optional) → `0xRRGGBBAA`.
pub fn parse_hex(s: &str) -> Option<u32> {
    let h = s.trim().trim_start_matches('#');
    if !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match h.len() {
        3 => {
            let v = u32::from_str_radix(h, 16).ok()?;
            let (r, g, b) = ((v >> 8) & 0xf, (v >> 4) & 0xf, v & 0xf);
            Some((r * 17) << 24 | (g * 17) << 16 | (b * 17) << 8 | 0xff)
        }
        6 => Some(u32::from_str_radix(h, 16).ok()? << 8 | 0xff),
        8 => u32::from_str_radix(h, 16).ok(),
        _ => None,
    }
}

fn parse_all(pairs: &[(&str, &str)]) -> BTreeMap<String, u32> {
    pairs.iter().filter_map(|(k, v)| Some((k.to_string(), parse_hex(v)?))).collect()
}

/// bat themes that match a preset: (light, dark).
pub fn preset_bat_themes(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name.to_lowercase().as_str() {
        "catppuccin" => ("Catppuccin Latte", "Catppuccin Mocha"),
        "gruvbox" => ("gruvbox-light", "gruvbox-dark"),
        "nord" => ("GitHub", "Nord"),
        "solarized" => ("Solarized (light)", "Solarized (dark)"),
        _ => return None,
    })
}

type Palette = &'static [(&'static str, &'static str)];

/// (light, dark) palettes. `default` is empty: system colours throughout.
fn preset(name: &str) -> Option<(Palette, Palette)> {
    Some(match name {
        "default" => (&[], &[]),
        "catppuccin" => (CATPPUCCIN_LATTE, CATPPUCCIN_MOCHA),
        "gruvbox" => (GRUVBOX_LIGHT, GRUVBOX_DARK),
        "nord" => (NORD_LIGHT, NORD_DARK),
        "solarized" => (SOLARIZED_LIGHT, SOLARIZED_DARK),
        _ => return None,
    })
}

const CATPPUCCIN_MOCHA: Palette = &[
    ("background", "#1e1e2e"),
    ("bar", "#181825"),
    ("border", "#313244"),
    ("text", "#cdd6f4"),
    ("text_dim", "#a6adc8"),
    ("selection", "#45475a"),
    ("selection_text", "#cdd6f4"),
    ("match", "#fab387"),
    ("match_selected", "#f9e2af"),
    ("go", "#89b4fa"),
    ("select", "#a6e3a1"),
    ("shelf", "#a6e3a1"),
    ("badge_label", "#1e1e2e"),
    ("badge_folder", "#89b4fa"),
    ("badge_app", "#b4befe"),
    ("badge_image", "#a6e3a1"),
    ("badge_video", "#f5c2e7"),
    ("badge_audio", "#94e2d5"),
    ("badge_pdf", "#f38ba8"),
    ("badge_document", "#89dceb"),
    ("badge_code", "#cba6f7"),
    ("badge_text", "#9399b2"),
    ("badge_archive", "#f9e2af"),
    ("badge_other", "#7f849c"),
];

const CATPPUCCIN_LATTE: Palette = &[
    ("background", "#eff1f5"),
    ("bar", "#e6e9ef"),
    ("border", "#ccd0da"),
    ("text", "#4c4f69"),
    ("text_dim", "#6c6f85"),
    ("selection", "#bcc0cc"),
    ("selection_text", "#4c4f69"),
    ("match", "#fe640b"),
    ("match_selected", "#d20f39"),
    ("go", "#1e66f5"),
    ("select", "#40a02b"),
    ("shelf", "#40a02b"),
    ("badge_label", "#eff1f5"),
    ("badge_folder", "#1e66f5"),
    ("badge_app", "#7287fd"),
    ("badge_image", "#40a02b"),
    ("badge_video", "#ea76cb"),
    ("badge_audio", "#179299"),
    ("badge_pdf", "#d20f39"),
    ("badge_document", "#04a5e5"),
    ("badge_code", "#8839ef"),
    ("badge_text", "#7c7f93"),
    ("badge_archive", "#df8e1d"),
    ("badge_other", "#9ca0b0"),
];

const GRUVBOX_DARK: Palette = &[
    ("background", "#282828"),
    ("bar", "#1d2021"),
    ("border", "#3c3836"),
    ("text", "#ebdbb2"),
    ("text_dim", "#a89984"),
    ("selection", "#504945"),
    ("selection_text", "#fbf1c7"),
    ("match", "#fe8019"),
    ("match_selected", "#fabd2f"),
    ("go", "#83a598"),
    ("select", "#b8bb26"),
    ("shelf", "#b8bb26"),
    ("badge_label", "#282828"),
    ("badge_folder", "#83a598"),
    ("badge_app", "#d3869b"),
    ("badge_image", "#b8bb26"),
    ("badge_video", "#d3869b"),
    ("badge_audio", "#8ec07c"),
    ("badge_pdf", "#fb4934"),
    ("badge_document", "#8ec07c"),
    ("badge_code", "#d3869b"),
    ("badge_text", "#a89984"),
    ("badge_archive", "#fabd2f"),
    ("badge_other", "#928374"),
];

const GRUVBOX_LIGHT: Palette = &[
    ("background", "#fbf1c7"),
    ("bar", "#f2e5bc"),
    ("border", "#d5c4a1"),
    ("text", "#3c3836"),
    ("text_dim", "#7c6f64"),
    ("selection", "#d5c4a1"),
    ("selection_text", "#282828"),
    ("match", "#af3a03"),
    ("match_selected", "#9d0006"),
    ("go", "#076678"),
    ("select", "#79740e"),
    ("shelf", "#79740e"),
    ("badge_label", "#fbf1c7"),
    ("badge_folder", "#076678"),
    ("badge_app", "#8f3f71"),
    ("badge_image", "#79740e"),
    ("badge_video", "#8f3f71"),
    ("badge_audio", "#427b58"),
    ("badge_pdf", "#9d0006"),
    ("badge_document", "#427b58"),
    ("badge_code", "#8f3f71"),
    ("badge_text", "#7c6f64"),
    ("badge_archive", "#b57614"),
    ("badge_other", "#928374"),
];

const NORD_DARK: Palette = &[
    ("background", "#2e3440"),
    ("bar", "#3b4252"),
    ("border", "#434c5e"),
    ("text", "#eceff4"),
    ("text_dim", "#a3acbb"),
    ("selection", "#4c566a"),
    ("selection_text", "#eceff4"),
    ("match", "#ebcb8b"),
    ("match_selected", "#ebcb8b"),
    ("go", "#88c0d0"),
    ("select", "#a3be8c"),
    ("shelf", "#a3be8c"),
    ("badge_label", "#2e3440"),
    ("badge_folder", "#81a1c1"),
    ("badge_app", "#b48ead"),
    ("badge_image", "#a3be8c"),
    ("badge_video", "#b48ead"),
    ("badge_audio", "#8fbcbb"),
    ("badge_pdf", "#bf616a"),
    ("badge_document", "#88c0d0"),
    ("badge_code", "#b48ead"),
    ("badge_text", "#d8dee9"),
    ("badge_archive", "#ebcb8b"),
    ("badge_other", "#a3acbb"),
];

const NORD_LIGHT: Palette = &[
    ("background", "#eceff4"),
    ("bar", "#e5e9f0"),
    ("border", "#d8dee9"),
    ("text", "#2e3440"),
    ("text_dim", "#4c566a"),
    ("selection", "#5e81ac"),
    ("selection_text", "#eceff4"),
    ("match", "#d08770"),
    ("match_selected", "#ebcb8b"),
    ("go", "#5e81ac"),
    ("select", "#a3be8c"),
    ("shelf", "#a3be8c"),
    ("badge_label", "#2e3440"),
    ("badge_folder", "#81a1c1"),
    ("badge_app", "#b48ead"),
    ("badge_image", "#a3be8c"),
    ("badge_video", "#b48ead"),
    ("badge_audio", "#8fbcbb"),
    ("badge_pdf", "#bf616a"),
    ("badge_document", "#88c0d0"),
    ("badge_code", "#b48ead"),
    ("badge_text", "#d8dee9"),
    ("badge_archive", "#ebcb8b"),
    ("badge_other", "#d8dee9"),
];

const SOLARIZED_DARK: Palette = &[
    ("background", "#002b36"),
    ("bar", "#073642"),
    ("border", "#0a4452"),
    ("text", "#93a1a1"),
    ("text_dim", "#657b83"),
    ("selection", "#268bd2"),
    ("selection_text", "#fdf6e3"),
    ("match", "#cb4b16"),
    ("match_selected", "#b58900"),
    ("go", "#268bd2"),
    ("select", "#859900"),
    ("shelf", "#859900"),
    ("badge_label", "#002b36"),
    ("badge_folder", "#268bd2"),
    ("badge_app", "#6c71c4"),
    ("badge_image", "#859900"),
    ("badge_video", "#d33682"),
    ("badge_audio", "#2aa198"),
    ("badge_pdf", "#dc322f"),
    ("badge_document", "#2aa198"),
    ("badge_code", "#6c71c4"),
    ("badge_text", "#93a1a1"),
    ("badge_archive", "#b58900"),
    ("badge_other", "#839496"),
];

const SOLARIZED_LIGHT: Palette = &[
    ("background", "#fdf6e3"),
    ("bar", "#eee8d5"),
    ("border", "#e2dbc4"),
    ("text", "#586e75"),
    ("text_dim", "#93a1a1"),
    ("selection", "#268bd2"),
    ("selection_text", "#fdf6e3"),
    ("match", "#cb4b16"),
    ("match_selected", "#b58900"),
    ("go", "#268bd2"),
    ("select", "#859900"),
    ("shelf", "#859900"),
    ("badge_label", "#fdf6e3"),
    ("badge_folder", "#268bd2"),
    ("badge_app", "#6c71c4"),
    ("badge_image", "#859900"),
    ("badge_video", "#d33682"),
    ("badge_audio", "#2aa198"),
    ("badge_pdf", "#dc322f"),
    ("badge_document", "#2aa198"),
    ("badge_code", "#6c71c4"),
    ("badge_text", "#839496"),
    ("badge_archive", "#b58900"),
    ("badge_other", "#93a1a1"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_forms() {
        assert_eq!(parse_hex("#89b4fa"), Some(0x89b4faff));
        assert_eq!(parse_hex("89B4FA80"), Some(0x89b4fa80));
        assert_eq!(parse_hex("#fff"), Some(0xffffffff));
        assert_eq!(parse_hex("#12345"), None);
        assert_eq!(parse_hex("blue"), None);
    }

    #[test]
    fn presets_are_complete() {
        for name in PRESETS.iter().filter(|p| **p != "default") {
            let (light, dark) = preset(name).unwrap();
            for palette in [light, dark] {
                let parsed = parse_all(palette);
                for key in COLOR_KEYS {
                    assert!(parsed.contains_key(*key), "{name} is missing {key}");
                }
                assert_eq!(parsed.len(), COLOR_KEYS.len(), "{name} has bad or extra entries");
            }
        }
    }

    #[test]
    fn overrides_layer_on_the_preset() {
        let cfg = ThemeConfig {
            appearance: "dark".into(),
            preset: "nord".into(),
            colors: [("match".to_string(), "#ff0000".to_string())].into(),
            dark: [("background".to_string(), "#000000".to_string())].into(),
            ..Default::default()
        };
        let (t, problems) = cfg.resolve();
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(t.appearance, Appearance::Dark);
        assert_eq!(t.dark["match"], 0xff0000ff);
        assert_eq!(t.light["match"], 0xff0000ff);
        assert_eq!(t.dark["background"], 0x000000ff);
        assert_eq!(t.light["background"], 0xeceff4ff, "light keeps the preset");
    }

    #[test]
    fn problems_are_reported_and_skipped() {
        let cfg = ThemeConfig {
            appearance: "purple".into(),
            preset: "dracula".into(),
            colors: [("backgrnd".to_string(), "#000".to_string()), ("text".to_string(), "white".to_string())].into(),
            ..Default::default()
        };
        let (t, problems) = cfg.resolve();
        assert_eq!(problems.len(), 4, "{problems:?}");
        assert_eq!(t.appearance, Appearance::System);
        assert!(t.light.is_empty() && t.dark.is_empty());
    }
}
