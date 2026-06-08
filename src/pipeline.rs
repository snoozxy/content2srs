use std::collections::HashMap;
use std::path::Path;
use std::thread;

use rayon::prelude::*;
use tracing::{info, warn};

use crate::apkg;
use crate::audio;
use crate::batch::{self, Episode};
use crate::bundle::{self, Bundle};
use crate::config::{AudioFormat, BuildConfig, ImageFormat};
use crate::errors::{AppError, Result};
use crate::ffmpeg::FfmpegTools;
use crate::frames::{self, FRAME_BATCH};
use crate::hwaccel;
use crate::jobplan::{CardJob, JobPlan};
use crate::pool;
use crate::probe::{self, SubtitleSource};
use crate::subtitles::{self, subtitle_filename};

#[derive(Debug, Default, Clone)]
pub struct BuildSummary {
    pub total: usize,
    pub committed: usize,
    pub skipped: usize,
    pub failed: usize,
    pub failures: Vec<(String, String)>,
}

struct PreparedEpisode {
    source_label: String,
    subtitle_name: String,
    subtitle_bytes: Vec<u8>,
    plan: JobPlan,
    sources: probe::MediaSources,
}

struct MediaMaps {
    audio: HashMap<String, Result<Vec<u8>>>,
    image: HashMap<String, Result<Vec<u8>>>,
}

struct EpisodeState {
    episode_index: usize,
    episode_label: String,
    prepared: PreparedEpisode,
    pending: Vec<CardJob>,
    skipped: usize,
}

pub fn run_build(ffmpeg: &FfmpegTools, config: &BuildConfig) -> Result<BuildSummary> {
    let paths = batch::expand_inputs(&config.inputs)?;
    let episodes = batch::discover_episodes(&paths)?;

    if episodes.len() > 1 && !config.batch {
        return Err(AppError::Config(format!(
            "found {} episodes in inputs; use --batch to build one combined deck",
            episodes.len()
        )));
    }

    if config.batch {
        if config.subtitle.is_some() {
            return Err(AppError::Config(
                "--subtitle cannot be used with --batch; place sidecar .srt/.ass next to each video"
                    .into(),
            ));
        }
        return run_batch_build(ffmpeg, config, episodes);
    }

    let episode = episodes
        .into_iter()
        .next()
        .ok_or_else(|| AppError::Config("no episodes to build".into()))?;
    let ep_config = config.for_episode(episode.video, config.subtitle.clone(), config.workers);
    run_single_build(ffmpeg, &ep_config, None)
}

fn run_batch_build(
    ffmpeg: &FfmpegTools,
    config: &BuildConfig,
    episodes: Vec<Episode>,
) -> Result<BuildSummary> {
    let episode_jobs = pool::resolve_episode_jobs(config.jobs);
    let workers_per_episode = pool::workers_per_episode(config.workers, episode_jobs);

    info!(
        "batch build: {} episodes, {episode_jobs} parallel jobs, {workers_per_episode} workers/episode",
        episodes.len()
    );

    let bundle_path = config.default_output();
    let bundle = Bundle::open(&bundle_path, true)?;
    bundle.init_build_meta(config, &config.deck_name)?;

    let mut cue_offset = 0usize;
    let mut states = Vec::with_capacity(episodes.len());

    for (episode_index, episode) in episodes.iter().enumerate() {
        let ep_config =
            config.for_episode(episode.video.clone(), episode.subtitle.clone(), workers_per_episode);
        let prepared =
            prepare_episode(ffmpeg, &ep_config, Some(&episode.prefix), cue_offset)?;
        cue_offset += prepared.plan.cards.len();

        bundle.store_subtitle_asset(
            &prepared.subtitle_name,
            "text/plain",
            &prepared.subtitle_bytes,
        )?;

        if config.dry_run {
            continue;
        }

        let (pending, skipped) =
            filter_pending(&bundle, &prepared.plan.cards, config.resume)?;
        states.push(EpisodeState {
            episode_index,
            episode_label: episode.prefix.clone(),
            prepared,
            pending,
            skipped,
        });
    }

    if config.dry_run {
        info!(
            "dry run: {cue_offset} cards planned across {} episodes -> {}",
            episodes.len(),
            bundle_path.display()
        );
        return Ok(BuildSummary {
            total: cue_offset,
            ..Default::default()
        });
    }

    let mut summary = BuildSummary {
        total: cue_offset,
        skipped: states.iter().map(|s| s.skipped).sum(),
        ..Default::default()
    };

    let media_by_episode: HashMap<usize, MediaMaps> = pool::install_pool_with(episode_jobs, || {
        states
            .par_iter()
            .filter(|s| !s.pending.is_empty())
            .map(|state| {
                let ep_config = config.for_episode(
                    state.prepared.sources.audio.path.clone(),
                    None,
                    workers_per_episode,
                );
                let media = extract_media_maps(
                    ffmpeg,
                    &ep_config,
                    &state.prepared.sources,
                    &state.pending,
                )
                .unwrap_or_else(|e| MediaMaps {
                    audio: state
                        .pending
                        .iter()
                        .map(|j| (j.card_id.clone(), Err(e.clone())))
                        .collect(),
                    image: HashMap::new(),
                });
                (state.episode_index, media)
            })
            .collect()
    });

    for state in &states {
        if state.pending.is_empty() {
            continue;
        }
        let media = media_by_episode.get(&state.episode_index).ok_or_else(|| {
            AppError::Ffmpeg(format!(
                "missing media extraction for episode {}",
                state.episode_label
            ))
        })?;
        info!(
            "episode {} ({} cards)",
            state.episode_label,
            state.pending.len()
        );
        let partial = commit_jobs(
            &bundle,
            &state.prepared.source_label,
            &state.pending,
            &media.audio,
            &media.image,
            state.prepared.sources.video.as_ref(),
            config,
        )?;
        merge_summary(&mut summary, partial);
    }

    if states.iter().all(|s| s.pending.is_empty()) && summary.skipped > 0 {
        info!("all {} cards already complete", summary.total);
    }

    finish_build(&bundle, config, &mut summary, &bundle_path)
}

fn run_single_build(
    ffmpeg: &FfmpegTools,
    config: &BuildConfig,
    id_prefix: Option<&str>,
) -> Result<BuildSummary> {
    let bundle_path = config.default_output();
    let bundle = Bundle::open(&bundle_path, true)?;
    let prepared = prepare_episode(ffmpeg, config, id_prefix, 0)?;
    bundle.init_build_meta(config, &prepared.source_label)?;
    bundle.store_subtitle_asset(
        &prepared.subtitle_name,
        "text/plain",
        &prepared.subtitle_bytes,
    )?;

    if config.dry_run {
        info!(
            "dry run: {} cards planned, bundle would be {}",
            prepared.plan.cards.len(),
            bundle_path.display()
        );
        return Ok(BuildSummary {
            total: prepared.plan.cards.len(),
            ..Default::default()
        });
    }

    let mut summary = BuildSummary {
        total: prepared.plan.cards.len(),
        ..Default::default()
    };

    let (pending, skipped) = filter_pending(&bundle, &prepared.plan.cards, config.resume)?;
    summary.skipped = skipped;

    if pending.is_empty() {
        info!("all {} cards already complete", summary.total);
        return finish_build(&bundle, config, &mut summary, &bundle_path);
    }

    let media = extract_media_maps(ffmpeg, config, &prepared.sources, &pending)?;
    let partial = commit_jobs(
        &bundle,
        &prepared.source_label,
        &pending,
        &media.audio,
        &media.image,
        prepared.sources.video.as_ref(),
        config,
    )?;
    merge_summary(&mut summary, partial);

    finish_build(&bundle, config, &mut summary, &bundle_path)
}

fn prepare_episode(
    ffmpeg: &FfmpegTools,
    config: &BuildConfig,
    id_prefix: Option<&str>,
    cue_index_offset: usize,
) -> Result<PreparedEpisode> {
    let sources = probe::MediaSources::discover(ffmpeg, config)?;
    let source_label = sources
        .audio
        .path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".into());

    let (subtitle_name, subtitle_bytes, subtitle_ext) = match &sources.subtitle {
        SubtitleSource::External { path, bytes } => (
            subtitle_filename(path),
            bytes.clone(),
            path.extension().and_then(|e| e.to_str()).map(String::from),
        ),
        SubtitleSource::Embedded {
            container,
            stream_index,
            codec: _,
        } => {
            let bytes = probe::demux_subtitle(ffmpeg, container, *stream_index)?;
            (
                format!(
                    "{}_embedded_{stream_index}.srt",
                    container
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("video")
                ),
                bytes,
                Some("srt".to_string()),
            )
        }
    };

    let cues = subtitles::load_from_bytes(config, &subtitle_bytes, subtitle_ext.as_deref())?;
    let plan = JobPlan::from_cues_with_options(
        &cues,
        config.screenshot_offset_ms,
        id_prefix,
        cue_index_offset,
    );

    Ok(PreparedEpisode {
        source_label,
        subtitle_name,
        subtitle_bytes,
        plan,
        sources,
    })
}

fn filter_pending(
    bundle: &Bundle,
    cards: &[CardJob],
    resume: bool,
) -> Result<(Vec<CardJob>, usize)> {
    if !resume {
        return Ok((cards.to_vec(), 0));
    }
    let mut pending = Vec::new();
    let mut skipped = 0usize;
    for job in cards {
        if bundle.has_complete_card(&job.card_id)? {
            skipped += 1;
        } else {
            pending.push(job.clone());
        }
    }
    Ok((pending, skipped))
}

fn extract_media_maps(
    ffmpeg: &FfmpegTools,
    config: &BuildConfig,
    sources: &probe::MediaSources,
    pending: &[CardJob],
) -> Result<MediaMaps> {
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

    let audio = if let Some(handle) = audio_handle {
        handle
            .join()
            .map_err(|_| AppError::Ffmpeg("audio extraction thread panicked".into()))?
    } else {
        HashMap::new()
    };

    let image = if let Some(handle) = screenshot_handle {
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
            image.len(),
            hwaccel_label.as_deref().unwrap_or("none")
        );
    }

    Ok(MediaMaps { audio, image })
}

fn commit_jobs(
    bundle: &Bundle,
    source_label: &str,
    pending: &[CardJob],
    audio_map: &HashMap<String, Result<Vec<u8>>>,
    image_map: &HashMap<String, Result<Vec<u8>>>,
    video: Option<&probe::SourceFile>,
    config: &BuildConfig,
) -> Result<BuildSummary> {
    let audio_ext = match config.audio_format {
        AudioFormat::Mp3 => "mp3",
        AudioFormat::Opus => "opus",
    };
    let image_ext = match config.image_format {
        ImageFormat::Jpg => "jpg",
        ImageFormat::Webp => "webp",
    };

    let mut summary = BuildSummary::default();
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
                        bundle.mark_card_failed(job, source_label, &e.to_string())?;
                        continue;
                    }
                    None => {
                        summary.failed += 1;
                        let msg = "missing audio extraction result".to_string();
                        summary.failures.push((job.card_id.clone(), msg.clone()));
                        bundle.mark_card_failed(job, source_label, &msg)?;
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
                        bundle.mark_card_failed(job, source_label, &e.to_string())?;
                        continue;
                    }
                    None => {
                        summary.failed += 1;
                        let msg = "missing screenshot extraction result".to_string();
                        summary.failures.push((job.card_id.clone(), msg.clone()));
                        bundle.mark_card_failed(job, source_label, &msg)?;
                        continue;
                    }
                }
            } else {
                None
            };

            records.push(bundle::build_card_record(
                job, source_label, audio_ext, image_ext, audio, image,
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

    Ok(summary)
}

fn merge_summary(total: &mut BuildSummary, partial: BuildSummary) {
    total.committed += partial.committed;
    total.failed += partial.failed;
    total.failures.extend(partial.failures);
}

fn finish_build(
    bundle: &Bundle,
    config: &BuildConfig,
    summary: &mut BuildSummary,
    bundle_path: &Path,
) -> Result<BuildSummary> {
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

    maybe_export_apkg(bundle, config)?;
    Ok(summary.clone())
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
