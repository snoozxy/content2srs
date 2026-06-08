use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};

fn pool_default_workers() -> usize {
    crate::pool::default_workers()
}

#[derive(Debug, Clone, ValueEnum)]
pub enum AudioFormat {
    Mp3,
    Opus,
}

#[derive(Debug, Clone, ValueEnum)]
pub enum ImageFormat {
    Jpg,
    Webp,
}

#[derive(Debug, Parser)]
#[command(name = "content2srs", about = "Fast subtitle-to-Anki media engine")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    #[arg(long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    #[arg(long, global = true, env = "CONTENT2SRS_FFMPEG")]
    pub ffmpeg: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Build a deck bundle from video + subtitles
    Build(BuildConfig),
    /// Export an existing bundle to .apkg
    Export(ExportConfig),
    /// Manage episode / deck translation briefings
    Summary {
        #[command(subcommand)]
        command: SummaryCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum SummaryCommand {
    /// Set an episode briefing for a source (video filename or id)
    Set(SetSummaryConfig),
    /// Set a deck-wide default briefing
    SetDeck(SetDeckSummaryConfig),
    /// Import briefings from a JSON file
    Import(ImportSummariesConfig),
    /// List registered sources and briefing status
    List(ListSummariesConfig),
}

#[derive(Debug, Parser)]
pub struct SetSummaryConfig {
    #[arg(short = 'i', long = "bundle", required = true)]
    pub bundle: PathBuf,

    /// Source id — usually the episode video filename (matches cards.source)
    #[arg(short = 's', long = "source", required = true)]
    pub source: String,

    #[arg(short = 'l', long = "label")]
    pub label: Option<String>,

    /// Briefing text, or `-` to read from stdin
    #[arg(required = true)]
    pub text: String,
}

#[derive(Debug, Parser)]
pub struct SetDeckSummaryConfig {
    #[arg(short = 'i', long = "bundle", required = true)]
    pub bundle: PathBuf,

    /// Briefing text, or `-` to read from stdin
    #[arg(required = true)]
    pub text: String,
}

#[derive(Debug, Parser)]
pub struct ImportSummariesConfig {
    #[arg(short = 'i', long = "bundle", required = true)]
    pub bundle: PathBuf,

    #[arg(short = 'f', long = "file", required = true)]
    pub file: PathBuf,
}

#[derive(Debug, Parser)]
pub struct ListSummariesConfig {
    #[arg(short = 'i', long = "bundle", required = true)]
    pub bundle: PathBuf,
}

#[derive(Debug, Clone, Parser)]
pub struct BuildConfig {
    /// Input video/audio file(s); globs supported (e.g. `episodes/*.mkv`)
    #[arg(short = 'i', long = "input", required = true)]
    pub inputs: Vec<PathBuf>,

    /// Batch mode: group inputs into episodes (video + sidecar sub) → one combined deck
    #[arg(short = 'b', long = "batch")]
    pub batch: bool,

    /// Episodes to process in parallel (batch mode). Env: CONTENT2SRS_JOBS
    #[arg(short = 'j', long = "jobs", env = "CONTENT2SRS_JOBS")]
    pub jobs: Option<usize>,

    /// External subtitle file (optional if embedded; not used with --batch)
    #[arg(short = 's', long = "subtitle")]
    pub subtitle: Option<PathBuf>,

    /// Output bundle path (.db)
    #[arg(short = 'o', long = "output")]
    pub output: Option<PathBuf>,

    /// Also export .apkg after build
    #[arg(long)]
    pub apkg: Option<PathBuf>,

    /// Force audio stream index
    #[arg(long = "audio-index")]
    pub audio_index: Option<usize>,

    /// Force subtitle stream index (embedded)
    #[arg(long = "subtitle-index")]
    pub subtitle_index: Option<usize>,

    /// Force video stream index
    #[arg(long = "video-index")]
    pub video_index: Option<usize>,

    /// Target language (ISO 639-2/3)
    #[arg(long = "lang")]
    pub lang: Option<String>,

    /// Padding around each cue in milliseconds
    #[arg(short = 'p', long, default_value_t = 75)]
    pub padding_ms: u64,

    /// Merge cues within threshold (ms); 0 = one line per card
    #[arg(short = 't', long, default_value_t = 0)]
    pub merge_threshold_ms: u64,

    /// Context lines before/after each card
    #[arg(long, default_value_t = 3)]
    pub context_lines: usize,

    /// Regex to exclude matching subtitle lines
    #[arg(short = 'R', long = "regex")]
    pub regex_filter: Option<String>,

    /// Disable dialogue filtering (include signs/songs)
    #[arg(long = "no-filter")]
    pub no_filter: bool,

    #[arg(long, value_enum, default_value_t = AudioFormat::Mp3)]
    pub audio_format: AudioFormat,

    #[arg(long, default_value_t = 128)]
    pub audio_bitrate_kbps: u32,

    /// Normalize audio loudness
    #[arg(long)]
    pub loudnorm: bool,

    #[arg(long, value_enum, default_value_t = ImageFormat::Jpg)]
    pub image_format: ImageFormat,

    /// Screenshot offset from cue start in milliseconds
    #[arg(long, default_value_t = 0)]
    pub screenshot_offset_ms: u64,

    /// Seek to exact frame (slower) instead of nearest keyframe
    #[arg(long)]
    pub exact_frame: bool,

    /// Hardware video decode: auto, none, d3d11va, cuda, etc.
    #[arg(long, default_value = "auto")]
    pub hwaccel: String,

    /// Parallel ffmpeg / encode workers (default: min(cpu_count, 8))
    #[arg(long, default_value_t = pool_default_workers())]
    pub workers: usize,

    /// Resume an interrupted build (default: true)
    #[arg(long, default_value_t = true)]
    pub resume: bool,

    /// Dry run: plan jobs without extracting media
    #[arg(long)]
    pub dry_run: bool,

    /// Skip audio extraction
    #[arg(long)]
    pub no_audio: bool,

    /// Skip screenshot extraction
    #[arg(long)]
    pub no_screenshot: bool,

    /// Deck / note type name for apkg export
    #[arg(long, default_value = "content2srs")]
    pub deck_name: String,

    /// Import episode/deck briefings from JSON after build (see `summary import`)
    #[arg(long = "summaries")]
    pub summaries: Option<PathBuf>,
}

#[derive(Debug, Parser)]
pub struct ExportConfig {
    /// Bundle (.db) to export
    #[arg(short = 'i', long = "input", required = true)]
    pub input: PathBuf,

    /// Output .apkg path
    #[arg(short = 'o', long = "output", required = true)]
    pub output: PathBuf,

    #[arg(long, default_value = "content2srs")]
    pub deck_name: String,
}

impl BuildConfig {
    pub fn default_output(&self) -> PathBuf {
        if let Some(out) = &self.output {
            return out.clone();
        }
        if self.batch {
            let name = crate::util::sanitize_filename(&self.deck_name);
            return self
                .inputs
                .first()
                .and_then(|p| p.parent())
                .filter(|parent| !parent.as_os_str().is_empty())
                .map(|d| d.join(format!("{name}.db")))
                .unwrap_or_else(|| PathBuf::from(format!("{name}.db")));
        }
        let stem = self
            .inputs
            .first()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "deck".to_string());
        self.inputs
            .first()
            .and_then(|p| p.parent())
            .map(|d| d.join(format!("{stem}.db")))
            .unwrap_or_else(|| PathBuf::from(format!("{stem}.db")))
    }

    pub fn for_episode(
        &self,
        video: std::path::PathBuf,
        subtitle: Option<std::path::PathBuf>,
        workers: usize,
    ) -> Self {
        Self {
            inputs: vec![video],
            subtitle,
            workers,
            batch: false,
            jobs: None,
            ..self.clone()
        }
    }
}
