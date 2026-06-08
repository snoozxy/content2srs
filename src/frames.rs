use std::path::Path;

use rayon::prelude::*;
use tempfile::TempDir;
use tracing::info;

use crate::config::{BuildConfig, ImageFormat};
use crate::errors::{AppError, Result};
use crate::ffmpeg::FfmpegTools;
use crate::hwaccel;
use crate::pool;

/// Timestamps per ffmpeg select batch (keeps filter expressions under OS limits).
pub const FRAME_BATCH: usize = 96;

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
    sorted.dedup_by_key(|(_, t)| *t);

    let hwaccel = hwaccel::resolve_hwaccel(ffmpeg, Some(config.hwaccel.as_str()));

    info!(
        "extracting {} screenshots in select batches (batch={}, workers={}, hwaccel={})",
        sorted.len(),
        FRAME_BATCH,
        config.workers,
        hwaccel.as_deref().unwrap_or("none")
    );

    let chunks: Vec<_> = sorted.chunks(FRAME_BATCH).collect();

    pool::install_pool_with(config.workers, || {
        chunks
            .par_iter()
            .flat_map(|chunk| {
                extract_batch(ffmpeg, input, video_stream_index, chunk, config, hwaccel.as_deref())
                    .unwrap_or_else(|e| {
                        chunk
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
    let tmp = TempDir::new().map_err(|e| AppError::Ffmpeg(e.to_string()))?;
    let input_str = input.to_str().unwrap_or_default();

    let select_expr: String = batch
        .iter()
        .map(|(_, ms)| {
            let t = *ms as f64 / 1000.0;
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

    if config.exact_frame {
        args.push("-i".into());
    } else {
        args.push("-noaccurate_seek".into());
        args.push("-i".into());
    }
    args.push(input_str.into());
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
