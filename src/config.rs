//! Minimal TOML config for egs. Dependency-free on purpose.
//!
//! ```toml
//! default_install_path = "/mnt/Games/EpicGames"
//!
//! [games."Eel"]
//! install_path = "/mnt/Games/NonSteamGames/KingdomCome"
//! ```
//! Keys are stable Epic `app_name` values, never display titles.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::filesystem;

pub const DEFAULT_INSTALL_PATH: &str = "/mnt/Games/EpicGames";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameConfig {
    pub install_path: Option<String>,
    pub proton: Option<String>,
    pub gamemode: Option<bool>,
    pub mangohud: Option<bool>,
    pub lsfg: Option<String>,
    pub prefix_path: Option<String>,
    pub launch_args: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub default_install_path: String,
    pub default_prefix_path: String,
    pub default_proton: String,
    pub default_gamemode: bool,
    pub default_mangohud: bool,
    pub lsfg_path: String,
    pub lsfg_multiplier: String,
    pub games: HashMap<String, GameConfig>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            default_install_path: DEFAULT_INSTALL_PATH.to_string(),
            default_prefix_path: filesystem::prefixes_dir().to_string_lossy().to_string(),
            default_proton: "Auto".to_string(),
            default_gamemode: true,
            default_mangohud: true,
            lsfg_path: String::new(),
            lsfg_multiplier: "Disabled".to_string(),
            games: HashMap::new(),
        }
    }
}

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Parse a value after '=': supports "quoted strings", booleans, and raw words.
fn parse_val(rest: &str) -> Option<String> {
    let rest = rest.trim();
    if let Some(stripped) = rest.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = stripped.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some(o) => {
                        out.push('\\');
                        out.push(o);
                    }
                    None => return None,
                },
                '"' => return Some(out),
                _ => out.push(c),
            }
        }
        None
    } else {
        // e.g. true / false / numbers / unquoted strings
        let token = rest.split('#').next()?.trim();
        if !token.is_empty() {
            Some(token.to_string())
        } else {
            None
        }
    }
}

pub fn parse(text: &str) -> Config {
    let mut cfg = Config::default();
    let mut section: Option<String> = None;
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            let inner = line[1..line.len() - 1].trim();
            // Accept [games."ID"] and [games.ID].
            if let Some(rest) = inner.strip_prefix("games.") {
                let id = rest.trim().trim_matches('"').to_string();
                if !id.is_empty() {
                    section = Some(id);
                    continue;
                }
            }
            section = None;
            continue;
        }
        let Some(eq) = line.find('=') else { continue };
        let key = line[..eq].trim();
        let Some(val) = parse_val(&line[eq + 1..]) else {
            continue;
        };
        match (section.as_deref(), key) {
            (None, "default_install_path") if !val.is_empty() => {
                cfg.default_install_path = val;
            }
            (None, "default_prefix_path") if !val.is_empty() => {
                cfg.default_prefix_path = val;
            }
            (None, "default_proton") if !val.is_empty() => {
                cfg.default_proton = val;
            }
            (None, "default_gamemode") => {
                cfg.default_gamemode = val.parse::<bool>().unwrap_or(true);
            }
            (None, "default_mangohud") => {
                cfg.default_mangohud = val.parse::<bool>().unwrap_or(true);
            }
            (None, "lsfg_path") => {
                cfg.lsfg_path = val;
            }
            (None, "lsfg_multiplier") if !val.is_empty() => {
                cfg.lsfg_multiplier = val;
            }
            (Some(id), "install_path") => {
                cfg.games.entry(id.to_string()).or_default().install_path = Some(val);
            }
            (Some(id), "proton") => {
                cfg.games.entry(id.to_string()).or_default().proton = Some(val);
            }
            (Some(id), "gamemode") => {
                cfg.games.entry(id.to_string()).or_default().gamemode = val.parse::<bool>().ok();
            }
            (Some(id), "mangohud") => {
                cfg.games.entry(id.to_string()).or_default().mangohud = val.parse::<bool>().ok();
            }
            (Some(id), "lsfg") => {
                cfg.games.entry(id.to_string()).or_default().lsfg = Some(val);
            }
            (Some(id), "prefix_path") => {
                cfg.games.entry(id.to_string()).or_default().prefix_path = Some(val);
            }
            (Some(id), "launch_args") => {
                cfg.games.entry(id.to_string()).or_default().launch_args = Some(val);
            }
            _ => {}
        }
    }
    cfg
}

pub fn render(cfg: &Config) -> String {
    let mut out = format!(
        "# egs configuration — managed by the egs TUI (Settings).\n\
         # Keys are stable Epic app_name values, never display titles.\n\
         default_install_path = \"{}\"\n\
         default_prefix_path = \"{}\"\n\
         default_proton = \"{}\"\n\
         default_gamemode = {}\n\
         default_mangohud = {}\n\
         lsfg_path = \"{}\"\n\
         lsfg_multiplier = \"{}\"\n",
        escape(&cfg.default_install_path),
        escape(&cfg.default_prefix_path),
        escape(&cfg.default_proton),
        cfg.default_gamemode,
        cfg.default_mangohud,
        escape(&cfg.lsfg_path),
        escape(&cfg.lsfg_multiplier),
    );
    let mut ids: Vec<&String> = cfg.games.keys().collect();
    ids.sort();
    for id in ids {
        let g = &cfg.games[id];
        let mut has_fields = false;
        let mut section_str = format!("\n[games.\"{}\"]\n", escape(id));
        if let Some(p) = &g.install_path {
            section_str.push_str(&format!("install_path = \"{}\"\n", escape(p)));
            has_fields = true;
        }
        if let Some(pr) = &g.proton {
            section_str.push_str(&format!("proton = \"{}\"\n", escape(pr)));
            has_fields = true;
        }
        if let Some(gm) = g.gamemode {
            section_str.push_str(&format!("gamemode = {}\n", gm));
            has_fields = true;
        }
        if let Some(mh) = g.mangohud {
            section_str.push_str(&format!("mangohud = {}\n", mh));
            has_fields = true;
        }
        if let Some(ls) = &g.lsfg {
            section_str.push_str(&format!("lsfg = \"{}\"\n", escape(ls)));
            has_fields = true;
        }
        if let Some(pfx) = &g.prefix_path {
            section_str.push_str(&format!("prefix_path = \"{}\"\n", escape(pfx)));
            has_fields = true;
        }
        if let Some(args) = &g.launch_args {
            section_str.push_str(&format!("launch_args = \"{}\"\n", escape(args)));
            has_fields = true;
        }
        if has_fields {
            out.push_str(&section_str);
        }
    }
    out
}

pub fn config_path() -> PathBuf {
    filesystem::config_dir().join("config.toml")
}

pub fn load() -> Config {
    load_from(&config_path())
}

pub fn load_from(path: &Path) -> Config {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text),
        Err(_) => Config::default(),
    }
}

pub fn save(cfg: &Config) -> Result<(), String> {
    save_to(&config_path(), cfg)
}

pub fn save_to(path: &Path, cfg: &Config) -> Result<(), String> {
    filesystem::atomic_write(path, render(cfg).as_bytes())
}

/// A configured path is only accepted if it exists and is a directory.
pub fn validate_dir(path: &str) -> Result<(), String> {
    let p = Path::new(path);
    if !p.exists() {
        return Err(format!("does not exist: {path}"));
    }
    if !p.is_dir() {
        return Err(format!("not a directory: {path}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut cfg = Config {
            default_install_path: "/mnt/Games/EpicGames".into(),
            default_proton: "GE-Proton11-7".into(),
            default_gamemode: true,
            lsfg_multiplier: "2x".into(),
            ..Default::default()
        };
        cfg.games.insert(
            "Eel".into(),
            GameConfig {
                install_path: Some("/mnt/Games/NonSteamGames/KCD".into()),
                proton: Some("Proton Experimental".into()),
                gamemode: Some(true),
                mangohud: Some(true),
                lsfg: Some("2x".into()),
                prefix_path: Some("/mnt/Games/Prefixes/Eel".into()),
                launch_args: Some("-novid -dx11".into()),
            },
        );
        let text = render(&cfg);
        let back = parse(&text);
        assert_eq!(back.default_install_path, "/mnt/Games/EpicGames");
        assert_eq!(back.default_proton, "GE-Proton11-7");
        assert!(back.default_gamemode);
        assert_eq!(back.lsfg_multiplier, "2x");
        assert_eq!(
            back.games["Eel"].install_path.as_deref(),
            Some("/mnt/Games/NonSteamGames/KCD")
        );
        assert_eq!(
            back.games["Eel"].proton.as_deref(),
            Some("Proton Experimental")
        );
        assert_eq!(back.games["Eel"].gamemode, Some(true));
        assert_eq!(back.games["Eel"].mangohud, Some(true));
        assert_eq!(back.games["Eel"].lsfg.as_deref(), Some("2x"));
        assert_eq!(
            back.games["Eel"].prefix_path.as_deref(),
            Some("/mnt/Games/Prefixes/Eel")
        );
        assert_eq!(
            back.games["Eel"].launch_args.as_deref(),
            Some("-novid -dx11")
        );
    }

    #[test]
    fn tolerant_parse() {
        let cfg =
            parse("# comment\n\ngarbage line\ndefault_install_path = \"/x\"\n[other]\nk = \"v\"\n");
        assert_eq!(cfg.default_install_path, "/x");
        assert!(cfg.games.is_empty());
    }
}
