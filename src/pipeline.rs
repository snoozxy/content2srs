use std::collections::HashMap;
use std::path::Path;

use tracing::{info, warn};

use crate::apkg;
use crate::audio;
use crate::bundle::{self, Bundle};
use crate::config::{AudioFormat, BuildConfig, ImageFormat};
use crate::errors::Result;
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

    let mut audio_map: HashMap<String, Result<Vec<u8>>> = HashMap::new();

    if !config.no_audio {
        let ranges: Vec<_> = pending
            .iter()
            .map(|j| (j.card_id.clone(), j.start_ms, j.end_ms))
            .collect();
        for (id, result) in audio::extract_audio_clips(
            ffmpeg,
            &sources.audio.path,
            sources.audio.stream_index,
            &ranges,
            config,
        ) {
            audio_map.insert(id, result);
        }
    }

    let hwaccel = hwaccel::resolve_hwaccel(ffmpeg, Some(config.hwaccel.as_str()));
    let video = sources.video.as_ref();
    let batch_count = pending.len().div_ceil(FRAME_BATCH);

    for (batch_idx, chunk) in pending.chunks(FRAME_BATCH).enumerate() {
        let mut image_map: HashMap<String, Result<Vec<u8>>> = HashMap::new();

        if !config.no_screenshot {
            if let Some(video) = video {
                let timestamps: Vec<_> = chunk
                    .iter()
                    .map(|j| (j.card_id.clone(), j.screenshot_ms))
                    .collect();
                info!(
                    "screenshot batch {}/{} ({} cards)",
                    batch_idx + 1,
                    batch_count,
                    chunk.len()
                );
                match frames::extract_batch(
                    ffmpeg,
                    &video.path,
                    video.stream_index,
                    &timestamps,
                    config,
                    hwaccel.as_deref(),
                ) {
                    Ok(results) => {
                        for (id, result) in results {
                            image_map.insert(id, result);
                        }
                    }
                    Err(e) => {
                        for job in chunk {
                            summary.failed += 1;
                            summary.failures.push((job.card_id.clone(), e.to_string()));
                            bundle.mark_card_failed(job, &source_label, &e.to_string())?;
                        }
                        continue;
                    }
                }
            } else if batch_idx == 0 {
                warn!("no video stream found; skipping screenshots");
            }
        }

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
            } else {
                match image_map.remove(&job.card_id) {
                    Some(Ok(bytes)) => Some(bytes),
                    Some(Err(e)) => {
                        summary.failed += 1;
                        summary.failures.push((job.card_id.clone(), e.to_string()));
                        bundle.mark_card_failed(job, &source_label, &e.to_string())?;
                        continue;
                    }
                    None if video.is_some() => {
                        summary.failed += 1;
                        let msg = "missing screenshot extraction result".to_string();
                        summary.failures.push((job.card_id.clone(), msg.clone()));
                        bundle.mark_card_failed(job, &source_label, &msg)?;
                        continue;
                    }
                    None => None,
                }
            };

            let record =
                bundle::build_card_record(job, &source_label, audio_ext, image_ext, audio, image);
            bundle.commit_card(&record)?;
            summary.committed += 1;
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
