use std::path::Path;

use rayon::prelude::*;
use tempfile::TempDir;

use crate::config::{AudioFormat, BuildConfig};
use crate::errors::{AppError, Result};
use crate::ffmpeg::FfmpegTools;
use crate::util::ms_to_ffmpeg_time;

const BATCH_SIZE: usize = 32;

pub fn extract_audio_clips(
    ffmpeg: &FfmpegTools,
    input: &Path,
    audio_stream_index: usize,
    ranges: &[(String, u64, u64)],
    config: &BuildConfig,
) -> Vec<(String, Result<Vec<u8>>)> {
    ranges
        .par_chunks(BATCH_SIZE)
        .flat_map(|chunk| {
            extract_audio_batch(ffmpeg, input, audio_stream_index, chunk, config)
                .unwrap_or_else(|e| {
                    chunk
                        .iter()
                        .map(|(id, _, _)| (id.clone(), Err(e.clone())))
                        .collect()
                })
        })
        .collect()
}

fn extract_audio_batch(
    ffmpeg: &FfmpegTools,
    input: &Path,
    audio_stream_index: usize,
    batch: &[(String, u64, u64)],
    config: &BuildConfig,
) -> Result<Vec<(String, Result<Vec<u8>>)>> {
    let tmp = TempDir::new().map_err(|e| AppError::Ffmpeg(e.to_string()))?;
    let input_str = input.to_str().unwrap_or_default();
    let (codec, ext) = match config.audio_format {
        AudioFormat::Mp3 => ("libmp3lame", "mp3"),
        AudioFormat::Opus => ("libopus", "opus"),
    };

    let mut filter_parts = Vec::new();
    let mut outputs = Vec::new();
    let mut args: Vec<String> = vec!["-y".into(), "-i".into(), input_str.into()];

    for (i, (id, start_ms, end_ms)) in batch.iter().enumerate() {
        let label = format!("a{i}");
        let start = ms_to_ffmpeg_time(*start_ms);
        let end = ms_to_ffmpeg_time(*end_ms);
        let mut chain = format!(
            "[0:{audio_stream_index}]atrim=start={start}:end={end},asetpts=PTS-STARTPTS"
        );
        if config.loudnorm {
            chain.push_str(",loudnorm");
        }
        chain.push_str(&format!("[{label}]"));
        filter_parts.push(chain);

        let out_path = tmp.path().join(format!("{id}.{ext}"));
        outputs.push((id.clone(), out_path));
    }

    let filter_complex = filter_parts.join(";");
    args.push("-filter_complex".into());
    args.push(filter_complex);

    for (i, (_, out_path)) in outputs.iter().enumerate() {
        let label = format!("[a{i}]");
        args.push("-map".into());
        args.push(label);
        args.push("-c:a".into());
        args.push(codec.into());
        args.push("-b:a".into());
        args.push(format!("{}k", config.audio_bitrate_kbps));
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
