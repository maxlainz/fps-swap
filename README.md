# fps-swap

Lossless, in-place 24 ↔ 25 fps conform for QuickTime/MP4 files on macOS.

Generative video models work at 24 fps; European advertising is shot and
delivered at 25 fps. Feeding 25 fps footage to a model resamples it and drops
frames, so the result no longer conforms. `fps-swap` retimes the file instead:
every frame is kept, only the playback rate changes. Run it before the model
(25 → 24) and again on the model's output (24 → 25).

- **Lossless**: the picture essence is never decoded or rewritten.
- **In place**: a few bytes of container metadata are patched; a 50 GB file
  takes milliseconds and needs no extra disk space.
- **Reversible**: running it again swaps back.
- **Right-click**: ships with a Finder Quick Action.

Status: under construction. See [SPEC.md](SPEC.md) for the design contract.

## Install

Download the latest `fps-swap-*-macos-universal.zip` from
[Releases](../../releases), unzip it and run:

    ./install.sh

This installs, for the current user and without sudo, the binary to
`~/.local/bin/fps-swap` and the Quick Action to `~/Library/Services/`, and
clears the download quarantine. `./uninstall.sh` removes both.

From a checkout: `cargo build --release && ./install.sh`.

### Install / update (internal)

To install or update to the latest release in one step (downloads the
release zip and runs its `install.sh`):

    curl -fsSL https://raw.githubusercontent.com/maxlainz/fps-swap/main/update.sh | bash

## Use

**Finder**: select one or more movie files, right-click › Quick Actions ›
**FPS Swap (24↔25)**. Each file is toggled and you get a notification with the
direction and frame count, or the reason it was refused (refused files are left
untouched).

**Terminal**:

    fps-swap clip.mov              # toggle: 25 → 24 or 24 → 25
    fps-swap --to 24 *.mov         # explicit target; no-op if already 24
    fps-swap --dry-run clip.mov    # show what would change, write nothing
    fps-swap info clip.mov         # fps, frames, duration, tracks, timecode
