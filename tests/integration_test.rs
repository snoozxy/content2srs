use std::path::PathBuf;
use std::process::Command;

use content2srs::bundle::Bundle;

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn ensure_sample_video() -> PathBuf {
    let path = fixture_path("sample.mp4");
    if path.exists() {
        return path;
    }
    std::fs::create_dir_all(fixture_path(".").parent().unwrap()).ok();
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "lavfi",
            "-i",
            "testsrc=size=320x240:rate=24:duration=10",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=10",
            "-c:v",
            "libx264",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
            path.to_str().unwrap(),
        ])
        .status()
        .expect("ffmpeg required for integration tests");
    assert!(status.success(), "failed to generate sample.mp4 fixture");
    path
}

#[test]
fn bundle_resume_and_export_roundtrip() {
    let video = ensure_sample_video();
    let subs = fixture_path("sample.srt");
    let dir = tempfile::tempdir().unwrap();
    let bundle_path = dir.path().join("test.db");
    let apkg_path = dir.path().join("test.apkg");

    let bin = env!("CARGO_BIN_EXE_content2srs");
    let base_args = [
        "build",
        "-i",
        video.to_str().unwrap(),
        "-s",
        subs.to_str().unwrap(),
        "-o",
        bundle_path.to_str().unwrap(),
        "--no-filter",
    ];

    let first = Command::new(bin).args(base_args).output().unwrap();
    assert!(
        first.status.success(),
        "first build failed: stdout={} stderr={}",
        String::from_utf8_lossy(&first.stdout),
        String::from_utf8_lossy(&first.stderr)
    );

    let bundle = Bundle::open(&bundle_path, false).unwrap();
    let cards = bundle.list_complete_cards().unwrap();
    assert_eq!(cards.len(), 4);

    let second = Command::new(bin).args(base_args).output().unwrap();
    assert!(second.status.success());

    let export = Command::new(bin)
        .args([
            "export",
            "-i",
            bundle_path.to_str().unwrap(),
            "-o",
            apkg_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        export.status.success(),
        "export failed: {}",
        String::from_utf8_lossy(&export.stderr)
    );
    assert!(apkg_path.exists());

    let file = std::fs::File::open(&apkg_path).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    assert!(archive.by_name("collection.anki2").is_ok());
    assert!(archive.by_name("media").is_ok());
}
