//! Live download and install progress handling inside EGS.
//!
//! Spawns Legendary as a non-blocking child process, redirects and incrementally
//! parses stdout/stderr lines into structured progress state, renders in Ratatui,
//! and handles graceful cancellation without blocking the UI event loop.

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::thread;

#[derive(Clone, Debug, PartialEq)]
pub struct InstallProgress {
    pub app_name: String,
    pub title: String,
    pub percentage: f32,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub speed_str: String,
    pub eta_str: String,
    pub status_stage: String,
}

impl Default for InstallProgress {
    fn default() -> Self {
        Self {
            app_name: String::new(),
            title: String::new(),
            percentage: 0.0,
            downloaded_bytes: 0,
            total_bytes: 0,
            speed_str: String::from("0.0 MiB/s"),
            eta_str: String::from("--:--"),
            status_stage: String::from("Starting..."),
        }
    }
}

pub enum InstallEvent {
    Progress(InstallProgress),
}

pub struct ActiveInstall {
    pub app_name: String,
    pub title: String,
    pub progress: InstallProgress,
    child: Option<Child>,
    rx: Receiver<InstallEvent>,
    cancelled: Arc<AtomicBool>,
}

impl ActiveInstall {
    pub fn start(
        app_name: &str,
        title: &str,
        base_path: &str,
        game_folder: Option<&str>,
        total_bytes: u64,
    ) -> Result<Self, String> {
        let mut cmd = Command::new("legendary");
        cmd.args(["install", app_name, "--base-path", base_path, "-y"]);
        if let Some(folder) = game_folder {
            cmd.args(["--game-folder", folder]);
        }
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("cannot spawn legendary: {e}"))?;
        let stdout = child.stdout.take().ok_or("cannot capture stdout")?;
        let stderr = child.stderr.take().ok_or("cannot capture stderr")?;

        let (tx, rx) = channel();
        let cancelled = Arc::new(AtomicBool::new(false));

        let app_id = app_name.to_string();
        let app_title = title.to_string();
        let cancel_flag = cancelled.clone();

        // Background reader thread for stdout & stderr
        thread::spawn(move || {
            let mut prog = InstallProgress {
                app_name: app_id,
                title: app_title,
                total_bytes,
                status_stage: "Downloading".into(),
                ..Default::default()
            };

            // Merge lines from stdout and stderr
            let (line_tx, line_rx) = channel();

            let ltx1 = line_tx.clone();
            thread::spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.lines().map_while(Result::ok) {
                    let _ = ltx1.send(line);
                }
            });

            thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    let _ = line_tx.send(line);
                }
            });

            while let Ok(line) = line_rx.recv() {
                if cancel_flag.load(Ordering::SeqCst) {
                    break;
                }
                if parse_progress_line(&line, &mut prog) {
                    let _ = tx.send(InstallEvent::Progress(prog.clone()));
                }
            }
        });

        let initial_prog = InstallProgress {
            app_name: app_name.to_string(),
            title: title.to_string(),
            total_bytes,
            ..Default::default()
        };

        Ok(Self {
            app_name: app_name.to_string(),
            title: title.to_string(),
            progress: initial_prog,
            child: Some(child),
            rx,
            cancelled,
        })
    }

    /// Poll for status updates. Returns true if progress was updated.
    pub fn poll(&mut self) -> Result<Option<()>, String> {
        while let Ok(evt) = self.rx.try_recv() {
            match evt {
                InstallEvent::Progress(p) => {
                    self.progress = p;
                }
            }
        }

        // Check if child process has exited
        if let Some(child) = &mut self.child {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if status.success() {
                        return Ok(Some(()));
                    } else if self.cancelled.load(Ordering::SeqCst) {
                        return Err("Installation cancelled.".into());
                    } else {
                        return Err(format!("Legendary exited with code {:?}", status.code()));
                    }
                }
                Ok(None) => {}
                Err(e) => return Err(format!("Failed to monitor install: {e}")),
            }
        }
        Ok(None)
    }

    /// Cancel the install gracefully.
    pub fn cancel(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Parse progress numbers from a line of legendary output.
pub fn parse_progress_line(line: &str, p: &mut InstallProgress) -> bool {
    let mut updated = false;

    // Line e.g. "= Progress: 64.12% (1234/2000), Running for 00:01:23, ETA: 00:08:14"
    if let Some(idx) = line.find("Progress:") {
        let rest = &line[idx + 9..].trim_start();
        if let Some(perc_end) = rest.find('%') {
            if let Ok(val) = rest[..perc_end].trim().parse::<f32>() {
                p.percentage = val;
                p.status_stage = "Downloading".into();
                updated = true;
            }
        }
        if let Some(eta_idx) = rest.find("ETA:") {
            let eta_part = rest[eta_idx + 4..].trim();
            let eta = eta_part.split_whitespace().next().unwrap_or(eta_part);
            p.eta_str = eta.to_string();
            updated = true;
        }
    }

    // Line e.g. " - Downloaded: 42800.00 MiB, Written: 45000.00 MiB"
    if let Some(idx) = line.find("Downloaded:") {
        let rest = &line[idx + 11..].trim_start();
        if let Some(mib_str) = rest.split_whitespace().next() {
            if let Ok(mib) = mib_str.parse::<f64>() {
                p.downloaded_bytes = (mib * 1024.0 * 1024.0) as u64;
                updated = true;
            }
        }
    }

    // Line e.g. " + Download	- 48.20 MiB/s (raw) / 52.10 MiB/s (decompressed)"
    if line.contains("Download") && line.contains("MiB/s") {
        if let Some(idx) = line.find('-') {
            let rest = &line[idx + 1..].trim_start();
            if let Some(speed_end) = rest.find("(raw)") {
                p.speed_str = rest[..speed_end].trim().to_string();
                updated = true;
            } else if let Some(m_idx) = rest.find("MiB/s") {
                p.speed_str = rest[..m_idx + 5].trim().to_string();
                updated = true;
            }
        }
    }

    // Line e.g. "Verification progress: 123/456 (27.0%) [45.2 MiB/s]"
    if let Some(idx) = line.find("Verification progress:") {
        p.status_stage = "Verifying".into();
        let rest = &line[idx + 22..].trim_start();
        if let Some(open) = rest.find('(') {
            if let Some(close) = rest[open..].find('%') {
                if let Ok(val) = rest[open + 1..open + close].trim().parse::<f32>() {
                    p.percentage = val;
                    updated = true;
                }
            }
        }
        if let Some(sq_open) = rest.find('[') {
            if let Some(sq_close) = rest[sq_open..].find(']') {
                p.speed_str = rest[sq_open + 1..sq_open + sq_close].trim().to_string();
                updated = true;
            }
        }
    }

    if line.contains("Running prerequisite") || line.contains("prerequisites") {
        p.status_stage = "Running prerequisites".into();
        updated = true;
    } else if line.contains("Finishing installation") || line.contains("Waiting for installation") {
        p.status_stage = "Finishing installation".into();
        updated = true;
    }

    updated
}

#[derive(Clone, Debug, PartialEq)]
pub struct VerifyProgress {
    pub app_name: String,
    pub title: String,
    pub percentage: f32,
    pub files_checked: u64,
    pub total_files: u64,
    pub bad_files: u64,
    pub speed_str: String,
    pub message: String,
}

impl VerifyProgress {
    pub fn files_checked_str(&self) -> String {
        if self.total_files > 0 {
            format!("{}/{}", self.files_checked, self.total_files)
        } else {
            format!("{}", self.files_checked)
        }
    }
}

impl Default for VerifyProgress {
    fn default() -> Self {
        Self {
            app_name: String::new(),
            title: String::new(),
            percentage: 0.0,
            files_checked: 0,
            total_files: 0,
            bad_files: 0,
            speed_str: String::from("0.0 MiB/s"),
            message: String::from("Starting verification..."),
        }
    }
}

pub struct ActiveVerify {
    pub app_name: String,
    pub title: String,
    pub progress: VerifyProgress,
    child: Option<Child>,
    rx: Receiver<VerifyProgress>,
    cancelled: Arc<AtomicBool>,
}

impl ActiveVerify {
    pub fn start(app_name: &str, title: &str) -> Result<Self, String> {
        let mut cmd = Command::new("legendary");
        cmd.args(["verify", app_name]);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("cannot spawn legendary verify: {e}"))?;
        let stdout = child.stdout.take().ok_or("cannot capture stdout")?;
        let stderr = child.stderr.take().ok_or("cannot capture stderr")?;

        let (tx, rx) = channel();
        let cancelled = Arc::new(AtomicBool::new(false));

        let app_id = app_name.to_string();
        let app_title = title.to_string();
        let cancel_flag = cancelled.clone();

        thread::spawn(move || {
            let mut prog = VerifyProgress {
                app_name: app_id,
                title: app_title,
                message: "Verifying...".into(),
                ..Default::default()
            };

            let (line_tx, line_rx) = channel();
            let ltx1 = line_tx.clone();
            thread::spawn(move || {
                let reader = BufReader::new(stdout);
                for line in reader.split(b'\t').flatten() {
                    if let Ok(s) = String::from_utf8(line) {
                        let _ = ltx1.send(s);
                    }
                }
            });
            thread::spawn(move || {
                let reader = BufReader::new(stderr);
                for line in reader.lines().map_while(Result::ok) {
                    let _ = line_tx.send(line);
                }
            });

            while let Ok(line) = line_rx.recv() {
                if cancel_flag.load(Ordering::SeqCst) {
                    break;
                }
                if parse_verify_line(&line, &mut prog) {
                    let _ = tx.send(prog.clone());
                }
            }
        });

        let initial = VerifyProgress {
            app_name: app_name.to_string(),
            title: title.to_string(),
            ..Default::default()
        };

        Ok(Self {
            app_name: app_name.to_string(),
            title: title.to_string(),
            progress: initial,
            child: Some(child),
            rx,
            cancelled,
        })
    }

    pub fn poll(&mut self) -> Result<Option<()>, String> {
        while let Ok(p) = self.rx.try_recv() {
            self.progress = p;
        }
        if let Some(child) = &mut self.child {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if status.success() {
                        return Ok(Some(()));
                    } else if self.cancelled.load(Ordering::SeqCst) {
                        return Err("Verification cancelled.".into());
                    } else {
                        return Err(format!(
                            "Legendary verify failed with code {:?}",
                            status.code()
                        ));
                    }
                }
                Ok(None) => {}
                Err(e) => return Err(format!("verify child wait error: {e}")),
            }
        }
        Ok(None)
    }

    pub fn cancel(&mut self) {
        self.cancelled.store(true, Ordering::SeqCst);
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

pub fn parse_verify_line(line: &str, p: &mut VerifyProgress) -> bool {
    let mut updated = false;
    let t = line.trim();
    if let Some(idx) = t.find("Verification progress:") {
        let rest = t[idx + 22..].trim();
        if let Some(slash) = rest.find('/') {
            if let Ok(cur) = rest[..slash].trim().parse::<u64>() {
                p.files_checked = cur;
                updated = true;
            }
            let after_slash = &rest[slash + 1..];
            if let Some(paren) = after_slash.find('(') {
                if let Ok(tot) = after_slash[..paren].trim().parse::<u64>() {
                    p.total_files = tot;
                    updated = true;
                }
                let after_paren = &after_slash[paren + 1..];
                if let Some(pct_end) = after_paren.find('%') {
                    if let Ok(pct) = after_paren[..pct_end].trim().parse::<f32>() {
                        p.percentage = pct;
                        updated = true;
                    }
                }
            }
        }
        if let Some(br_open) = rest.find('[') {
            if let Some(br_close) = rest[br_open..].find(']') {
                p.speed_str = rest[br_open + 1..br_open + br_close].trim().to_string();
                updated = true;
            }
        }
        p.message = "Verifying files...".into();
    } else if t.contains("finished successfully") {
        p.percentage = 100.0;
        p.message = "Verification finished successfully: no corrupted files detected.".into();
        updated = true;
    }
    updated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_legendary_progress_lines() {
        let mut p = InstallProgress::default();
        let l1 = "= Progress: 64.12% (1234/2000), Running for 00:01:23, ETA: 00:08:14";
        assert!(parse_progress_line(l1, &mut p));
        assert_eq!(p.percentage, 64.12);
        assert_eq!(p.eta_str, "00:08:14");
        assert_eq!(p.status_stage, "Downloading");

        let l2 = " - Downloaded: 4280.00 MiB, Written: 4500.00 MiB";
        assert!(parse_progress_line(l2, &mut p));
        assert!(p.downloaded_bytes > 4_000_000_000);

        let l3 = " + Download - 48.20 MiB/s (raw) / 52.10 MiB/s (decompressed)";
        assert!(parse_progress_line(l3, &mut p));
        assert_eq!(p.speed_str, "48.20 MiB/s");

        let l4 = "Verification progress: 123/456 (27.0%) [45.2 MiB/s]";
        assert!(parse_progress_line(l4, &mut p));
        assert_eq!(p.status_stage, "Verifying");
        assert_eq!(p.percentage, 27.0);
        assert_eq!(p.speed_str, "45.2 MiB/s");

        let mut vp = VerifyProgress::default();
        let vl1 = "Verification progress: 142/142 (100.0%) [0.0 MiB/s]";
        assert!(parse_verify_line(vl1, &mut vp));
        assert_eq!(vp.files_checked, 142);
        assert_eq!(vp.total_files, 142);
        assert_eq!(vp.percentage, 100.0);
        assert_eq!(vp.speed_str, "0.0 MiB/s");

        let vl2 = "[cli] INFO: Verification finished successfully.";
        assert!(parse_verify_line(vl2, &mut vp));
        assert!(vp.message.contains("finished successfully"));
    }
}
