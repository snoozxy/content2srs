# content2srs

Fast, resilient subtitle-to-Anki media engine. Turns video (+ subtitles) into a **SQLite deck bundle** (`.db`) with per-cue audio clips and screenshots as BLOBs, then exports a ready-to-import **Anki `.apkg`**.

```bash
content2srs --help
content2srs build --help
content2srs export --help
content2srs summary --help
```

## Why this exists

Older tools like subs2cia spawn ~2 ffmpeg subprocesses per subtitle line. For a season that means thousands of subprocesses and thousands of tiny media files on disk.

content2srs instead:

- **Per-clip ffmpeg audio** (one short decode per cue)
- **Parallel seeked screenshots** with **GPU decode** (`d3d11va` / `cuda` on Windows)
- **Bounded parallelism** (`--workers`, default min(cpu, 8))
- **Season batch mode** (`--batch -j 4`) → one combined `.db` + `.apkg`
- **ElevenLabs Scribe `.json`** with Japanese SRS sentence segmentation (UniDic via Lindera)
- **Episode translation briefings** stored in the bundle, applied at export
- **Incremental commits** every 96 cards (resumable mid-build)

## Requirements

- Rust 1.80+
- `ffmpeg` and `ffprobe` on PATH (or `--ffmpeg` / `CONTENT2SRS_FFMPEG`)

## Install

```bash
cargo install --path .
# or
cargo build --release
```

## Quick start

### Single episode (SRT)

```bash
content2srs build \
  -i episode.mkv \
  -s episode.srt \
  -o episode.db \
  --apkg episode.apkg \
  --deck-name "Episode Name"
```

### AI transcript JSON (ElevenLabs Scribe)

Japanese transcripts use UniDic sentence-level cues (subs2cia-compatible SRS mode). Other languages use gap-based segmentation (`--merge-threshold-ms`).

```bash
content2srs build \
  -i episode.mkv \
  -s transcript.json \
  --lang jpn \
  -o episode.db
```

### Season batch (one combined deck)

Globs supported. Sidecar `.srt` / `.ass` / `.json` matched by filename stem (Plex-style `.ja.srt` suffixes stripped). Embedded subs used when no sidecar.

```bash
content2srs build \
  -i "season/*.mkv" \
  --batch -j 4 \
  -o "show_name.db" \
  --apkg "show_name.apkg" \
  --deck-name "Show Name" \
  --lang jpn
```

Batch mode namespaced card IDs (`ep01_card_00001`, …), global `cue_index` for apkg ordering, and `workers / jobs` per episode.

### Export only

```bash
content2srs export -i show.db -o show.apkg --deck-name "Show Name"
```

## Commands

| Command | Description |
|---------|-------------|
| `build` | Extract media → SQLite bundle |
| `export` | Bundle → Anki `.apkg` |
| `summary set` | Set briefing for one episode (`cards.source`) |
| `summary set-deck` | Set deck-wide default briefing |
| `summary import` | Import briefings from JSON |
| `summary list` | List sources and briefing status |

### Episode summaries

Translation briefings (from dojo-prompts / `prepend_summary.py` workflow) live in the bundle — not baked into card rows. At export they become the Anki **Context** field:

`Episode summary: {briefing} | {neighbor subtitle lines}`

```json
{
  "deck_summary": "Optional default for the whole deck",
  "sources": {
    "ep01.mkv": "Translation briefing for episode 1…",
    "ep02.mkv": "Episode 2 briefing…"
  }
}
```

See `tests/fixtures/summaries.example.json`.

```bash
# Import anytime (re-export .apkg after — no rebuild)
content2srs summary import -i show.db -f summaries.json

# Or during build
content2srs build -i ep.mkv -s ep.srt --summaries summaries.json -o show.db

# One episode (stdin with `-`)
content2srs summary set -i show.db -s ep01.mkv "Briefing text…"

content2srs summary set-deck -i show.db "Show-level briefing…"

content2srs summary list -i show.db
```

## Build options

| Flag | Description |
|------|-------------|
| `-i, --input` | Video file(s); globs OK (`season/*.mkv`) |
| `-s, --subtitle` | External `.srt` / `.ass` / `.json` (not with `--batch`) |
| `-o, --output` | Output bundle path (`.db`) |
| `--apkg` | Also export `.apkg` after build |
| `-b, --batch` | Group inputs into episodes → one deck |
| `-j, --jobs` | Parallel episodes (env: `CONTENT2SRS_JOBS`) |
| `--deck-name` | Anki deck name; batch default output stem |
| `--summaries` | Import briefings JSON after build |
| `-p, --padding-ms` | Padding around each cue in ms (default 75) |
| `-t, --merge-threshold-ms` | Merge nearby SRT lines in ms (default 0); JSON gap mode for non-Japanese |
| `--context-lines` | Neighbor lines in card context (default 3) |
| `--lang` | Prefer audio/subtitle stream; required for Japanese JSON |
| `--audio-index` / `--subtitle-index` / `--video-index` | Force stream indices |
| `--no-filter` | Include signs/songs |
| `-R, --regex` | Exclude lines matching regex |
| `--audio-format` | `mp3` (default) or `opus` |
| `--audio-bitrate-kbps` | Audio bitrate (default 128) |
| `--loudnorm` | Normalize audio loudness |
| `--image-format` | `jpg` (default) or `webp` |
| `--screenshot-offset-ms` | Screenshot offset from cue start |
| `--exact-frame` | Exact screenshot frame (slower) |
| `--hwaccel` | `auto` (default), `none`, `d3d11va`, `cuda` |
| `--workers` | Parallel workers (default min(cpu, 8)) |
| `--resume` | Resume interrupted build (default true) |
| `--dry-run` | Plan only, no extraction |
| `--no-audio` / `--no-screenshot` | Skip media types |
| `-v, --verbose` | More logging (`-vv` for trace) |
| `--ffmpeg` | Path to ffmpeg (env: `CONTENT2SRS_FFMPEG`) |

## Bundle format

SQLite `.db` with:

| Table | Contents |
|-------|----------|
| `cards` | Text, timestamps, line context, source, audio/image BLOBs |
| `sources` | Per-episode translation briefings (`id` = video filename) |
| `deck_summary` | Optional deck-wide default briefing |
| `assets` | Original subtitle files |
| `meta` | Build metadata |

Share the `.db` to hand off cards + media. Re-export `.apkg` anytime without re-encoding video.

## Development

```bash
cargo test
```

Integration tests generate `tests/fixtures/sample.mp4` via ffmpeg on first run.

## License

MIT
