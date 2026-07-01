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
#[command(
    name = "content2srs",
    about = "Fast subtitle-to-Anki media engine",
    long_about = "Build SQLite deck bundles (.db) from video + subtitles, then export Anki .apkg.\n\
Supports .srt/.ass, ElevenLabs Scribe .json (Japanese SRS segmentation), season batch mode,\n\
resumable builds, and episode translation briefings.",
    version,
    after_help = "EXAMPLES:\n  \
        content2srs build -i ep.mkv -s ep.srt -o ep.db\n  \
        content2srs build -i \"season/*.mkv\" --batch -j 4 --deck-name \"Show\" -o show.db\n  \
        content2srs summary import -i show.db -f summaries.json\n  \
        content2srs export -i show.db -o show.apkg --deck-name \"Show\"\n\n\
Run `content2srs <command> --help` for command-specific options."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Increase log verbosity (-v, -vv)
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Path to ffmpeg binary (ffprobe must be alongside it or on PATH)
    #[arg(long, global = true, env = "CONTENT2SRS_FFMPEG")]
    pub ffmpeg: Option<PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Extract per-cue audio + screenshots into a SQLite bundle (.db)
    Build(BuildConfig),
    /// Export a bundle to Anki .apkg (applies episode briefings to Context field)
    Export(ExportConfig),
    /// Manage episode / deck translation briefings stored in the bundle
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
#[command(
    about = "Set an episode briefing for a source (video filename or id)",
    after_help = "EXAMPLE:\n  content2srs summary set -i show.db -s ep01.mkv \"Briefing text…\"\n  \
        echo \"Briefing…\" | content2srs summary set -i show.db -s ep01.mkv -"
)]
pub struct SetSummaryConfig {
    /// Bundle (.db) to update
    #[arg(short = 'i', long = "bundle", required = true)]
    pub bundle: PathBuf,

    /// Source id — usually the episode video filename (matches cards.source)
    #[arg(short = 's', long = "source", required = true)]
    pub source: String,

    /// Optional display label for this source
    #[arg(short = 'l', long = "label")]
    pub label: Option<String>,

    /// Briefing text, or `-` to read from stdin
    #[arg(required = true)]
    pub text: String,
}

#[derive(Debug, Parser)]
#[command(
    about = "Set a deck-wide default briefing",
    after_help = "EXAMPLE:\n  content2srs summary set-deck -i show.db \"Show-level briefing…\""
)]
pub struct SetDeckSummaryConfig {
    /// Bundle (.db) to update
    #[arg(short = 'i', long = "bundle", required = true)]
    pub bundle: PathBuf,

    /// Briefing text, or `-` to read from stdin
    #[arg(required = true)]
    pub text: String,
}

#[derive(Debug, Parser)]
#[command(
    about = "Import briefings from a JSON file",
    after_help = "JSON format: { \"deck_summary\": \"…\", \"sources\": { \"ep01.mkv\": \"…\" } }\n\
EXAMPLE:\n  content2srs summary import -i show.db -f summaries.json"
)]
pub struct ImportSummariesConfig {
    /// Bundle (.db) to update
    #[arg(short = 'i', long = "bundle", required = true)]
    pub bundle: PathBuf,

    /// Summaries JSON file (see tests/fixtures/summaries.example.json)
    #[arg(short = 'f', long = "file", required = true)]
    pub file: PathBuf,
}

#[derive(Debug, Parser)]
#[command(about = "List registered sources and briefing status")]
pub struct ListSummariesConfig {
    /// Bundle (.db) to inspect
    #[arg(short = 'i', long = "bundle", required = true)]
    pub bundle: PathBuf,
}

#[derive(Debug, Clone, Parser)]
#[command(
    about = "Build a deck bundle from video + subtitles",
    after_help = "SUBTITLE FORMATS:\n  \
        .srt / .ass — standard subtitles\n  \
        .json — ElevenLabs Scribe transcript (use --lang jpn for sentence-level SRS cues)\n\n\
EXAMPLES:\n  \
        content2srs build -i ep.mkv -s ep.srt -o ep.db --apkg ep.apkg\n  \
        content2srs build -i ep.mkv -s transcript.json --lang jpn -o ep.db\n  \
        content2srs build -i \"season/*.mkv\" --batch -j 4 -o show.db --deck-name \"Show\""
)]
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

    /// Max same-speaker pause (ms) tolerated inside one card for JSON/Japanese SRS
    /// segmentation. A gap at/above this starts a new card. Larger = fewer, longer
    /// cards (keeps sentences whole through breaths); smaller = tighter splits.
    #[arg(long = "max-gap-ms", default_value_t = 400)]
    pub max_gap_ms: u64,

    /// Context lines before/after each card
    #[arg(long, default_value_t = 3)]
    pub context_lines: usize,

    /// Regex to exclude matching subtitle lines
    #[arg(short = 'R', long = "regex")]
    pub regex_filter: Option<String>,

    /// Disable dialogue filtering (include signs/songs)
    #[arg(long = "no-filter")]
    pub no_filter: bool,

    /// Output audio codec for card clips
    #[arg(long, value_enum, default_value_t = AudioFormat::Mp3)]
    pub audio_format: AudioFormat,

    /// Audio bitrate in kbps
    #[arg(long, default_value_t = 128)]
    pub audio_bitrate_kbps: u32,

    /// Normalize audio loudness
    #[arg(long)]
    pub loudnorm: bool,

    /// Screenshot image format
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
#[command(
    about = "Export an existing bundle to .apkg",
    after_help = "Episode briefings in the bundle are composed into each card's Anki Context field.\n\
Re-run export after `summary import` — no rebuild required.\n\n\
EXAMPLE:\n  content2srs export -i show.db -o show.apkg --deck-name \"Show\""
)]
pub struct ExportConfig {
    /// Bundle (.db) to export
    #[arg(short = 'i', long = "input", required = true)]
    pub input: PathBuf,

    /// Output .apkg path
    #[arg(short = 'o', long = "output", required = true)]
    pub output: PathBuf,

    /// Anki deck / note type name
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
