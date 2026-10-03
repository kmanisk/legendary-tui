//! Alt+G Rofi integration. Owns ONLY the Epic registry file:
//! `~/.config/rofi/epic-games.list`, one `Title|appid|prefix` line each.
//! Never touches unrelated Rofi entries, menus, or bindings.

use crate::filesystem;

#[derive(Clone, Debug)]
pub struct Entry {
    pub title: String,
    pub appid: String,
    /// Optional prefix dir name under compatdata ("" = appid).
    pub prefix: String,
}

fn path() -> std::path::PathBuf {
    filesystem::rofi_registry()
}

fn parse_line(line: &str) -> Option<Entry> {
    let mut parts = line.splitn(3, '|');
    let title = parts.next()?.trim().to_string();
    let appid = parts.next()?.trim().to_string();
    if title.is_empty() || appid.is_empty() {
        return None;
    }
    let prefix = parts.next().unwrap_or("").trim().to_string();
    Some(Entry {
        title,
        appid,
        prefix,
    })
}

fn render(entries: &[Entry]) -> String {
    entries
        .iter()
        .map(|e| format!("{}|{}|{}", e.title, e.appid, e.prefix))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

pub fn read_entries_from(path: &std::path::Path) -> Vec<Entry> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines().filter_map(parse_line).collect()
}

fn write_entries_to(path: &std::path::Path, entries: &[Entry]) -> Result<(), String> {
    filesystem::atomic_write(path, render(entries).as_bytes())
}

pub fn entries() -> Vec<Entry> {
    read_entries_from(&path())
}

/// Idempotent add. Returns true when a line was actually appended.
pub fn ensure(title: &str, appid: &str) -> Result<bool, String> {
    let mut all = entries();
    if all.iter().any(|e| e.appid == appid) {
        return Ok(false);
    }
    all.push(Entry {
        title: title.to_string(),
        appid: appid.to_string(),
        prefix: String::new(),
    });
    write_entries_to(&path(), &all)?;
    Ok(true)
}

/// Remove the entry. Returns true when something was removed.
pub fn remove(appid: &str) -> Result<bool, String> {
    let all = entries();
    let kept: Vec<Entry> = all.into_iter().filter(|e| e.appid != appid).collect();
    let before = read_entries_from(&path()).len();
    if kept.len() == before {
        return Ok(false);
    }
    write_entries_to(&path(), &kept)?;
    Ok(true)
}

/// Entries whose app is not installed (safe cleanup candidates).
pub fn stale(installed: &std::collections::HashSet<String>) -> Vec<Entry> {
    entries()
        .into_iter()
        .filter(|e| !installed.contains(&e.appid))
        .collect()
}

/// Installed games lacking an entry.
pub fn missing(installed: &std::collections::HashSet<String>) -> Vec<String> {
    let have: std::collections::HashSet<String> = entries().into_iter().map(|e| e.appid).collect();
    let mut out: Vec<String> = installed.difference(&have).cloned().collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_legacy_and_new() {
        let p = std::env::temp_dir().join(format!("egs-rofi-legacy-{}.list", std::process::id()));
        std::fs::write(
            &p,
            "🔥 Hell Let Loose|abc123|hll\nPlain|def456|\nBadLine\n|empty|\n",
        )
        .unwrap();
        let es = read_entries_from(&p);
        assert_eq!(es.len(), 2);
        assert_eq!(es[0].prefix, "hll");
        assert_eq!(es[1].prefix, "");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn add_remove_roundtrip() {
        let p =
            std::env::temp_dir().join(format!("egs-rofi-roundtrip-{}.list", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let mut es = read_entries_from(&p);
        es.push(Entry {
            title: "X".into(),
            appid: "id1".into(),
            prefix: "".into(),
        });
        write_entries_to(&p, &es).unwrap();
        assert_eq!(read_entries_from(&p).len(), 1);
        let kept: Vec<Entry> = read_entries_from(&p)
            .into_iter()
            .filter(|e| e.appid != "id1")
            .collect();
        write_entries_to(&p, &kept).unwrap();
        assert!(read_entries_from(&p).is_empty());
        let _ = std::fs::remove_file(&p);
    }
}
