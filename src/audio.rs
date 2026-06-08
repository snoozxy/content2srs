use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;

use rayon::prelude::*;
use tracing::info;

use crate::config::{AudioFormat, BuildConfig};
use crate::errors::{AppError, Result};
use crate::ffmpeg::{AudioStreamInfo, FfmpegTools};
use crate::pool;

const READ_CHUNK: usize = 256 * 1024;

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

    let info = match ffmpeg.audio_stream_info(input, audio_stream_index) {
        Ok(i) => i,
        Err(e) => {
            return ranges
                .iter()
                .map(|(id, _, _)| (id.clone(), Err(AppError::Ffmpeg(e.to_string()))))
                .collect();
        }
    };

    info!(
        "decoding audio once ({} Hz, {} ch) then encoding {} clips with {} workers",
        info.sample_rate,
        info.channels,
        ranges.len(),
        config.workers
    );

    let jobs = match decode_pcm_jobs(ffmpeg, input, &info, ranges) {
        Ok(j) => j,
        Err(e) => {
            return ranges
                .iter()
                .map(|(id, _, _)| (id.clone(), Err(e.clone())))
                .collect();
        }
    };

    let ffmpeg_path = ffmpeg.ffmpeg.clone();
    let format = config.audio_format.clone();
    let bitrate = config.audio_bitrate_kbps;

    let encoded: BTreeMap<String, Result<Vec<u8>>> = pool::install_pool_with(config.workers, || {
        jobs
            .into_par_iter()
            .map(|(id, pcm)| {
                let bytes = encode_pcm(
                    &ffmpeg_path,
                    &pcm,
                    info.sample_rate,
                    info.channels,
                    &format,
                    bitrate,
                );
                (id, bytes)
            })
            .collect()
    });

    ranges
        .iter()
        .map(|(id, _, _)| {
            (
                id.clone(),
                encoded
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| Err(AppError::Ffmpeg("missing audio clip".into()))),
            )
        })
        .collect()
}

fn decode_pcm_jobs(
    ffmpeg: &FfmpegTools,
    input: &Path,
    info: &AudioStreamInfo,
    ranges: &[(String, u64, u64)],
) -> Result<Vec<(String, Vec<u8>)>> {
    let mut by_end: BTreeMap<u64, Vec<(String, u64, u64)>> = BTreeMap::new();
    for (id, start, end) in ranges {
        by_end.entry(*end).or_default().push((id.clone(), *start, *end));
    }

    let mut child = Command::new(&ffmpeg.ffmpeg)
        .arg("-v")
        .arg("error")
        .arg("-i")
        .arg(input)
        .arg("-map")
        .arg(format!("0:{}", info.stream_index))
        .arg("-vn")
        .arg("-f")
        .arg("s16le")
        .arg("-acodec")
        .arg("pcm_s16le")
        .arg("-ar")
        .arg(info.sample_rate.to_string())
        .arg("-ac")
        .arg(info.channels.to_string())
        .arg("pipe:1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AppError::Ffmpeg(e.to_string()))?;

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::Ffmpeg("no ffmpeg stdout".into()))?;

    let stderr_handle = child.stderr.take().map(|mut stderr| {
        thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = stderr.read_to_end(&mut buf);
            buf
        })
    });

    let mut buffer: Vec<u8> = Vec::new();
    // Absolute byte offset in the decoded PCM stream (always frame-aligned).
    let mut buffer_start_byte: u64 = 0;
    let mut read_buf = [0u8; READ_CHUNK];
    let mut jobs = Vec::with_capacity(ranges.len());
    let mut completed_ends: Vec<u64> = Vec::new();

    loop {
        let n = stdout
            .read(&mut read_buf)
            .map_err(|e| AppError::Ffmpeg(e.to_string()))?;
        if n == 0 {
            break;
        }
        buffer.extend_from_slice(&read_buf[..n]);

        let stream_end_byte = buffer_start_byte + buffer.len() as u64;
        let position_ms = info.bytes_to_ms(stream_end_byte);

        let ready_ends: Vec<u64> = by_end
            .range(..=position_ms)
            .map(|(k, _)| *k)
            .collect();

        for end_ms in ready_ends {
            if let Some(batch) = by_end.remove(&end_ms) {
                for (id, start_ms, end_ms) in batch {
                    let pcm = slice_pcm(&buffer, buffer_start_byte, start_ms, end_ms, info);
                    jobs.push((id, pcm));
                }
                completed_ends.push(end_ms);
            }
        }

        if let Some(trim_to) = completed_ends.last().copied() {
            trim_buffer(&mut buffer, &mut buffer_start_byte, trim_to, info);
            completed_ends.clear();
        }
    }

    let status = child
        .wait()
        .map_err(|e| AppError::Ffmpeg(e.to_string()))?;
    let stderr = stderr_handle
        .map(|h| h.join().unwrap_or_default())
        .unwrap_or_default();
    if !status.success() {
        return Err(AppError::Ffmpeg(format!(
            "audio decode failed: {}",
            String::from_utf8_lossy(&stderr)
        )));
    }

    // Flush any cues whose end lies at/near EOF.
    for (_, batch) in by_end {
        for (id, start_ms, end_ms) in batch {
            let pcm = slice_pcm(&buffer, buffer_start_byte, start_ms, end_ms, info);
            jobs.push((id, pcm));
        }
    }

    Ok(jobs)
}

fn trim_buffer(
    buffer: &mut Vec<u8>,
    buffer_start_byte: &mut u64,
    trim_to_ms: u64,
    info: &AudioStreamInfo,
) {
    let trim_abs = info.ms_to_bytes(trim_to_ms);
    if trim_abs <= *buffer_start_byte {
        return;
    }
    let drop_bytes = (trim_abs - *buffer_start_byte) as usize;
    if drop_bytes >= buffer.len() {
        buffer.clear();
    } else {
        buffer.drain(..drop_bytes);
    }
    *buffer_start_byte = trim_abs;
}

fn slice_pcm(
    buffer: &[u8],
    buffer_start_byte: u64,
    start_ms: u64,
    end_ms: u64,
    info: &AudioStreamInfo,
) -> Vec<u8> {
    if buffer.is_empty() {
        return Vec::new();
    }
    let abs_start = info.ms_to_bytes(start_ms);
    let abs_end = info.ms_to_bytes_ceil(end_ms);
    if abs_end <= abs_start {
        return Vec::new();
    }
    let stream_end = buffer_start_byte + buffer.len() as u64;
    if abs_start >= stream_end {
        return Vec::new();
    }
    let rel_start = abs_start.saturating_sub(buffer_start_byte) as usize;
    let rel_end = abs_end.min(stream_end).saturating_sub(buffer_start_byte) as usize;
    if rel_start >= buffer.len() || rel_start >= rel_end {
        return Vec::new();
    }
    buffer[rel_start..rel_end].to_vec()
}

fn encode_pcm(
    ffmpeg: &PathBuf,
    pcm: &[u8],
    sample_rate: u32,
    channels: u16,
    format: &AudioFormat,
    bitrate_kbps: u32,
) -> Result<Vec<u8>> {
    if pcm.is_empty() {
        return Err(AppError::Ffmpeg("empty pcm slice".into()));
    }

    let (codec, extra): (&str, &[&str]) = match format {
        AudioFormat::Mp3 => ("libmp3lame", &[]),
        AudioFormat::Opus => ("libopus", &["-vbr", "on"]),
    };

    let mut child = Command::new(ffmpeg)
        .arg("-v")
        .arg("error")
        .arg("-f")
        .arg("s16le")
        .arg("-ar")
        .arg(sample_rate.to_string())
        .arg("-ac")
        .arg(channels.to_string())
        .arg("-i")
        .arg("pipe:0")
        .arg("-c:a")
        .arg(codec)
        .arg("-b:a")
        .arg(format!("{bitrate_kbps}k"))
        .args(extra)
        .arg("-f")
        .arg(match format {
            AudioFormat::Mp3 => "mp3",
            AudioFormat::Opus => "opus",
        })
        .arg("pipe:1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| AppError::Ffmpeg(e.to_string()))?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| AppError::Ffmpeg("no ffmpeg stdin".into()))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::Ffmpeg("no ffmpeg stdout".into()))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| AppError::Ffmpeg("no ffmpeg stderr".into()))?;

    let pcm_owned = pcm.to_vec();
    let stdin_handle = thread::spawn(move || -> Result<()> {
        stdin
            .write_all(&pcm_owned)
            .map_err(|e| AppError::Ffmpeg(e.to_string()))?;
        // EOF tells ffmpeg the PCM stream is complete.
        Ok(())
    });
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

    stdin_handle
        .join()
        .map_err(|_| AppError::Ffmpeg("stdin writer thread panicked".into()))??;
    let stdout = stdout_handle
        .join()
        .map_err(|_| AppError::Ffmpeg("stdout reader thread panicked".into()))?;
    let status = child.wait().map_err(|e| AppError::Ffmpeg(e.to_string()))?;
    let stderr = stderr_handle
        .join()
        .map_err(|_| AppError::Ffmpeg("stderr reader thread panicked".into()))?;

    if !status.success() {
        return Err(AppError::Ffmpeg(format!(
            "pcm encode failed: {}",
            String::from_utf8_lossy(&stderr)
        )));
    }
    Ok(stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ffmpeg::AudioStreamInfo;

    fn stereo_44k() -> AudioStreamInfo {
        AudioStreamInfo {
            stream_index: 0,
            sample_rate: 44_100,
            channels: 2,
        }
    }

    #[test]
    fn slice_pcm_stays_frame_aligned() {
        let info = stereo_44k();
        let frame = info.frame_bytes() as usize;
        // 100ms of silence-ish PCM at 44.1k stereo
        let bytes_per_100ms = info.ms_to_bytes(100) as usize;
        let buffer: Vec<u8> = (0..bytes_per_100ms * 10)
            .map(|i| (i % 256) as u8)
            .collect();

        let pcm = slice_pcm(&buffer, 0, 100, 250, &info);
        assert!(!pcm.is_empty());
        assert_eq!(pcm.len() % frame, 0);
    }
}
