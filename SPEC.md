# fps-swap — design contract

This document is the source of truth. Code, tests and docs must agree with it.
If something here turns out to be wrong or impossible, change the spec in the
same commit and say why in the commit message.

## 1. What it does

Conforms a movie file between exactly 24 fps and exactly 25 fps by rewriting
container timing metadata in place. Same frames, same bytes of essence, a
different playback rate. Duration changes by 25/24 or 24/25.

One action that toggles: a 25 fps file becomes 24 fps, a 24 fps file becomes
25 fps. Anything else is refused and the file is left byte-for-byte untouched.

## 2. Scope (v1)

- macOS only (arm64 + x86_64 universal binary). Rust, no GPU, no decoding.
- Containers: QuickTime `.mov` and ISO BMFF `.mp4`/`.m4v`, non-fragmented.
- Video codecs: ProRes, H.264, H.265. The tool is codec-agnostic in principle
  (it never parses essence) but these three are the tested, supported set.
- No runtime dependency on ffmpeg. ffmpeg/ffprobe are test-time tools only.

Out of scope for v1: image sequences, MXF, fragmented MP4, compressed `moov`
(`cmov`), 23.976/29.97/30/50 fps, variable frame rate, padding or trimming
frames to satisfy a model's length constraints.

## 3. Hard guarantees

G1. **Essence untouched.** No byte of any video sample is modified. Verified by
    hashing the video packets before and after.
G2. **Size preserving.** The file length never changes. Every patch overwrites
    a fixed-width field with a value of the same width.
G3. **All or nothing on refusal.** Every check runs and every patch is computed
    before the first byte is written. A refused file has an identical SHA-256.
G4. **Frame count preserved.** Sample count per track is identical before and
    after.
G5. **Exact rates only.** The source video rate must be exactly 24/1 or 25/1
    with a constant sample duration. The result is exactly 25/1 or 24/1.
G6. **Round trip.** 25 → 24 → 25 yields a byte-identical file whenever every
    timescale involved is divisible as required by Strategy A (see §4), which
    covers every 25 fps file. Otherwise the round trip is frame-hash identical
    and metadata-equivalent (durations within ±1 tick).

## 4. Algorithm

Let r = new_fps / old_fps (24/25 or 25/24).

**Strategy A — scale timescales (preferred).** Multiply `mvhd.timescale` and
every track's `mdhd.timescale` by r. All durations, `stts`, `ctts`, `elst`
entries keep their numeric values, so the whole presentation is uniformly
retimed and nothing else needs touching. For the video track
timescale = fps × delta, so the product is always an integer.

**Strategy B — fallback for the movie timescale only.** If `mvhd.timescale × r`
is not an integer (e.g. 1000 × 25/24), keep `mvhd.timescale` and instead
rescale every value expressed in movie timescale by 1/r with round-half-up:
`mvhd.duration`, each `tkhd.duration`, each `elst.segment_duration`. Handle
version 0 (32-bit) and version 1 (64-bit) boxes; refuse on 32-bit overflow.

If any *track* `mdhd.timescale × r` is not an integer, refuse.

**Track-specific fixups** (in addition to the timescale change):

- *Audio* — relabel the sample rate. Patch the sample rate in the sound sample
  description (`stsd`) to match the new media timescale (48000 → 46080 at
  25→24, 48000 → 50000 at 24→25). Audio data is untouched; pitch shifts ±4 %
  and sync is exact. PCM is the supported case. For compressed audio (AAC),
  investigate what ffmpeg and AVFoundation do when the container rate differs
  from the codec config; if playback is not reliably in sync, refuse files
  with compressed audio with a clear message rather than ship a guess.
  Refuse if the new rate is not an integer or does not fit the field.
- *Timecode (`tmcd`)* — keep the start timecode label. Patch the `tmcd` sample
  description (timescale, frame duration, number of frames) and the 32-bit
  frame-number sample(s) in `mdat` so HH:MM:SS:FF reads the same in the new
  base. If FF = 24 when going to 24 fps, clamp to 23 and emit a warning (the
  one documented non-reversible case). Refuse drop-frame timecode.
- *Everything else* (text, metadata, chapters) — timescale change only.

**Deliberately not touched:** codec-level timing hints (H.264/H.265 VUI
`timing_info` in `avcC`/`hvcC` or in-band parameter sets, any ProRes frame
header field). Tests must establish whether ffprobe, AVFoundation and DaVinci
Resolve report the new rate regardless. If any of them trusts the codec-level
value over the container, document it in `docs/FINDINGS.md` and raise it; do
not silently patch essence.

## 5. Refusals (file untouched, non-zero exit, one-line reason)

Not a MOV/MP4; no `moov`; fragmented (`moof`/`mvex`); compressed `moov`; no
video track; video rate not exactly 24 or 25; non-constant sample durations;
several video tracks with different rates; non-integer scaled timescale on a
track; field overflow; drop-frame timecode; unsupported audio; file not
writable; truncated or malformed boxes (never panic, never read out of
bounds — the parser is fed untrusted files).

## 6. Writing

Open read-write, apply patches with positioned writes, `fsync`, close. Take an
exclusive advisory lock for the duration. Total bytes written are in the tens.
No temp file, no sidecar, no backup. Must work on SMB/NFS network volumes.

## 7. CLI

    fps-swap <file>...            toggle each file (24→25 or 25→24)
    fps-swap --to 24|25 <file>... explicit target; no-op if already there
    fps-swap --dry-run <file>...  report what would change, write nothing
    fps-swap info <file>...       fps, frame count, duration, tracks, timecode
    --json                        machine-readable output for all of the above

Per file, print one line: `clip.mov: 25 → 24 fps, 312 frames, TC 01:00:00:00`
plus any warnings. Always report the frame count; add a note when the count is
not of the form 8n+1, since several video models require it (information only,
never an error). Exit code 0 only if every file succeeded; process remaining
files after a failure.

## 8. Finder integration and distribution

- One Quick Action, "FPS Swap (24↔25)", for movie files in Finder, accepting a
  multi-selection. It calls the CLI and shows a macOS notification with the
  direction and frame count per file, or the refusal reason.
- `install.sh` installs the binary and the Quick Action for the current user
  without sudo and clears quarantine; `uninstall.sh` removes both.
- GitHub Actions: on push/PR run fmt, clippy (`-D warnings`) and tests on
  macOS; on a `v*` tag build the universal binary and attach a zip (binary,
  Quick Action, install scripts) to a GitHub Release.

## 9. Tests (the reason this is in Rust)

- Unit tests for box parsing and patch computation on hand-built byte buffers,
  including version 0/1 boxes, `moov` before and after `mdat`, 64-bit box sizes.
- Property tests: for random valid timescales and durations, scale then
  inverse-scale is the identity under Strategy A; Strategy B stays within
  ±1 tick; no input bytes ever make the parser panic.
- Integration tests on fixtures generated at test time with ffmpeg (skip with
  a clear message if ffmpeg is absent): ProRes/H.264/H.265 × 24/25 fps ×
  {no audio, PCM, AAC} × {with, without timecode}, `moov` at front and at end.
  For each: packet hash of the video stream identical (G1), file size
  identical (G2), ffprobe frame count identical (G4), ffprobe rate exactly
  24/1 or 25/1 (G5), start timecode label preserved, 25→24→25 SHA-256
  identical (G6).
- Refusal tests: 23.976, 29.97, 30, VFR, fragmented MP4, truncated file,
  read-only file — each leaves the SHA-256 unchanged (G3).
