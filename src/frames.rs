use std::path::Path;

use rayon::prelude::*;
use tempfile::TempDir;

use crate::config::{BuildConfig, ImageFormat};
use crate::errors::{AppError, Result};
use crate::ffmpeg::FfmpegTools;
use crate::util::ms_to_ffmpeg_time;

const BATCH_SIZE: usize = 32;

pub fn extract_frames(
    ffmpeg: &FfmpegTools,
    input: &Path,
    video_stream_index: usize,
    timestamps: &[(String, u64)],
    config: &BuildConfig,
) -> Vec<(String, Result<Vec<u8>>)> {
    let mut sorted = timestamps.to_vec();
    sorted.sort_by_key(|(_, t)| *t);

    sorted
        .par_chunks(BATCH_SIZE)
        .flat_map(|chunk| {
            extract_frame_batch(ffmpeg, input, video_stream_index, chunk, config)
                .unwrap_or_else(|e| {
                    chunk
                        .iter()
                        .map(|(id, _)| (id.clone(), Err(e.clone())))
                        .collect()
                })
        })
        .collect()
}

fn extract_frame_batch(
    ffmpeg: &FfmpegTools,
    input: &Path,
    video_stream_index: usize,
    batch: &[(String, u64)],
    config: &BuildConfig,
) -> Result<Vec<(String, Result<Vec<u8>>)>> {
    let tmp = TempDir::new().map_err(|e| AppError::Ffmpeg(e.to_string()))?;
    let input_str = input.to_str().unwrap_or_default();

    let (codec_args, ext): (Vec<&str>, &str) = match config.image_format {
        ImageFormat::Jpg => (vec!["-q:v", "2"], "jpg"),
        ImageFormat::Webp => (vec!["-c:v", "libwebp", "-quality", "80"], "webp"),
    };

    let mut filter_parts = Vec::new();
    let mut outputs = Vec::new();
    let mut args: Vec<String> = Vec::new();

    if let Some(hw) = &config.hwaccel {
        args.push("-hwaccel".into());
        args.push(hw.clone());
    }

    if config.exact_frame {
        args.push("-i".into());
        args.push(input_str.into());
    } else {
        args.push("-i".into());
        args.push(input_str.into());
    }

    for (i, (id, ts_ms)) in batch.iter().enumerate() {
        let label = format!("v{i}");
        let t = ms_to_ffmpeg_time(*ts_ms);
        let duration = "0.04";
        filter_parts.push(format!(
            "[0:{video_stream_index}]trim=start={t}:duration={duration},setpts=PTS-STARTPTS,scale=1280:-1[{label}]"
        ));
        let out_path = tmp.path().join(format!("{id}.{ext}"));
        outputs.push((id.clone(), out_path));
    }

    let filter_complex = filter_parts.join(";");
    args.push("-filter_complex".into());
    args.push(filter_complex);

    for (i, (_, out_path)) in outputs.iter().enumerate() {
        let label = format!("[v{i}]");
        args.push("-map".into());
        args.push(label);
        args.push("-frames:v".into());
        args.push("1".into());
        for ca in &codec_args {
            args.push((*ca).into());
        }
        args.push(out_path.to_str().unwrap_or_default().into());
    }

    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    ffmpeg
        .run(&arg_refs)
        .map_err(|e| AppError::Ffmpeg(e.to_string()))?;

    let mut results = Vec::new();
    for (id, path) in outputs {
        let bytes = std::fs::read(&path).map_err(|e| AppError::Ffmpeg(e.to_string()));
        results.push((id, bytes));
    }
    Ok(results)
}
