# Findings

Measured on macOS 26.3 (25D125), ffmpeg/ffprobe 8.1.1 (Homebrew) and Apple Swift 6.3.3. AVFoundation was queried with [`avprobe.swift`](avprobe.swift) (`swiftc -O -o avprobe docs/avprobe.swift`). For each track it prints `nominalFrameRate`, `minFrameDuration`, `naturalTimeScale` and the format description's sample rate. It then decodes every track with `AVAssetReader` (audio to 16-bit PCM) and reports the sample count, end PTS and output rate. All fixtures were synthetic (`testsrc2` + `sine`) in a scratch directory. DaVinci Resolve was **not** tested (pending).

## 1. AAC: relabelling the container rate is not reliable, so v1 refuses it

The shipped binary refuses `mp4a`. To measure what relabelling would do, a throwaway build added `b"mp4a"` to the PCM allow-list in `src/conform.rs` (not committed). It patched `mdhd.timescale` and the 16.16 rate in the `mp4a` entry the same way as for PCM. The `esds` AudioSpecificConfig still says 48000 Hz; changing it would change its size, which G2 forbids.

```
ffmpeg -f lavfi -i testsrc2=size=64x64:rate=25:duration=10 -f lavfi -i sine=frequency=1000:sample_rate=48000:duration=10 -c:v libx264 -g 25 -c:a aac -b:a 128k src25.mov   # also .mp4 and 24 fps
cp src25.mov out25.mov && fps-swap-aac-experiment out25.mov   # 25 → 24
```

ffprobe gets the container durations right but reports the codec rate:

```
## out25.mov
stream|codec_type=video|r_frame_rate=24/1|avg_frame_rate=24/1|duration=10.416667|nb_frames=250
stream|codec_type=audio|sample_rate=48000|duration=10.416667|nb_frames=470
```

ffmpeg decoding and remuxing follow the codec rate:

```
ffmpeg -i out25.mov -map 0:a -c:a pcm_s16le dec.wav  → sample_rate=48000 duration_ts=480256 duration=10.005333 (video 10.416667)
ffmpeg -i out25.mov -c:v copy -c:a pcm_s16le remux.mov → video 10.416667, audio 48000 Hz 10.005333
ffmpeg -i out24.mp4 -c:v copy -c:a pcm_s16le remux.mov (24→25) → video 9.600000, audio 9.605979
```

AVFoundation does the same:

```
## src25.mov   track 2 soun naturalTimeScale=48000 range=0.0+10.0 asbd.rate=48000.0 | decoded samples=480000 end=10.0 outRate=48000.0
## out25.mov   track 2 soun naturalTimeScale=46080 range=0.0+10.4167 asbd.rate=48000.0 | decoded samples=481280 end=10.026666666666667 outRate=48000.0
## out24.mp4   track 2 soun naturalTimeScale=50000 range=0.0+9.6 asbd.rate=48000.0 | decoded samples=461451 end=9.6135625 outRate=48000.0
```

Both stacks decode without errors, but neither applies the 4 % retime. After 10 s the audio ends about 0.39–0.41 s before the video, and the sample counts differ between stacks. Decision: v1 refuses non-PCM audio (`unsupported audio codec 'mp4a' (only uncompressed PCM can be relabelled safely)`), and the AAC fixtures are refusal tests (SPEC §4 and §9 updated).

PCM control (shipped binary): AVFoundation's decoded duration follows the container exactly. `p_h264_25.mov` (25→24, 50 frames) gives `samples=96000 end=2.0833333333333335` = 96000/46080, matching the video. 24→25 gives `end=1.92` = 96000/50000.

## 2. Codec-level timing: ffprobe and AVFoundation follow the container

The patched files still carry the original VUI timing:

```
ffmpeg -i p_h264_25.mp4 -map 0:v -c copy -bsf:v trace_headers -f null -  → timing_info_present_flag=1, num_units_in_tick=1, time_scale=50 (=25 fps)
ffmpeg -i p_h265_25.mov -map 0:v -c copy -bsf:v trace_headers -f null -  → vui_timing_info_present_flag=1, vui_num_units_in_tick=1, vui_time_scale=25
```

Matrix: 2 s files with PCM audio, patched with the shipped binary (`p_` = patched):

```
file            | ffprobe r,avg,nb_frames | AVF nominalFrameRate minFrameDuration
h264_24.mov     | 24/1,24/1,48 | 24.0 512/12288
p_h264_24.mov   | 25/1,25/1,48 | 25.0 512/12800
h264_24.mp4     | 24/1,24/1,48 | 24.0 512/12288
p_h264_24.mp4   | 25/1,25/1,48 | 25.0 512/12800
h264_25.mov     | 25/1,25/1,50 | 25.0 512/12800
p_h264_25.mov   | 24/1,24/1,50 | 24.0 512/12288
h264_25.mp4     | 25/1,25/1,50 | 25.0 512/12800
p_h264_25.mp4   | 24/1,24/1,50 | 24.0 512/12288
h265_24.mov     | 24/1,24/1,48 | 24.0 512/12288
p_h265_24.mov   | 25/1,25/1,48 | 25.0 512/12800
h265_24.mp4     | 24/1,24/1,48 | 24.0 512/12288
p_h265_24.mp4   | 25/1,25/1,48 | 25.0 512/12800
h265_25.mov     | 25/1,25/1,50 | 25.0 512/12800
p_h265_25.mov   | 24/1,24/1,50 | 24.0 512/12288
h265_25.mp4     | 25/1,25/1,50 | 25.0 512/12800
p_h265_25.mp4   | 24/1,24/1,50 | 24.0 512/12288
prores_24.mov   | 24/1,24/1,48 | 24.0 512/12288
p_prores_24.mov | 25/1,25/1,48 | 25.0 512/12800
prores_25.mov   | 25/1,25/1,50 | 25.0 512/12800
p_prores_25.mov | 24/1,24/1,50 | 24.0 512/12288
```

ffmpeg transcodes of patched files also take the container rate: `p_h264_25.mp4` → `24/1,24/1,50`, `p_h265_24.mov` → 25/1, `p_prores_25.mov` → 24/1. `AVAssetReader` decoded every video frame without error.

One oddity, not caused by the VUI: on some longer 24→25 files, `nominalFrameRate` is 1 float32 ulp off, while `minFrameDuration` is always exact.

```
10 s H.264 mp4 → 24.999998 ; 10 s H.265 mp4 → 24.999998 ; 20 s H.264 → 25.000002 ; 2/4/6/8/60 s → 25.0 ; 10 s ProRes mov → 25.0
controls: patched + VUI rewritten to 25 (h264_metadata=tick_rate=50/1) → 24.999998 ; native 25 fps + VUI rewritten to 24 (tick_rate=48/1) → 25.0 (ffprobe 25/1)
```

The container fields of the patched 24→25 file are identical to those of a native 25 fps encode of the same length: mvhd 1000/9600, tkhd 9600, elst 9600 with media_time 1024, mdhd 12800/122880, stts 240×512. This looks like float rounding in AVFoundation's estimate (an inference, not proven). Essence was not patched. Flagged for the Resolve check.

## 3. Timecode frame 24 when going to 24 fps

Implemented as the spec says: a frame-24 label clamps to 23 with a warning, and that file's round trip is not byte-identical. Covered by `tests::timecode_label_is_kept_and_ff24_clamps_with_warning` and the integration test `timecode_frame_24_clamps_with_warning`.

```
$ fps-swap c2.mov      # ffmpeg -timecode 10:00:00:24, 25 fps
c2.mov: 25 → 24 fps, 25 frames, TC 10:00:00:24
c2.mov: warning: timecode 10:00:00:24 has frame 24, which does not exist at 24 fps; clamped to 10:00:00:23 (not reversible)
ffprobe stream_tags=timecode → 10:00:00:23
```

Pending owner confirmation.

## Other checks

Advisory lock: while another process held `flock(LOCK_EX)`, `fps-swap lk.mov` exited 1 with `refused: file is locked by another process` and the SHA-256 was unchanged. Without the lock it succeeded.

Not tested: DaVinci Resolve, SMB/NFS volumes, real camera files.
