use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

use rayon::prelude::*;
use tracing::info;

use crate::config::{AudioFormat, BuildConfig};
use crate::errors::{AppError, Result};
use crate::ffmpeg::FfmpegTools;
use crate::pool;

pub fn extract_audio_clips(
    ffmpeg: &FfmpegTools,
    input: &Path,
    audio_stream_index: usize,
    ranges: &[(String, u64, u64)],
    config: &BuildConfig,
) -> Vec<(String, Result<Vec<u8>>)> {
    if ranges.is_empty() {
        return Vec::new();
    }

    info!(
        "extracting {} audio clips with {} workers (per-clip ffmpeg)",
        ranges.len(),
        config.workers
    );

    let ffmpeg_path = ffmpeg.ffmpeg.clone();
    let input_path = input.to_path_buf();
    let format = config.audio_format.clone();
    let bitrate = config.audio_bitrate_kbps;

    pool::install_pool_with(config.workers, || {
        ranges
            .par_iter()
            .map(|(id, start_ms, end_ms)| {
                let bytes = extract_single_clip(
                    &ffmpeg_path,
                    &input_path,
                    audio_stream_index,
                    *start_ms,
                    *end_ms,
                    &format,
                    bitrate,
                );
                (id.clone(), bytes)
            })
            .collect()
    })
}

fn extract_single_clip(
    ffmpeg: &PathBuf,
    input: &Path,
    stream_index: usize,
    start_ms: u64,
    end_ms: u64,
    format: &AudioFormat,
    bitrate_kbps: u32,
) -> Result<Vec<u8>> {
    if end_ms <= start_ms {
        return Err(AppError::Ffmpeg("invalid audio range".into()));
    }

    let input_str = input.to_str().unwrap_or_default();
    let start_s = format!("{:.3}", start_ms as f64 / 1000.0);
    let dur_s = format!("{:.3}", (end_ms - start_ms) as f64 / 1000.0);
    let map = format!("0:{stream_index}");
    let bitrate = format!("{bitrate_kbps}k");

    let (codec, extra, out_fmt): (&str, &[&str], &str) = match format {
        AudioFormat::Mp3 => ("libmp3lame", &[], "mp3"),
        AudioFormat::Opus => ("libopus", &["-vbr", "on"], "opus"),
    };

    let mut args: Vec<String> = vec![
        "-v".into(),
        "error".into(),
        "-ss".into(),
        start_s,
        "-i".into(),
        input_str.into(),
        "-map".into(),
        map,
        "-t".into(),
        dur_s,
        "-vn".into(),
        "-c:a".into(),
        codec.into(),
        "-b:a".into(),
        bitrate,
    ];
    for arg in extra {
        args.push((*arg).into());
    }
    args.push("-f".into());
    args.push(out_fmt.into());
    args.push("pipe:1".into());

    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    run_ffmpeg_capture(ffmpeg, &arg_refs)
}

fn run_ffmpeg_capture(ffmpeg: &PathBuf, args: &[&str]) -> Result<Vec<u8>> {
    let mut child = Command::new(ffmpeg)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AppError::Ffmpeg(e.to_string()))?;

    let stdin = child.stdin.take();
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::Ffmpeg("no ffmpeg stdout".into()))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| AppError::Ffmpeg("no ffmpeg stderr".into()))?;

    drop(stdin);

    let stdout_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let stderr_handle = thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr.read_to_end(&mut buf);
        buf
    });

    let stdout = stdout_handle
        .join()
        .map_err(|_| AppError::Ffmpeg("stdout reader thread panicked".into()))?;
    let status = child.wait().map_err(|e| AppError::Ffmpeg(e.to_string()))?;
    let stderr = stderr_handle
        .join()
        .map_err(|_| AppError::Ffmpeg("stderr reader thread panicked".into()))?;

    if !status.success() {
        return Err(AppError::Ffmpeg(format!(
            "ffmpeg failed: {}",
            String::from_utf8_lossy(&stderr)
        )));
    }
    if stdout.is_empty() {
        return Err(AppError::Ffmpeg("empty ffmpeg output".into()));
    }
    Ok(stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ImageFormat;
    use std::process::Command as ShellCommand;

    fn test_config(video: PathBuf) -> BuildConfig {
        BuildConfig {
            inputs: vec![video],
            subtitle: None,
            output: None,
            apkg: None,
            audio_index: None,
            subtitle_index: None,
            video_index: None,
            lang: None,
            padding_ms: 0,
            merge_threshold_ms: 0,
            context_lines: 0,
            regex_filter: None,
            no_filter: true,
            audio_format: AudioFormat::Mp3,
            audio_bitrate_kbps: 128,
            loudnorm: false,
            image_format: ImageFormat::Jpg,
            screenshot_offset_ms: 0,
            exact_frame: false,
            hwaccel: "none".into(),
            workers: 1,
            resume: true,
            dry_run: false,
            no_audio: false,
            no_screenshot: false,
            deck_name: "test".into(),
        }
    }

    fn ensure_sample_video() -> PathBuf {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("sample.mp4");
        if path.exists() {
            return path;
        }
        std::fs::create_dir_all(path.parent().unwrap()).ok();
        let status = ShellCommand::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=320x240:rate=24:duration=10",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=10",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-c:a",
                "aac",
                "-shortest",
                path.to_str().unwrap(),
            ])
            .status()
            .expect("ffmpeg required");
        assert!(status.success());
        path
    }

    #[test]
    fn per_clip_audio_extracts_bytes() {
        let video = ensure_sample_video();
        let ffmpeg = FfmpegTools::discover(None).expect("ffmpeg");
        let results = extract_audio_clips(
            &ffmpeg,
            &video,
            1,
            &[("card_00000".into(), 1000, 2500)],
            &test_config(video.clone()),
        );
        let (_, Ok(bytes)) = &results[0] else {
            panic!("clip extraction failed: {:?}", results);
        };
        assert!(bytes.len() > 100);
    }
}
