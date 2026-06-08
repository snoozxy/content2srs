use std::path::{Path, PathBuf};

use crate::config::BuildConfig;
use crate::errors::{AppError, Result};
use crate::ffmpeg::{FfmpegTools, ProbeResult, ProbeStream};

const BITMAP_SUB_CODECS: &[&str] = &["hdmv_pgs_subtitle", "dvd_subtitle", "dvb_subtitle"];

#[derive(Debug, Clone)]
pub struct MediaSources {
    pub video: Option<SourceFile>,
    pub audio: SourceFile,
    pub subtitle: SubtitleSource,
}

#[derive(Debug, Clone)]
pub struct SourceFile {
    pub path: PathBuf,
    pub stream_index: usize,
    pub probe: ProbeResult,
}

#[derive(Debug, Clone)]
pub enum SubtitleSource {
    External {
        path: PathBuf,
        bytes: Vec<u8>,
    },
    Embedded {
        container: PathBuf,
        stream_index: usize,
        codec: String,
    },
}

impl MediaSources {
    pub fn discover(ffmpeg: &FfmpegTools, config: &BuildConfig) -> Result<Self> {
        let mut video: Option<SourceFile> = None;
        let mut audio_candidates: Vec<(PathBuf, ProbeResult)> = Vec::new();
        let mut subtitle_embedded: Vec<(PathBuf, ProbeStream, ProbeResult)> = Vec::new();

        for input in &config.inputs {
            let probe = ffmpeg.probe_json(input).map_err(|e| AppError::Probe {
                path: input.display().to_string(),
                message: e.to_string(),
            })?;

            let streams = probe.streams.clone().unwrap_or_default();
            for stream in streams {
                let idx = stream.index.unwrap_or(0) as usize;
                match stream.codec_type.as_deref() {
                    Some("video") if video.is_none() => {
                        if config.video_index.is_none_or(|v| v == idx) {
                            video = Some(SourceFile {
                                path: input.clone(),
                                stream_index: idx,
                                probe: probe.clone(),
                            });
                        }
                    }
                    Some("audio") => {
                        if config.audio_index.is_none_or(|v| v == idx) {
                            audio_candidates.push((input.clone(), probe.clone()));
                        }
                    }
                    Some("subtitle") => {
                        if let Some(codec) = &stream.codec_name {
                            if BITMAP_SUB_CODECS.contains(&codec.as_str()) {
                                if config.subtitle.is_none() {
                                    return Err(AppError::BitmapSubtitles {
                                        codec: codec.clone(),
                                    });
                                }
                                continue;
                            }
                        }
                        if config.subtitle_index.is_none_or(|v| v == idx) {
                            subtitle_embedded.push((input.clone(), stream, probe.clone()));
                        }
                    }
                    _ => {}
                }
            }
        }

        let audio = pick_audio(&audio_candidates, config)
            .ok_or_else(|| AppError::MissingStream {
                kind: "audio".into(),
            })?;

        let subtitle = if let Some(path) = &config.subtitle {
            let bytes = std::fs::read(path).map_err(|e| AppError::Probe {
                path: path.display().to_string(),
                message: e.to_string(),
            })?;
            SubtitleSource::External { path: path.clone(), bytes }
        } else {
            let (container, stream, _) = pick_subtitle(&subtitle_embedded, config)
                .ok_or_else(|| AppError::MissingStream {
                    kind: "subtitle".into(),
                })?;
            SubtitleSource::Embedded {
                container,
                stream_index: stream.index.unwrap_or(0) as usize,
                codec: stream.codec_name.clone().unwrap_or_else(|| "unknown".into()),
            }
        };

        if video.is_none() {
            video = config.inputs.first().map(|p| {
                let probe = ffmpeg.probe_json(p).unwrap_or(ProbeResult {
                    streams: None,
                    format: None,
                });
                SourceFile {
                    path: p.clone(),
                    stream_index: 0,
                    probe,
                }
            });
        }

        Ok(MediaSources {
            video,
            audio,
            subtitle,
        })
    }
}

fn pick_audio(candidates: &[(PathBuf, ProbeResult)], config: &BuildConfig) -> Option<SourceFile> {
    if let Some(idx) = config.audio_index {
        for (path, probe) in candidates {
            if let Some(streams) = &probe.streams {
                if streams.iter().any(|s| s.index.unwrap_or(0) as usize == idx) {
                    return Some(SourceFile {
                        path: path.clone(),
                        stream_index: idx,
                        probe: probe.clone(),
                    });
                }
            }
        }
    }

    if let Some(lang) = &config.lang {
        let lang = lang.to_lowercase();
        for (path, probe) in candidates {
            if let Some(streams) = &probe.streams {
                for stream in streams {
                    if stream.codec_type.as_deref() == Some("audio")
                        && stream.language().is_some_and(|l| l.starts_with(&lang))
                    {
                        return Some(SourceFile {
                            path: path.clone(),
                            stream_index: stream.index.unwrap_or(0) as usize,
                            probe: probe.clone(),
                        });
                    }
                }
            }
        }
    }

    candidates.first().map(|(path, probe)| {
        let idx = probe
            .streams
            .as_ref()
            .and_then(|s| {
                s.iter()
                    .find(|st| st.codec_type.as_deref() == Some("audio"))
                    .and_then(|st| st.index)
            })
            .unwrap_or(0) as usize;
        SourceFile {
            path: path.clone(),
            stream_index: idx,
            probe: probe.clone(),
        }
    })
}

fn pick_subtitle(
    candidates: &[(PathBuf, ProbeStream, ProbeResult)],
    config: &BuildConfig,
) -> Option<(PathBuf, ProbeStream, ProbeResult)> {
    if let Some(idx) = config.subtitle_index {
        return candidates
            .iter()
            .find(|(_, s, _)| s.index.unwrap_or(0) as usize == idx)
            .map(|(p, s, pr)| (p.clone(), s.clone(), pr.clone()));
    }

    if let Some(lang) = &config.lang {
        let lang = lang.to_lowercase();
        if let Some(found) = candidates.iter().find(|(_, s, _)| {
            s.language().is_some_and(|l| l.starts_with(&lang)) && !s.is_forced()
        }) {
            return Some((found.0.clone(), found.1.clone(), found.2.clone()));
        }
    }

    candidates
        .iter()
        .find(|(_, s, _)| !s.is_forced())
        .or_else(|| candidates.first())
        .map(|(p, s, pr)| (p.clone(), s.clone(), pr.clone()))
}

pub fn demux_subtitle(ffmpeg: &FfmpegTools, container: &Path, stream_index: usize) -> Result<Vec<u8>> {
    let tmp = tempfile::NamedTempFile::new().map_err(|e| AppError::Ffmpeg(e.to_string()))?;
    let out = tmp.path().with_extension("srt");
    let map = format!("0:{stream_index}");

    ffmpeg
        .run(&[
            "-y",
            "-i",
            container.to_str().unwrap_or_default(),
            "-map",
            &map,
            "-c:s",
            "srt",
            out.to_str().unwrap_or_default(),
        ])
        .map_err(|e| AppError::Ffmpeg(e.to_string()))?;

    std::fs::read(&out).map_err(|e| AppError::Ffmpeg(e.to_string()))
}
