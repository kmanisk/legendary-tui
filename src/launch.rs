//! Launch configuration and structured command composition.
//!
//! Avoids fragile shell pipelines. Composes GameMode, Prime offload, LSFG,
//! and Proton/umu execution using structured `std::process::Command`.

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::proton::{primary_steam_root, resolve_proton};

#[derive(Clone, Debug)]
pub struct LaunchConfig {
    pub app_name: String,
    pub game_exe: PathBuf,
    pub working_dir: PathBuf,
    pub prefix_path: PathBuf,
    pub proton_choice: String,
    pub gamemode: bool,
    pub mangohud: bool,
    pub lsfg_choice: String,
    pub extra_args: Vec<String>,
    pub env_vars: Vec<(String, String)>,
}

impl LaunchConfig {
    /// Build the executable `Command` without shell string formatting.
    pub fn build_command(&self) -> Result<Command, String> {
        if !self.game_exe.is_file() {
            return Err(format!(
                "Game executable not found at:\n{}\n\nUse Repair or Refresh metadata to resolve.",
                self.game_exe.display()
            ));
        }

        let proton_ver = resolve_proton(&self.proton_choice)?;
        let proton_bin = proton_ver.path.join("proton");
        if !proton_bin.is_file() {
            return Err(format!(
                "Proton binary not found at:\n{}\n\nPlease select another Proton in Settings.",
                proton_bin.display()
            ));
        }

        // Determine if umu-run is present (recommended for EAC/container runtime)
        let has_umu = which::which("umu-run").is_ok();
        let has_gamemode = self.gamemode && which::which("gamemoderun").is_ok();
        let has_prime_run = which::which("prime-run").is_ok();

        // Start command chain
        let mut cmd;
        if has_gamemode {
            cmd = Command::new("gamemoderun");
            if has_prime_run {
                cmd.arg("prime-run");
            }
        } else if has_prime_run {
            cmd = Command::new("prime-run");
        } else if has_umu {
            cmd = Command::new("umu-run");
        } else {
            cmd = Command::new(&proton_bin);
        }

        // If wrapped by gamemoderun or prime-run, append the core runner
        if (has_gamemode || has_prime_run) && has_umu {
            cmd.arg("umu-run");
        } else if (has_gamemode || has_prime_run) && !has_umu {
            cmd.arg(&proton_bin);
            cmd.arg("run");
        } else if !has_gamemode && !has_prime_run && !has_umu {
            cmd.arg("run");
        }

        // Apply MangoHud if requested
        if self.mangohud {
            cmd.env("MANGOHUD", "1");
        }

        // Apply LSFG if requested and wrapper exists
        if !self.lsfg_choice.is_empty() && self.lsfg_choice != "Disabled" {
            // Check for lossless scaling or lsfg environment / wrapper
            cmd.env("LSFG_MULTIPLIER", &self.lsfg_choice);
        }

        // Set working directory
        cmd.current_dir(&self.working_dir);

        // Core Proton / Wine environment variables
        cmd.env("STEAM_COMPAT_DATA_PATH", &self.prefix_path);
        cmd.env("WINEPREFIX", &self.prefix_path);
        if let Some(steam_root) = primary_steam_root() {
            cmd.env("STEAM_COMPAT_CLIENT_INSTALL_PATH", steam_root);
        }

        // For umu runtime
        cmd.env("PROTONPATH", &proton_ver.path);
        cmd.env("GAMEID", format!("umu-{}", self.app_name));
        cmd.env("STORE", "egs");

        // Custom environment variables
        for (k, v) in &self.env_vars {
            cmd.env(k, v);
        }

        // Target executable + arguments
        cmd.arg(&self.game_exe);
        cmd.args(&self.extra_args);

        Ok(cmd)
    }
}

/// Fallback helper to search PATH for a binary.
mod which {
    use std::path::PathBuf;

    pub fn which(name: &str) -> Result<PathBuf, ()> {
        let paths = std::env::var_os("PATH").unwrap_or_default();
        for p in std::env::split_paths(&paths) {
            let candidate = p.join(name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
        Err(())
    }
}

/// Resolve the launch executable for an installed game.
pub fn resolve_game_exe(install_path: &Path, known_exe: Option<&str>) -> Result<PathBuf, String> {
    if !install_path.is_dir() {
        return Err(format!(
            "Install directory does not exist: {}",
            install_path.display()
        ));
    }

    if let Some(exe_name) = known_exe {
        let direct = install_path.join(exe_name);
        if direct.is_file() {
            return Ok(direct);
        }

        // Search recursively for the filename (ignoring case)
        if let Some(found) = find_file_recursive(install_path, exe_name) {
            return Ok(found);
        }
    }

    // Inspect directory for potential .exe files (excluding EAC/setup/crashpad)
    if let Some(candidate) = find_best_exe(install_path) {
        return Ok(candidate);
    }

    Err(format!(
        "Installed files exist, but launch executable could not be resolved in:\n{}\nUse Repair / Refresh metadata.",
        install_path.display()
    ))
}

fn find_file_recursive(dir: &Path, target: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let target_lower = target.to_lowercase();
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            if let Some(found) = find_file_recursive(&p, target) {
                return Some(found);
            }
        } else if p.is_file() {
            if let Some(name) = p.file_name().and_then(|n| n.to_str()) {
                if name.to_lowercase() == target_lower {
                    return Some(p);
                }
            }
        }
    }
    None
}

fn find_best_exe(dir: &Path) -> Option<PathBuf> {
    let mut exes = Vec::new();
    collect_exes(dir, &mut exes, 0);

    // Filter out obvious helper tools
    exes.retain(|p| {
        let s = p
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase();
        !s.contains("setup")
            && !s.contains("uninstall")
            && !s.contains("crash")
            && !s.contains("redist")
            && !s.contains("prereq")
            && !s.contains("easyanticheat")
            && !s.contains("unitycrash")
    });

    exes.into_iter().next()
}

fn collect_exes(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 4 {
        return;
    }
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_exes(&p, out, depth + 1);
        } else if p.is_file() {
            if let Some(ext) = p.extension() {
                if ext.eq_ignore_ascii_case("exe") {
                    out.push(p);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_exe_direct() {
        let tmp = std::env::temp_dir().join(format!("egs-launch-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let exe = tmp.join("Game.exe");
        std::fs::write(&exe, b"MZ").unwrap();

        let resolved = resolve_game_exe(&tmp, Some("Game.exe")).unwrap();
        assert_eq!(resolved, exe);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn launch_missing_exe_fails_gracefully() {
        let cfg = LaunchConfig {
            app_name: "test_app".into(),
            game_exe: PathBuf::from("/nonexistent/Game.exe"),
            working_dir: PathBuf::from("/nonexistent"),
            prefix_path: PathBuf::from("/nonexistent/prefix"),
            proton_choice: "Auto".into(),
            gamemode: false,
            mangohud: false,
            lsfg_choice: "Disabled".into(),
            extra_args: Vec::new(),
            env_vars: Vec::new(),
        };
        let err = cfg.build_command().unwrap_err();
        assert!(err.contains("executable not found"));
    }
}
