use std::collections::HashMap;
use std::path::Path;

use rayon::prelude::*;
use tempfile::TempDir;
use tracing::info;

use crate::config::{AudioFormat, BuildConfig};
use crate::errors::{AppError, Result};
use crate::ffmpeg::FfmpegTools;
use crate::pool;
use crate::util::ms_to_ffmpeg_time;

/// Clips per ffmpeg batch (keeps filter_complex graphs and per-batch decode
/// windows a reasonable size). Matches frames::FRAME_BATCH in spirit.
pub const AUDIO_BATCH: usize = 96;

/// Seek this far before the earliest clip so ffmpeg has a nearby sync point.
const SEEK_MARGIN_MS: u64 = 2_000;

/// Extra decode tail after the last clip window.
const CAPTURE_TAIL_MS: u64 = 500;

/// Compute input seek position and decode duration for a temporally grouped batch.
fn batch_seek_range(batch: &[(String, u64, u64)]) -> (u64, u64) {
    let min_ms = batch.iter().map(|(_, start, _)| *start).min().unwrap_or(0);
    let max_ms = batch.iter().map(|(_, _, end)| *end).max().unwrap_or(min_ms);
    let seek_ms = min_ms.saturating_sub(SEEK_MARGIN_MS);
    let duration_ms = max_ms.saturating_sub(seek_ms) + CAPTURE_TAIL_MS;
    (seek_ms, duration_ms.max(1))
}

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

    let mut sorted: Vec<(String, u64, u64)> = ranges.to_vec();
    sorted.sort_by_key(|(_, start, _)| *start);
    let batches: Vec<Vec<(String, u64, u64)>> =
        sorted.chunks(AUDIO_BATCH).map(|c| c.to_vec()).collect();

    info!(
        "extracting {} audio clips in {} batches (up to {AUDIO_BATCH}/batch, {} workers)",
        ranges.len(),
        batches.len(),
        config.workers
    );

    let map: HashMap<String, Result<Vec<u8>>> = pool::install_pool_with(config.workers, || {
        batches
            .par_iter()
            .flat_map(|batch| {
                extract_batch(ffmpeg, input, audio_stream_index, batch, config).unwrap_or_else(
                    |e| {
                        batch
                            .iter()
                            .map(|(id, _, _)| (id.clone(), Err(e.clone())))
                            .collect()
                    },
                )
            })
            .collect()
    });

    ranges
        .iter()
        .map(|(id, _, _)| {
            (
                id.clone(),
                map.get(id)
                    .cloned()
                    .unwrap_or_else(|| Err(AppError::Ffmpeg("missing audio clip".into()))),
            )
        })
        .collect()
}

/// Extract every clip in `batch` with a single ffmpeg invocation: one input
/// decode pass feeds an `asplit` fan-out, and each branch is `atrim`'d to its
/// own clip window. `asplit`/`atrim` (unlike sequential per-output `-ss`) don't
/// require non-overlapping, monotonically increasing ranges, so overlapping
/// padded cues (adjacent cues can overlap once `--padding-ms` is applied) are
/// handled correctly.
fn extract_batch(
    ffmpeg: &FfmpegTools,
    input: &Path,
    stream_index: usize,
    batch: &[(String, u64, u64)],
    config: &BuildConfig,
) -> Result<Vec<(String, Result<Vec<u8>>)>> {
    if batch.is_empty() {
        return Ok(Vec::new());
    }

    // Validate up front so one bad range doesn't take down the whole batch.
    let mut valid = Vec::with_capacity(batch.len());
    let mut results = Vec::with_capacity(batch.len());
    for entry @ (id, start_ms, end_ms) in batch {
        if end_ms <= start_ms {
            results.push((
                id.clone(),
                Err(AppError::Ffmpeg("invalid audio range".into())),
            ));
        } else {
            valid.push(entry.clone());
        }
    }
    if valid.is_empty() {
        return Ok(results);
    }

    let tmp = TempDir::new().map_err(|e| AppError::Ffmpeg(e.to_string()))?;
    let input_str = input.to_str().unwrap_or_default();
    let (seek_ms, duration_ms) = batch_seek_range(&valid);

    let n = valid.len();
    let split_labels: String = (0..n).map(|i| format!("[s{i}]")).collect();
    let mut filter_parts = vec![format!("[0:{stream_index}]asplit={n}{split_labels}")];
    for (i, (_, start_ms, end_ms)) in valid.iter().enumerate() {
        let start_s = *start_ms as f64 / 1000.0;
        let end_s = *end_ms as f64 / 1000.0;
        filter_parts.push(format!(
            "[s{i}]atrim=start={start_s:.3}:end={end_s:.3},asetpts=PTS-STARTPTS[a{i}]"
        ));
    }
    let filter_complex = filter_parts.join(";");

    let bitrate = format!("{}k", config.audio_bitrate_kbps);
    let (codec, extra, ext): (&str, &[&str], &str) = match config.audio_format {
        AudioFormat::Mp3 => ("libmp3lame", &[], "mp3"),
        AudioFormat::Opus => ("libopus", &["-vbr", "on"], "opus"),
    };

    let mut args: Vec<String> = vec![
        "-y".into(),
        "-v".into(),
        "error".into(),
        "-ss".into(),
        ms_to_ffmpeg_time(seek_ms),
        "-t".into(),
        ms_to_ffmpeg_time(duration_ms),
        "-copyts".into(),
        "-i".into(),
        input_str.into(),
        "-filter_complex".into(),
        filter_complex,
    ];

    let mut out_paths = Vec::with_capacity(n);
    for i in 0..n {
        let out_path = tmp.path().join(format!("clip_{i:04}.{ext}"));
        args.push("-map".into());
        args.push(format!("[a{i}]"));
        args.push("-c:a".into());
        args.push(codec.into());
        args.push("-b:a".into());
        args.push(bitrate.clone());
        for e in extra {
            args.push((*e).into());
        }
        args.push(out_path.to_str().unwrap_or_default().into());
        out_paths.push(out_path);
    }

    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    ffmpeg
        .run(&arg_refs)
        .map_err(|e| AppError::Ffmpeg(e.to_string()))?;

    for ((id, _, _), path) in valid.iter().zip(out_paths.iter()) {
        let bytes = std::fs::read(path).map_err(|e| AppError::Ffmpeg(e.to_string()));
        results.push((id.clone(), bytes));
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ImageFormat;
    use std::path::PathBuf;
    use std::process::Command as ShellCommand;

    fn test_config(video: PathBuf) -> BuildConfig {
        BuildConfig {
            inputs: vec![video],
            batch: false,
            jobs: None,
            subtitle: None,
            output: None,
            apkg: None,
            audio_index: None,
            subtitle_index: None,
            video_index: None,
            lang: None,
            padding_ms: 0,
            merge_threshold_ms: 0,
            max_gap_ms: 400,
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
            summaries: None,
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

    #[test]
    fn batched_clips_extract_independently_including_overlaps() {
        let video = ensure_sample_video();
        let ffmpeg = FfmpegTools::discover(None).expect("ffmpeg");
        // Overlapping ranges (as padding can produce between adjacent cues)
        // plus out-of-order input, to prove asplit/atrim doesn't need sorted
        // non-overlapping ranges.
        let ranges = vec![
            ("b".to_string(), 3000u64, 4500u64),
            ("a".to_string(), 1000u64, 2500u64),
            ("c".to_string(), 4000u64, 5000u64), // overlaps "b"
        ];
        let results = extract_audio_clips(&ffmpeg, &video, 1, &ranges, &test_config(video.clone()));
        assert_eq!(results.len(), 3);
        for (id, result) in &results {
            let bytes = result
                .as_ref()
                .unwrap_or_else(|e| panic!("clip {id} failed: {e:?}"));
            assert!(bytes.len() > 100, "clip {id} unexpectedly small");
        }
    }

    #[test]
    fn invalid_range_does_not_fail_whole_batch() {
        let video = ensure_sample_video();
        let ffmpeg = FfmpegTools::discover(None).expect("ffmpeg");
        let ranges = vec![
            ("good".to_string(), 1000u64, 2500u64),
            ("bad".to_string(), 5000u64, 4000u64), // end <= start
        ];
        let results = extract_audio_clips(&ffmpeg, &video, 1, &ranges, &test_config(video.clone()));
        let map: HashMap<_, _> = results.into_iter().collect();
        assert!(map.get("good").unwrap().is_ok());
        assert!(map.get("bad").unwrap().is_err());
    }
}
