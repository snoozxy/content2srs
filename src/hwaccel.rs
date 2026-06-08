use tracing::info;

use crate::ffmpeg::FfmpegTools;

const CANDIDATES: &[&str] = if cfg!(windows) {
    &["d3d11va", "cuda", "dxva2"]
} else if cfg!(target_os = "macos") {
    &["videotoolbox"]
} else {
    &["cuda", "vaapi"]
};

pub fn resolve_hwaccel(ffmpeg: &FfmpegTools, explicit: Option<&str>) -> Option<String> {
    if let Some(v) = explicit {
        if v == "none" {
            return None;
        }
        if v == "auto" {
            return detect(ffmpeg);
        }
        return Some(v.to_string());
    }
    detect(ffmpeg)
}

fn detect(ffmpeg: &FfmpegTools) -> Option<String> {
    for candidate in CANDIDATES {
        if ffmpeg.hwaccel_available(candidate) {
            info!("using hardware video decode: {candidate}");
            return Some((*candidate).to_string());
        }
    }
    None
}
