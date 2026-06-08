use anyhow::Result;
use clap::Parser;
use tracing::Level;
use tracing_subscriber::EnvFilter;

use content2srs::config::{Cli, Command};
use content2srs::ffmpeg::FfmpegTools;
use content2srs::pipeline;

fn main() -> Result<()> {
    let cli = Cli::parse();
    init_logging(cli.verbose);

    let ffmpeg = FfmpegTools::discover(cli.ffmpeg)?;

    match cli.command {
        Command::Build(config) => {
            let summary = pipeline::run_build(&ffmpeg, &config)?;
            if summary.failed > 0 {
                std::process::exit(1);
            }
        }
        Command::Export(config) => {
            pipeline::run_export(&config.input, &config.output, &config.deck_name)?;
        }
    }

    Ok(())
}

fn init_logging(verbosity: u8) {
    let default_level = match verbosity {
        0 => Level::INFO,
        1 => Level::DEBUG,
        _ => Level::TRACE,
    };
    let filter = EnvFilter::builder()
        .with_default_directive(default_level.into())
        .from_env_lossy();
    tracing_subscriber::fmt().with_env_filter(filter).init();
}
