use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct FfmpegTools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

impl FfmpegTools {
    pub fn discover(explicit: Option<PathBuf>) -> Result<Self> {
        if let Some(path) = explicit {
            let parent = path.parent().context("invalid ffmpeg path")?.to_path_buf();
            return Ok(Self {
                ffmpeg: path,
                ffprobe: parent.join(if cfg!(windows) {
                    "ffprobe.exe"
                } else {
                    "ffprobe"
                }),
            });
        }

        let ffmpeg = which::which("ffmpeg").context(
            "ffmpeg not found on PATH; install ffmpeg or set CONTENT2SRS_FFMPEG",
        )?;
        let ffprobe = which::which("ffprobe").context("ffprobe not found on PATH")?;
        Ok(Self { ffmpeg, ffprobe })
    }

    pub fn probe_json(&self, path: &Path) -> Result<ProbeResult> {
        let output = Command::new(&self.ffprobe)
            .args([
                "-v",
                "quiet",
                "-print_format",
                "json",
                "-show_format",
                "-show_streams",
                "-show_chapters",
                path.to_str().unwrap_or_default(),
            ])
            .output()
            .with_context(|| format!("failed to run ffprobe on {}", path.display()))?;

        if !output.status.success() {
            bail!(
                "ffprobe failed for {}: {}",
                path.display(),
                String::from_utf8_lossy(&output.stderr)
            );
        }

        serde_json::from_slice(&output.stdout)
            .with_context(|| format!("failed to parse ffprobe output for {}", path.display()))
    }

    pub fn run(&self, args: &[&str]) -> Result<()> {
        let output = Command::new(&self.ffmpeg)
            .args(args)
            .output()
            .context("failed to spawn ffmpeg")?;

        if !output.status.success() {
            bail!(
                "ffmpeg failed (code {:?}): {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }

    pub fn run_capture(&self, args: &[&str]) -> Result<Vec<u8>> {
        let output = Command::new(&self.ffmpeg)
            .args(args)
            .output()
            .context("failed to spawn ffmpeg")?;

        if !output.status.success() {
            bail!(
                "ffmpeg failed (code {:?}): {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(output.stdout)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProbeResult {
    pub streams: Option<Vec<ProbeStream>>,
    pub format: Option<ProbeFormat>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProbeFormat {
    pub duration: Option<String>,
    pub format_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProbeStream {
    pub index: Option<u32>,
    pub codec_type: Option<String>,
    pub codec_name: Option<String>,
    pub tags: Option<serde_json::Map<String, serde_json::Value>>,
    pub disposition: Option<serde_json::Map<String, serde_json::Value>>,
}

impl ProbeStream {
    pub fn language(&self) -> Option<String> {
        self.tags.as_ref().and_then(|t| {
            t.get("language")
                .or_else(|| t.get("LANGUAGE"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_lowercase())
        })
    }

    pub fn is_forced(&self) -> bool {
        self.disposition
            .as_ref()
            .and_then(|d| d.get("forced"))
            .and_then(|v| v.as_i64())
            .map(|v| v == 1)
            .unwrap_or(false)
    }
}
