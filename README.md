# content2srs

Fast, resilient subtitle-to-Anki media engine. Turns a video (+ optional separate audio) and subtitle file into a **single SQLite deck bundle** with per-line audio clips and screenshots stored as BLOBs, plus a one-pass `.apkg` exporter.

## Why this exists

Older tools like subs2cia spawn ~2 ffmpeg subprocesses per subtitle line (audio trim + screenshot), re-opening and re-seeking the source each time. For a season that means thousands of subprocesses and thousands of tiny media files on disk.

content2srs instead:

- **Decodes audio once** (single ffmpeg pass to PCM), slices in memory, parallel MP3 encode
- **GPU-accelerated screenshots** (`d3d11va` / `cuda` auto-detected on Windows)
- **Bounded parallelism** (`--workers`, default min(cpu, 8)) — no ffmpeg process storm
- **Incremental commits** every 96 cards (resumable mid-build)
- Stores everything in one `.db` bundle (cards + media BLOBs + original subtitles)
- Exports a ready-to-import `.apkg` without re-encoding media

## Requirements

- Rust 1.80+
- `ffmpeg` and `ffprobe` on PATH

## Install

```bash
cargo install --path .
```

Or build locally:

```bash
cargo build --release
```

## Usage

### Build a deck bundle

```bash
content2srs build \
  -i episode.mkv \
  -s episode.srt \
  -o episode.db
```

### Build bundle and export .apkg in one step

```bash
content2srs build \
  -i episode.mkv \
  -s episode.srt \
  -o episode.db \
  --apkg episode.apkg
```

### Export an existing bundle to .apkg

```bash
content2srs export -i episode.db -o episode.apkg
```

## Common options

| Flag | Description |
|------|-------------|
| `-p, --padding` | Padding around each cue (ms, default 75) |
| `-t, --merge-threshold` | Merge nearby lines (ms, default 0 = one line per card) |
| `--lang` | Prefer audio/subtitle stream by language |
| `--audio-index` / `--subtitle-index` / `--video-index` | Force stream indices |
| `--no-filter` | Include signs/songs (disable dialogue filter) |
| `-R, --regex` | Exclude lines matching regex |
| `--exact-frame` | Exact screenshot frame (slower) |
| `--hwaccel` | Video decode: `auto` (default), `none`, `d3d11va`, `cuda` |
| `--workers` | Parallel encode / screenshot batches (default: min(cpu, 8)) |
| `--no-audio` / `--no-screenshot` | Skip media types |
| `--dry-run` | Plan only, no extraction |

## Bundle format

The `.db` file is SQLite with:

- `cards` — text, timestamps, context, source, audio/image BLOBs
- `assets` — original subtitle file
- `meta` — build metadata

Share the bundle to hand off all card data and media in one file. Re-export `.apkg` anytime without re-decoding video.

## Development

```bash
cargo test
```

Integration tests generate a short `tests/fixtures/sample.mp4` via ffmpeg on first run.

## License

MIT
