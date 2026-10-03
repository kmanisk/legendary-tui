//! Core data types shared across the application.

use serde::{Deserialize, Serialize};

/// One Epic library entry, optionally installed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Game {
    /// Stable Epic identifier (never shown as config key substitute).
    pub app_name: String,
    /// Human-readable display title.
    pub title: String,
    pub installed: bool,
    pub version: Option<String>,
    pub install_path: Option<String>,
    #[serde(default)]
    pub needs_update: bool,
    #[serde(default)]
    pub available_version: Option<String>,
}

/// Rich metadata for a game (cached on disk / in memory).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct GameDetails {
    pub app_name: String,
    pub title: String,
    pub installed: bool,
    pub version: Option<String>,
    pub install_path: Option<String>,
    pub download_size: Option<u64>,
    pub installed_size: Option<u64>,
    pub developer: Option<String>,
    pub publisher: Option<String>,
    pub release_date: Option<String>,
    pub genres: Vec<String>,
    pub description: Option<String>,
    pub launch_exe: Option<String>,
    // Rich manifest & Heroic/Lutris-style fields
    #[serde(default)]
    pub cloud_saves: Option<bool>,
    #[serde(default)]
    pub cloud_save_folder: Option<String>,
    #[serde(default)]
    pub command_line: Option<String>,
    #[serde(default)]
    pub extra_launch_options: Option<String>,
    #[serde(default)]
    pub is_dlc: bool,
    #[serde(default)]
    pub owned_dlc: Vec<String>,
    #[serde(default)]
    pub installed_dlc: Vec<String>,
    #[serde(default)]
    pub build_id: Option<String>,
    #[serde(default)]
    pub prerequisites: Vec<String>,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub grant_date: Option<String>,
    #[serde(default)]
    pub manifest_cached: bool,
    #[serde(default)]
    pub protondb_tier: Option<String>,
    #[serde(default)]
    pub needs_update: bool,
    #[serde(default)]
    pub available_version: Option<String>,
}

/// Relevance score for search ranking: exact > prefix > substring > fuzzy.
pub fn search_score(title: &str, query: &str) -> Option<u8> {
    let t = title.to_lowercase();
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Some(1);
    }
    if t == q {
        return Some(4);
    }
    if t.starts_with(&q) {
        return Some(3);
    }
    if t.contains(&q) {
        return Some(2);
    }
    // Simple subsequence fuzzy match.
    let mut qi = q.chars();
    let mut cur = qi.next()?;
    for ch in t.chars() {
        if ch == cur {
            match qi.next() {
                Some(next) => cur = next,
                None => return Some(1),
            }
        }
    }
    None
}

/// Match against title, app_name, developer, publisher, and genres, prioritizing title.
pub fn full_search_score(game: &Game, details: Option<&GameDetails>, query: &str) -> Option<u8> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Some(1);
    }
    if let Some(s) = search_score(&game.title, &q) {
        return Some(10 + s); // 11..=14
    }
    if game.app_name.to_lowercase().contains(&q) {
        return Some(8);
    }
    if let Some(d) = details {
        if let Some(dev) = &d.developer {
            if dev.to_lowercase().contains(&q) {
                return Some(6);
            }
        }
        if let Some(publ) = &d.publisher {
            if publ.to_lowercase().contains(&q) {
                return Some(5);
            }
        }
        for g in &d.genres {
            if g.to_lowercase().contains(&q) {
                return Some(4);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranking_order() {
        assert_eq!(search_score("Control", "control"), Some(4));
        assert_eq!(search_score("Control Ultimate", "control"), Some(3));
        assert_eq!(search_score("Remedy Control", "control"), Some(2));
        assert!(search_score("Celeste", "control").is_none());
        // subsequence fuzzy still matches
        assert_eq!(search_score("Chivalry 2", "chvy"), Some(1));
    }

    #[test]
    fn full_search_ranking() {
        let g = Game {
            app_name: "4656facc".into(),
            title: "20 Minutes Till Dawn".into(),
            installed: false,
            version: None,
            install_path: None,
            needs_update: false,
            available_version: None,
        };
        let d = GameDetails {
            app_name: "4656facc".into(),
            title: "20 Minutes Till Dawn".into(),
            installed: false,
            version: None,
            install_path: None,
            download_size: None,
            installed_size: None,
            developer: Some("flanne".into()),
            publisher: None,
            release_date: None,
            genres: vec!["Roguelike".into(), "Action".into()],
            description: None,
            launch_exe: None,
            ..Default::default()
        };
        assert!(full_search_score(&g, Some(&d), "Minutes").unwrap() > 10);
        assert_eq!(full_search_score(&g, Some(&d), "flanne"), Some(6));
        assert_eq!(full_search_score(&g, Some(&d), "roguelike"), Some(4));
        assert_eq!(full_search_score(&g, Some(&d), "4656"), Some(8));
        assert!(full_search_score(&g, Some(&d), "nomatch").is_none());
    }
}
