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
