use thiserror::Error;

#[derive(Debug, Clone, Error)]
pub enum AppError {
    #[error("probe failed for {path}: {message}")]
    Probe { path: String, message: String },

    #[error("no {kind} stream found in inputs")]
    MissingStream { kind: String },

    #[error("bitmap subtitles ({codec}) are not supported; use text-based .srt or .ass")]
    BitmapSubtitles { codec: String },

    #[error("subtitle parse error: {0}")]
    SubtitleParse(String),

    #[error("ffmpeg failed: {0}")]
    Ffmpeg(String),

    #[error("bundle error: {0}")]
    Bundle(String),

    #[error("apkg export error: {0}")]
    Apkg(String),

    #[error("invalid configuration: {0}")]
    Config(String),
}

pub type Result<T> = std::result::Result<T, AppError>;
