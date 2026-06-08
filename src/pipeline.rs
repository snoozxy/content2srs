use std::collections::HashMap;
use std::path::Path;
use std::thread;

use tracing::{info, warn};

use crate::apkg;
use crate::audio;
use crate::bundle::{self, Bundle};
use crate::config::{AudioFormat, BuildConfig, ImageFormat};
use crate::errors::{AppError, Result};
use crate::ffmpeg::FfmpegTools;
use crate::frames::{self, FRAME_BATCH};
use crate::hwaccel;
use crate::jobplan::JobPlan;
use crate::probe::{self, SubtitleSource};
use crate::subtitles::{self, subtitle_filename};

#[derive(Debug, Default)]
pub struct BuildSummary {
    pub total: usize,
    pub committed: usize,
    pub skipped: usize,
    pub failed: usize,
    pub failures: Vec<(String, String)>,
}

pub fn run_build(ffmpeg: &FfmpegTools, config: &BuildConfig) -> Result<BuildSummary> {
    let sources = probe::MediaSources::discover(ffmpeg, config)?;
    let source_label = sources
        .audio
        .path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".into());

    let subtitle_bytes = match &sources.subtitle {
        SubtitleSource::External { path, bytes } => {
            (
                bytes.clone(),
                subtitle_filename(path),
                path.extension().and_then(|e| e.to_str()).map(String::from),
            )
        }
        SubtitleSource::Embedded {
            container,
            stream_index,
            codec: _,
        } => {
            let bytes = probe::demux_subtitle(ffmpeg, container, *stream_index)?;
            let ext = "srt".to_string();
            (bytes, format!("embedded_{stream_index}.srt"), Some(ext))
        }
    };

    let cues = subtitles::load_from_bytes(config, &subtitle_bytes.0, subtitle_bytes.2.as_deref())?;
    let plan = JobPlan::from_cues(&cues, config.screenshot_offset_ms);

    let bundle_path = config.default_output();
    let bundle = Bundle::open(&bundle_path, true)?;
    bundle.init_build_meta(config, &source_label)?;
    bundle.store_subtitle_asset(&subtitle_bytes.1, "text/plain", &subtitle_bytes.0)?;

    if config.dry_run {
        info!(
            "dry run: {} cards planned, bundle would be {}",
            plan.cards.len(),
            bundle_path.display()
        );
        return Ok(BuildSummary {
            total: plan.cards.len(),
            ..Default::default()
        });
    }

    let mut summary = BuildSummary {
        total: plan.cards.len(),
        ..Default::default()
    };

    let pending: Vec<_> = plan
        .cards
        .iter()
        .filter(|job| {
            if !config.resume {
                return true;
            }
            match bundle.has_complete_card(&job.card_id) {
                Ok(true) => {
                    summary.skipped += 1;
                    false
                }
                _ => true,
            }
        })
        .cloned()
        .collect();

    if pending.is_empty() {
        info!("all {} cards already complete", summary.total);
        maybe_export_apkg(&bundle, config)?;
        return Ok(summary);
    }

    let audio_ext = match config.audio_format {
        AudioFormat::Mp3 => "mp3",
        AudioFormat::Opus => "opus",
    };
    let image_ext = match config.image_format {
        ImageFormat::Jpg => "jpg",
        ImageFormat::Webp => "webp",
    };

    let video = sources.video.as_ref();
    if video.is_none() && !config.no_screenshot {
        warn!("no video stream found; skipping screenshots");
    }

    let screenshot_batches: Vec<Vec<(String, u64)>> = if !config.no_screenshot && video.is_some() {
        pending
            .chunks(FRAME_BATCH)
            .map(|chunk| {
                chunk
                    .iter()
                    .map(|j| (j.card_id.clone(), j.screenshot_ms))
                    .collect()
            })
            .collect()
    } else {
        Vec::new()
    };

    let ffmpeg_audio = ffmpeg.clone();
    let ffmpeg_video = ffmpeg.clone();
    let config_audio = config.clone();
    let config_video = config.clone();
    let audio_path = sources.audio.path.clone();
    let audio_stream_index = sources.audio.stream_index;
    let audio_ranges: Vec<_> = pending
        .iter()
        .map(|j| (j.card_id.clone(), j.start_ms, j.end_ms))
        .collect();

    let video_path = video.map(|v| v.path.clone());
    let video_stream_index = video.map(|v| v.stream_index).unwrap_or(0);
    let hwaccel = hwaccel::resolve_hwaccel(ffmpeg, Some(config.hwaccel.as_str()));
    let hwaccel_label = hwaccel.clone();

    let audio_handle = if config.no_audio {
        None
    } else {
        Some(thread::spawn(move || {
            let mut map = HashMap::new();
            for (id, result) in audio::extract_audio_clips(
                &ffmpeg_audio,
                &audio_path,
                audio_stream_index,
                &audio_ranges,
                &config_audio,
            ) {
                map.insert(id, result);
            }
            map
        }))
    };

    let screenshot_handle = if screenshot_batches.is_empty() {
        None
    } else {
        let path = video_path.expect("video path required for screenshots");
        Some(thread::spawn(move || {
            frames::extract_batches_parallel(
                &ffmpeg_video,
                &path,
                video_stream_index,
                &screenshot_batches,
                &config_video,
                hwaccel.as_deref(),
            )
        }))
    };

    let audio_map: HashMap<String, Result<Vec<u8>>> = if let Some(handle) = audio_handle {
        handle
            .join()
            .map_err(|_| AppError::Ffmpeg("audio extraction thread panicked".into()))?
    } else {
        HashMap::new()
    };

    let image_map: HashMap<String, Result<Vec<u8>>> = if let Some(handle) = screenshot_handle {
        handle
            .join()
            .map_err(|_| AppError::Ffmpeg("screenshot extraction thread panicked".into()))?
    } else {
        HashMap::new()
    };

    if !config.no_screenshot && video.is_some() {
        info!(
            "media extraction complete (audio={}, screenshots={}, hwaccel={})",
            if config.no_audio { "skipped" } else { "done" },
            image_map.len(),
            hwaccel_label.as_deref().unwrap_or("none")
        );
    }

    let batch_count = pending.len().div_ceil(FRAME_BATCH);
    for (batch_idx, chunk) in pending.chunks(FRAME_BATCH).enumerate() {
        let mut records = Vec::with_capacity(chunk.len());

        for job in chunk {
            let audio = if config.no_audio {
                None
            } else {
                match audio_map.get(&job.card_id).cloned() {
                    Some(Ok(bytes)) => Some(bytes),
                    Some(Err(e)) => {
                        summary.failed += 1;
                        summary.failures.push((job.card_id.clone(), e.to_string()));
                        bundle.mark_card_failed(job, &source_label, &e.to_string())?;
                        continue;
                    }
                    None => {
                        summary.failed += 1;
                        let msg = "missing audio extraction result".to_string();
                        summary.failures.push((job.card_id.clone(), msg.clone()));
                        bundle.mark_card_failed(job, &source_label, &msg)?;
                        continue;
                    }
                }
            };

            let image = if config.no_screenshot {
                None
            } else if video.is_some() {
                match image_map.get(&job.card_id).cloned() {
                    Some(Ok(bytes)) => Some(bytes),
                    Some(Err(e)) => {
                        summary.failed += 1;
                        summary.failures.push((job.card_id.clone(), e.to_string()));
                        bundle.mark_card_failed(job, &source_label, &e.to_string())?;
                        continue;
                    }
                    None => {
                        summary.failed += 1;
                        let msg = "missing screenshot extraction result".to_string();
                        summary.failures.push((job.card_id.clone(), msg.clone()));
                        bundle.mark_card_failed(job, &source_label, &msg)?;
                        continue;
                    }
                }
            } else {
                None
            };

            records.push(bundle::build_card_record(
                job, &source_label, audio_ext, image_ext, audio, image,
            ));
        }

        if !records.is_empty() {
            info!(
                "committing batch {}/{} ({} cards)",
                batch_idx + 1,
                batch_count,
                records.len()
            );
            bundle.commit_cards(&records)?;
            summary.committed += records.len();
        }
    }

    info!(
        "build complete: {} committed, {} skipped, {} failed / {} total -> {}",
        summary.committed,
        summary.skipped,
        summary.failed,
        summary.total,
        bundle_path.display()
    );

    if !summary.failures.is_empty() {
        for (id, err) in &summary.failures {
            warn!("card {id} failed: {err}");
        }
    }

    maybe_export_apkg(&bundle, config)?;
    Ok(summary)
}

pub fn run_export(bundle_path: &Path, output: &Path, deck_name: &str) -> Result<()> {
    let bundle = Bundle::open(bundle_path, false)?;
    apkg::export_apkg(&bundle, output, deck_name)?;
    info!("exported apkg -> {}", output.display());
    Ok(())
}

fn maybe_export_apkg(bundle: &Bundle, config: &BuildConfig) -> Result<()> {
    if let Some(apkg_path) = &config.apkg {
        apkg::export_apkg(bundle, apkg_path, &config.deck_name)?;
        info!("exported apkg -> {}", apkg_path.display());
    }
    Ok(())
}
