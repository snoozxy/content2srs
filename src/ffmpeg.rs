use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

#[derive(Debug, Clone)]
pub struct AudioStreamInfo {
    pub stream_index: usize,
    pub sample_rate: u32,
    pub channels: u16,
}

impl AudioStreamInfo {
    pub fn bytes_per_second(&self) -> u64 {
        self.sample_rate as u64 * self.channels as u64 * 2
    }

    pub fn bytes_to_ms(&self, bytes: u64) -> u64 {
        let bps = self.bytes_per_second().max(1);
        bytes.saturating_mul(1000) / bps
    }

    pub fn ms_to_bytes(&self, ms: u64) -> u64 {
        let bps = self.bytes_per_second();
        ms.saturating_mul(bps) / 1000
    }
}

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

    pub fn audio_stream_info(&self, path: &Path, stream_index: usize) -> Result<AudioStreamInfo> {
        let probe = self.probe_json(path)?;
        let stream = probe
            .streams
            .as_ref()
            .and_then(|streams| streams.iter().find(|s| s.index.unwrap_or(0) as usize == stream_index))
            .context("audio stream not found in probe")?;

        let sample_rate = stream
            .sample_rate
            .as_deref()
            .and_then(|s| s.parse().ok())
            .unwrap_or(48_000);
        let channels = stream
            .channels
            .unwrap_or(2)
            .clamp(1, 8) as u16;

        Ok(AudioStreamInfo {
            stream_index,
            sample_rate,
            channels,
        })
    }

    pub fn hwaccel_available(&self, hwaccel: &str) -> bool {
        let output = Command::new(&self.ffmpeg)
            .args([
                "-hide_banner",
                "-v",
                "error",
                "-hwaccel",
                hwaccel,
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=0.1:size=64x64:rate=1",
                "-frames:v",
                "1",
                "-f",
                "null",
                "-",
            ])
            .output();

        match output {
            Ok(o) => o.status.success(),
            Err(_) => false,
        }
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
    pub sample_rate: Option<String>,
    pub channels: Option<u32>,
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
