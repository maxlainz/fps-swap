# fps-swap

[![CI](https://github.com/maxlainz/fps-swap/actions/workflows/ci.yml/badge.svg)](https://github.com/maxlainz/fps-swap/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/maxlainz/fps-swap)](https://github.com/maxlainz/fps-swap/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A macOS command-line tool and Finder Quick Action that conforms QuickTime and MP4 files between exactly 24 and 25 fps, in place, without re-encoding.

Conforming is the standard way to move material between 24 and 25 fps without dropping or duplicating frames: every frame is kept and only the playback rate changes. The picture is unchanged, the duration changes by 25/24 or 24/25, and PCM audio is retimed with it, so it stays in sync but its pitch shifts by about 4 %. fps-swap does this by rewriting the container's timing fields. The video data is never decoded or rewritten, so a large ProRes file takes the same time as a small one. A typical use is to take 25 fps footage to 24 fps for a tool that only works at 24, then take the result back to 25.

## Guarantees

- **Video data is untouched.** No byte of any video sample is modified. Only container metadata changes, plus the timecode frame number when the file has a timecode track.
- **Same file size.** Each change overwrites a fixed-width field with a value of the same width. A typical run writes a few dozen bytes.
- **All or nothing.** Every check runs before anything is written. A refused file is left byte-for-byte as it was.
- **Same frame count.** Every track keeps exactly the samples it had.
- **Exact rates only.** The input must be exactly 24/1 or 25/1 with a constant frame duration; the output is exactly 25/1 or 24/1.
- **Reversible.** Running it again restores the original rate. See the exceptions below.

## Warning: files are modified in place

fps-swap writes to the file you give it. It makes no copy, temporary file or backup. Running it again undoes the change, with two exceptions:

- A timecode whose frame field is 24 (e.g. `10:00:00:24`) does not exist at 24 fps. Going to 24 fps clamps it to `:23` with a warning, and going back does not restore `:24`.
- The round trip is byte-identical only when every timescale can be scaled exactly in both directions: multiples of 25 for a file that starts at 25 fps, multiples of 24 for one that starts at 24 fps. The usual exception is a 24 fps file with a movie timescale of 1000 (the ffmpeg default): 1000 × 25/24 is not a whole number, so going to 25 fps keeps 1000 and rescales the durations instead, and coming back changes the movie timescale to 960. The file then comes back with the same frames and durations within one tick, but not byte-identical.

If you need the original bytes, copy the file first.

## Supported and refused

Supported:

- macOS, Apple silicon and Intel (universal binary).
- Non-fragmented QuickTime `.mov` and ISO `.mp4` / `.m4v`.
- ProRes, H.264 and H.265 video. These are the tested codecs; the tool never parses the video data.
- Uncompressed PCM audio, relabelled to the new rate (48000 Hz becomes 46080 Hz at 25 → 24, 50000 Hz at 24 → 25).
- Non-drop-frame timecode tracks: the start timecode keeps its label (`01:00:00:00` reads `01:00:00:00` at the new rate).

Refused, with a one-line reason and the file left untouched:

- Any rate other than exactly 24 or 25 fps: 23.976, 29.97, 30, 50 and so on.
- Variable frame rate, or several video tracks with different rates.
- Fragmented MP4, compressed movie headers (`cmov`), and anything that is not MOV/MP4 (MXF, for example). Image sequences are not supported.
- Compressed audio such as AAC. Players and ffmpeg decode it at the rate stored in the codec configuration, not the container, so the audio would drift out of sync (about 0.4 s over 10 s; see [docs/FINDINGS.md §1](docs/FINDINGS.md)).
- Drop-frame timecode.
- Reference movies whose media lives in other files.
- Truncated or malformed files, files that are not writable, and files locked by another process.

## Install

### From the release

1. Download `fps-swap-<version>-macos-universal.zip` from the [latest release](https://github.com/maxlainz/fps-swap/releases/latest) and unzip it.
2. In Terminal, from the unzipped folder, run `./install.sh`.

No sudo is needed. The script installs the binary to `~/.local/bin/fps-swap` and the Quick Action to `~/Library/Services/`. The binary is not signed or notarized, so `install.sh` removes the download quarantine from the installed copies. If `~/.local/bin` is not in your `PATH`, the script prints the line to add to `~/.zshrc`.

### One-line install or update

```sh
curl -fsSL https://raw.githubusercontent.com/maxlainz/fps-swap/main/update.sh | bash
```

This downloads the latest release zip and runs its `install.sh`. Run the same command again to update.

### From source

Requires Rust 1.89 or later.

```sh
git clone https://github.com/maxlainz/fps-swap.git
cd fps-swap
cargo build --release
./install.sh
```

### Uninstall

Run `./uninstall.sh` from the release folder or the repository. It removes the binary and the Quick Action.

## Usage

### Finder

Select one or more movie files, right-click, and choose **Quick Actions > FPS Swap (24↔25)**. Each file is toggled (24 → 25 or 25 → 24) and you get one notification per file with the direction and frame count, or the reason it was refused.

### Command line

```
usage: fps-swap [--to 24|25] [--dry-run] [--json] <file>...
       fps-swap info [--json] <file>...
```

Without `--to`, each file toggles. `--to 24` or `--to 25` sets the target and leaves files already at that rate unchanged. `--dry-run` runs every check and reports what would change without writing. `info` shows the rate, frame count, duration, tracks and timecode.

```
$ fps-swap info clip.mov
clip.mov: 25 fps, 125 frames, 5.000 s, TC 01:00:00:00
  track 1 vide 'apcn': timescale 12800, 125 samples, 5.000 s
  track 2 soun 'in24': timescale 48000, 240000 samples, 5.000 s
  track 3 tmcd 'tmcd': timescale 12800, 1 samples, 5.000 s
clip.mov: note: 125 frames is not of the form 8n+1

$ fps-swap clip.mov
clip.mov: 25 → 24 fps, 125 frames, TC 01:00:00:00
clip.mov: note: 125 frames is not of the form 8n+1

$ fps-swap --to 24 clip.mov
clip.mov: already 24 fps, 125 frames, TC 01:00:00:00, unchanged
clip.mov: note: 125 frames is not of the form 8n+1

$ fps-swap aac.mov ntsc.mov frag.mp4
aac.mov: refused: unsupported audio codec 'mp4a' (only uncompressed PCM can be relabelled safely)
ntsc.mov: refused: video rate is 23.976 fps, not exactly 24 or 25
frag.mp4: refused: fragmented MP4 is not supported
```

The 8n+1 note is information only: several video models require a frame count of the form 8n+1 (49, 121, ...). It never causes a refusal.

`--json` prints one array with an object per file, for both modes:

```
$ fps-swap --json rt24.mp4
[{"file":"rt24.mp4","ok":true,"from":24,"to":25,"changed":true,"dry_run":false,"frames":49,"frames_8n1":true,"timecode":null,"timecode_after":null,"movie_rescaled":true,"patches":10,"bytes_written":40,"warnings":[]}]
```

A refused file gives `{"file":...,"ok":false,"error":"..."}`. Warnings (for example the timecode clamp) appear in `warnings` and, in text mode, as `<file>: warning: ...` lines.

Exit codes: `0` if every file succeeded (including files already at the target rate and repeated arguments, which are processed once), `1` if any file was refused, `2` for a usage error. Remaining files are still processed after a refusal.

## How it works

Let r = 24/25 or 25/24. fps-swap multiplies the movie timescale and every track's media timescale by r. All durations and sample tables keep their numeric values, so the whole presentation is retimed uniformly. When the movie timescale cannot be scaled exactly (1000 × 25/24, for example), it keeps that timescale and rescales the durations expressed in it instead. For PCM audio it also rewrites the sample rate in the sound description. For a timecode track it rewrites the timecode description and the frame-number sample so the start label reads the same at the new rate. Codec-level timing (H.264/H.265 VUI, ProRes headers) is deliberately left alone; ffprobe and AVFoundation follow the container ([docs/FINDINGS.md §2](docs/FINDINGS.md)).

The full design contract is in [SPEC.md](SPEC.md).

## Known limitations

- **DaVinci Resolve has not been tested.** ffprobe, ffmpeg and AVFoundation have been checked; Resolve has not.
- **Network volumes (SMB/NFS) have not been tested.** On a volume without file locking, fps-swap is designed to warn and continue without the lock, but that path is covered only by a unit test.
- **Only synthetic fixtures** generated with ffmpeg have been tested, not files from real cameras.
- Reference movies with external media are refused, even if otherwise valid.
- Unusual timecode (counter mode, negative, or not matching the video rate) and PCM whose declared rate differs from its track timescale are converted with a warning, not refused. These cases were not checked in Resolve or QuickTime.
- On some longer 24 → 25 files, AVFoundation reports `nominalFrameRate` as 24.999998 while `minFrameDuration` is exact ([docs/FINDINGS.md §2](docs/FINDINGS.md)).

## Development

```sh
cargo test                                   # unit, property and integration tests
cargo fmt --check
cargo clippy --all-targets -- -D warnings
```

The integration tests generate fixtures with ffmpeg and ffprobe. Without them, those tests print a `SKIPPED` message and pass. CI runs all three commands on macOS for every push and pull request; tagging `v*` builds the universal binary and publishes the release zip.

[SPEC.md](SPEC.md) is the source of truth for behaviour. [docs/FINDINGS.md](docs/FINDINGS.md) records measurements and decisions.

## License

[MIT](LICENSE)
