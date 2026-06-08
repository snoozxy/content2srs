use std::collections::HashMap;
use std::path::Path;

use rayon::prelude::*;
use tempfile::TempDir;
use tracing::info;

use crate::config::{BuildConfig, ImageFormat};
use crate::errors::{AppError, Result};
use crate::ffmpeg::FfmpegTools;
use crate::hwaccel;
use crate::pool;
use crate::util::ms_to_ffmpeg_time;

/// Timestamps per ffmpeg select batch (keeps filter expressions under OS limits).
pub const FRAME_BATCH: usize = 96;

/// Seek this far before the earliest frame so ffmpeg lands on a nearby keyframe.
const SEEK_MARGIN_MS: u64 = 3_000;

/// Extra decode tail after the last frame window.
const CAPTURE_TAIL_MS: u64 = 500;

/// Compute input seek position and decode duration for a temporally grouped batch.
pub fn batch_seek_range(batch: &[(String, u64)]) -> (u64, u64) {
    let min_ms = batch.iter().map(|(_, ms)| *ms).min().unwrap_or(0);
    let max_ms = batch.iter().map(|(_, ms)| *ms).max().unwrap_or(min_ms);
    let seek_ms = min_ms.saturating_sub(SEEK_MARGIN_MS);
    let duration_ms = max_ms.saturating_sub(seek_ms) + CAPTURE_TAIL_MS;
    (seek_ms, duration_ms.max(1))
}

pub fn extract_frames(
    ffmpeg: &FfmpegTools,
    input: &Path,
    video_stream_index: usize,
    timestamps: &[(String, u64)],
    config: &BuildConfig,
) -> Vec<(String, Result<Vec<u8>>)> {
    if timestamps.is_empty() {
        return Vec::new();
    }

    let mut sorted: Vec<(String, u64)> = timestamps.to_vec();
    sorted.sort_by_key(|(_, t)| *t);

    let hwaccel = hwaccel::resolve_hwaccel(ffmpeg, Some(config.hwaccel.as_str()));
    let batches: Vec<Vec<(String, u64)>> = sorted.chunks(FRAME_BATCH).map(|c| c.to_vec()).collect();

    let map = extract_batches_parallel(
        ffmpeg,
        input,
        video_stream_index,
        &batches,
        config,
        hwaccel.as_deref(),
    );

    timestamps
        .iter()
        .map(|(id, _)| {
            (
                id.clone(),
                map.get(id)
                    .cloned()
                    .unwrap_or_else(|| Err(AppError::Ffmpeg("missing screenshot".into()))),
            )
        })
        .collect()
}

pub fn extract_batches_parallel(
    ffmpeg: &FfmpegTools,
    input: &Path,
    video_stream_index: usize,
    batches: &[Vec<(String, u64)>],
    config: &BuildConfig,
    hwaccel: Option<&str>,
) -> HashMap<String, Result<Vec<u8>>> {
    if batches.is_empty() {
        return HashMap::new();
    }

    let frame_count: usize = batches.iter().map(|b| b.len()).sum();
    info!(
        "extracting {frame_count} screenshots in {} parallel batches (up to {FRAME_BATCH}/batch, {} workers, hwaccel={})",
        batches.len(),
        config.workers,
        hwaccel.unwrap_or("none")
    );

    pool::install_pool_with(config.workers, || {
        batches
            .par_iter()
            .flat_map(|batch| {
                extract_batch(ffmpeg, input, video_stream_index, batch, config, hwaccel)
                    .unwrap_or_else(|e| {
                        batch
                            .iter()
                            .map(|(id, _)| (id.clone(), Err(e.clone())))
                            .collect()
                    })
            })
            .collect()
    })
}

pub fn extract_batch(
    ffmpeg: &FfmpegTools,
    input: &Path,
    _video_stream_index: usize,
    batch: &[(String, u64)],
    config: &BuildConfig,
    hwaccel: Option<&str>,
) -> Result<Vec<(String, Result<Vec<u8>>)>> {
    if batch.is_empty() {
        return Ok(Vec::new());
    }

    let tmp = TempDir::new().map_err(|e| AppError::Ffmpeg(e.to_string()))?;
    let input_str = input.to_str().unwrap_or_default();
    let (seek_ms, duration_ms) = batch_seek_range(batch);
    let seek_sec = seek_ms as f64 / 1000.0;

    let select_expr: String = batch
        .iter()
        .map(|(_, ms)| {
            let t = (*ms as f64 / 1000.0) - seek_sec;
            let t2 = t + 0.05;
            format!("between(t\\,{t:.3}\\,{t2:.3})")
        })
        .collect::<Vec<_>>()
        .join("+");

    let vf = format!("select='{select_expr}',setpts=N/FRAME_RATE/TB,scale=1280:-1");

    let (codec_args, ext): (Vec<&str>, &str) = match config.image_format {
        ImageFormat::Jpg => (vec!["-q:v", "2"], "jpg"),
        ImageFormat::Webp => (vec!["-c:v", "libwebp", "-quality", "80"], "webp"),
    };

    let pattern = tmp.path().join(format!("frame_%04d.{ext}"));
    let pattern_str = pattern.to_str().unwrap_or_default();

    let mut args: Vec<String> = vec!["-y".into(), "-v".into(), "error".into()];

    if let Some(hw) = hwaccel {
        args.push("-hwaccel".into());
        args.push(hw.into());
        if hw == "cuda" {
            args.push("-hwaccel_output_format".into());
            args.push("cuda".into());
        }
    }

    args.push("-ss".into());
    args.push(ms_to_ffmpeg_time(seek_ms));

    if config.exact_frame {
        args.push("-i".into());
    } else {
        args.push("-noaccurate_seek".into());
        args.push("-i".into());
    }
    args.push(input_str.into());
    args.push("-t".into());
    args.push(ms_to_ffmpeg_time(duration_ms));
    args.push("-vf".into());
    args.push(vf);
    args.push("-vsync".into());
    args.push("vfr".into());
    args.push("-start_number".into());
    args.push("0".into());
    args.push("-frames:v".into());
    args.push(batch.len().to_string());
    for ca in codec_args {
        args.push(ca.into());
    }
    args.push(pattern_str.into());

    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    ffmpeg
        .run(&arg_refs)
        .map_err(|e| AppError::Ffmpeg(e.to_string()))?;

    let mut results = Vec::new();
    for (i, (id, _)) in batch.iter().enumerate() {
        let path = tmp.path().join(format!("frame_{i:04}.{ext}"));
        let bytes = std::fs::read(&path).map_err(|e| AppError::Ffmpeg(e.to_string()));
        results.push((id.clone(), bytes));
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_seek_range_clamps_to_zero() {
        let batch = vec![("a".into(), 500), ("b".into(), 2000)];
        let (seek, dur) = batch_seek_range(&batch);
        assert_eq!(seek, 0);
        assert_eq!(dur, 2000 + CAPTURE_TAIL_MS);
    }

    #[test]
    fn batch_seek_range_applies_margin() {
        let batch = vec![("a".into(), 60_000), ("b".into(), 65_000)];
        let (seek, dur) = batch_seek_range(&batch);
        assert_eq!(seek, 60_000 - SEEK_MARGIN_MS);
        assert_eq!(dur, 65_000 - seek + CAPTURE_TAIL_MS);
    }
}
