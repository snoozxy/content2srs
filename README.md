# content2srs

Fast, resilient subtitle-to-Anki media engine. Turns a video (+ optional separate audio) and subtitle file into a **single SQLite deck bundle** with per-line audio clips and screenshots stored as BLOBs, plus a one-pass `.apkg` exporter.

## Why this exists

Older tools like subs2cia spawn ~2 ffmpeg subprocesses per subtitle line (audio trim + screenshot), re-opening and re-seeking the source each time. For a season that means thousands of subprocesses and thousands of tiny media files on disk.

content2srs instead:

- **Per-clip ffmpeg audio** (one short decode per cue, same approach as subs2cia)
- **Parallel seeked screenshots** with **GPU decode** (`d3d11va` / `cuda` auto-detected on Windows)
- **Bounded parallelism** (`--workers`, default min(cpu, 8)) — no unbounded ffmpeg storm
- **Season batch mode** (`--batch -j 4`) — N episodes in parallel → one combined `.db` + `.apkg`
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

### Build from an AI transcript JSON (ElevenLabs Scribe)

Japanese transcripts use UniDic (via Lindera) for sentence-level cues, matching subs2cia's SRS export. Non-Japanese JSON uses gap-based segmentation (`--merge-threshold`).

```bash
content2srs build \
  -i episode.mkv \
  -s transcript.json \
  --lang jpn \
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

### Batch build a season (one combined deck)

Globs are supported. Sidecar `.srt`/`.ass`/`.json` files are matched by filename stem (Plex-style language suffixes like `.ja.srt` are stripped). Embedded subtitles are used when no sidecar exists.

```bash
content2srs build \
  -i "season/*.mkv" \
  --batch -j 4 \
  -o "show_name.db" \
  --apkg "show_name.apkg" \
  --deck-name "Show Name" \
  --lang jpn
```

In batch mode, card IDs are namespaced per episode (`ep01_card_00001`, …) and `cue_index` is global across the season for stable apkg ordering. `--workers` is split across parallel episodes (`workers / jobs` per episode).

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
| `-b, --batch` | Group inputs into episodes → one combined deck |
| `-j, --jobs` | Episodes in parallel (batch mode; env `CONTENT2SRS_JOBS`) |
| `--deck-name` | Anki deck name (also default output stem in batch mode) |
| `--no-audio` / `--no-screenshot` | Skip media types |
| `--dry-run` | Plan only, no extraction |

## Bundle format

The `.db` file is SQLite with:

- `cards` — text, timestamps, line context, source, audio/image BLOBs
- `sources` — per-episode translation briefings (keyed by `cards.source`, usually the video filename)
- `deck_summary` — optional deck-wide default briefing
- `assets` — original subtitle file
- `meta` — build metadata

Line context (neighbor subtitle lines) stays on each card. Episode briefings live separately and are composed into the Anki **Context** field at export time (`Episode summary: … | …`), matching the old `prepend_summary.py` workflow without mutating card rows.

### Episode summaries

JSON format (`summaries.example.json` in `tests/fixtures/`):

```json
{
  "deck_summary": "Optional default for the whole deck",
  "sources": {
    "ep01.mkv": "Translation briefing for episode 1…",
    "ep02.mkv": "Episode 2 briefing…"
  }
}
```

Import during build or anytime after:

```bash
# After build
content2srs summary import -i show.db -f summaries.json

# Or inline with build
content2srs build -i ep.mkv -s ep.json --summaries summaries.json -o show.db

# Set one episode (use `-` for stdin)
content2srs summary set -i show.db -s ep01.mkv "Briefing text…"

# Deck-wide default
content2srs summary set-deck -i show.db "Show-level briefing…"

# List sources
content2srs summary list -i show.db
```

Re-export `.apkg` after updating briefings — no rebuild required.

Share the bundle to hand off all card data and media in one file. Re-export `.apkg` anytime without re-decoding video.

## Development

```bash
cargo test
```

Integration tests generate a short `tests/fixtures/sample.mp4` via ffmpeg on first run.

## License

MIT
