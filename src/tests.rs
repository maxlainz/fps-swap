//! Unit and property tests on hand-built byte buffers (SPEC §9).

use crate::conform::{Plan, Target, plan, scale_round};
use crate::{Movie, Refusal, parse};
use proptest::prelude::*;
use std::io::Cursor;

// ---------- builder ----------

fn bx(t: &[u8; 4], parts: &[&[u8]]) -> Vec<u8> {
    let body: Vec<u8> = parts.concat();
    let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
    v.extend_from_slice(t);
    v.extend(body);
    v
}

fn full(t: &[u8; 4], ver: u8, parts: &[&[u8]]) -> Vec<u8> {
    let vf = [ver, 0, 0, 0];
    let mut p: Vec<&[u8]> = vec![&vf];
    p.extend_from_slice(parts);
    bx(t, &p)
}

fn u32b(v: u64) -> Vec<u8> {
    (v as u32).to_be_bytes().to_vec()
}
fn u64b(v: u64) -> Vec<u8> {
    v.to_be_bytes().to_vec()
}
/// 32-bit (v0) or 64-bit (v1) field.
fn w(v1: bool, v: u64) -> Vec<u8> {
    if v1 { u64b(v) } else { u32b(v) }
}

#[derive(Clone)]
struct T {
    handler: [u8; 4],
    timescale: u64,
    stts: Vec<(u32, u32)>,
    /// Raw sample entry (built with `video_entry`, `sound_entry`, `tmcd_entry`).
    entry: Vec<u8>,
    sample_size: u32,
    /// Data of each sample; only used for `tmcd`.
    tc: Option<u32>,
    /// Extra boxes appended to `stbl` (e.g. a `ctts`).
    stbl_extra: Vec<u8>,
}

#[derive(Clone)]
struct M {
    timescale: u64,
    v1: bool,
    tracks: Vec<T>,
    moov_first: bool,
    large_mdat: bool,
}

fn video_entry(fmt: &[u8; 4]) -> Vec<u8> {
    bx(fmt, &[&[0; 78]])
}

fn sound_entry(fmt: &[u8; 4], rate: u32) -> Vec<u8> {
    let mut b = vec![0u8; 6];
    b.extend([0, 1]); // data reference index
    b.extend([0; 8]); // version 0, revision, vendor
    b.extend([0, 2, 0, 16, 0, 0, 0, 0]); // channels, bits, compression id, packet size
    b.extend((rate << 16).to_be_bytes());
    bx(fmt, &[&b])
}

fn sound_entry_v2(rate: f64) -> Vec<u8> {
    let mut b = vec![0u8; 6];
    b.extend([0, 1, 0, 2, 0, 0, 0, 0, 0, 0]); // dref, version 2, revision, vendor
    b.extend([0, 3, 0, 16, 0xff, 0xfe, 0, 0]);
    b.extend(0x0001_0000u32.to_be_bytes());
    b.extend(72u32.to_be_bytes());
    b.extend(rate.to_bits().to_be_bytes());
    b.extend([
        0, 0, 0, 2, 0x7f, 0, 0, 0, 0, 0, 0, 16, 0, 0, 0, 0xc, 0, 0, 0, 4, 0, 0, 0, 1,
    ]);
    bx(b"lpcm", &[&b])
}

fn tmcd_entry(flags: u32, ts: u32, fd: u32, nf: u8) -> Vec<u8> {
    let mut b = vec![0u8; 6];
    b.extend([0, 1, 0, 0, 0, 0]);
    b.extend(flags.to_be_bytes());
    b.extend(ts.to_be_bytes());
    b.extend(fd.to_be_bytes());
    b.extend([nf, 0]);
    bx(b"tmcd", &[&b])
}

fn video(ts: u64, delta: u32, n: u32) -> T {
    T {
        handler: *b"vide",
        timescale: ts,
        stts: vec![(n, delta)],
        entry: video_entry(b"avc1"),
        sample_size: 1,
        tc: None,
        stbl_extra: Vec::new(),
    }
}

fn pcm(rate: u32, n: u32) -> T {
    T {
        handler: *b"soun",
        timescale: u64::from(rate),
        stts: vec![(n, 1)],
        entry: sound_entry(b"sowt", rate),
        sample_size: 1,
        tc: None,
        stbl_extra: Vec::new(),
    }
}

fn tmcd(fps: u32, frame: u32) -> T {
    T {
        handler: *b"tmcd",
        timescale: u64::from(fps) * 100,
        stts: vec![(1, 100)],
        entry: tmcd_entry(0, fps * 100, 100, fps as u8),
        sample_size: 4,
        tc: Some(frame),
        stbl_extra: Vec::new(),
    }
}

fn movie(tracks: Vec<T>) -> M {
    M {
        timescale: 1000,
        v1: false,
        tracks,
        moov_first: true,
        large_mdat: false,
    }
}

fn samples(t: &T) -> u64 {
    t.stts.iter().map(|e| u64::from(e.0)).sum()
}

fn media_dur(t: &T) -> u64 {
    t.stts.iter().map(|e| u64::from(e.0) * u64::from(e.1)).sum()
}

fn build_moov(m: &M, chunk_offs: &[u64]) -> Vec<u8> {
    let v = u8::from(m.v1);
    let movie_dur = |t: &T| media_dur(t) * m.timescale / t.timescale;
    let mdur = m.tracks.iter().map(movie_dur).max().unwrap_or(0);
    let dates = w(m.v1, 0);
    let mvhd = full(
        b"mvhd",
        v,
        &[&dates, &dates, &u32b(m.timescale), &w(m.v1, mdur), &[0; 80]],
    );
    let mut parts = vec![mvhd];
    for (i, t) in m.tracks.iter().enumerate() {
        let id = u32b(i as u64 + 1);
        let tkhd = full(
            b"tkhd",
            v,
            &[
                &dates,
                &dates,
                &id,
                &[0; 4],
                &w(m.v1, movie_dur(t)),
                &[0; 60],
            ],
        );
        let elst = full(
            b"elst",
            v,
            &[&u32b(1), &w(m.v1, movie_dur(t)), &w(m.v1, 0), &[0, 1, 0, 0]],
        );
        let mdhd = full(
            b"mdhd",
            v,
            &[
                &dates,
                &dates,
                &u32b(t.timescale),
                &w(m.v1, media_dur(t)),
                &[0; 4],
            ],
        );
        let hdlr = full(b"hdlr", 0, &[b"mhlr", &t.handler, &[0; 13]]);
        let stsd = full(b"stsd", 0, &[&u32b(1), &t.entry]);
        let mut stts_rows = u32b(t.stts.len() as u64);
        for &(c, d) in &t.stts {
            stts_rows.extend(c.to_be_bytes());
            stts_rows.extend(d.to_be_bytes());
        }
        let stts = full(b"stts", 0, &[&stts_rows]);
        let n = samples(t);
        let stsc = full(b"stsc", 0, &[&u32b(1), &u32b(1), &u32b(n), &u32b(1)]);
        let stsz = full(b"stsz", 0, &[&u32b(u64::from(t.sample_size)), &u32b(n)]);
        let off = chunk_offs.get(i).copied().unwrap_or(0);
        let stco = if off > u64::from(u32::MAX) {
            full(b"co64", 0, &[&u32b(1), &u64b(off)])
        } else {
            full(b"stco", 0, &[&u32b(1), &u32b(off)])
        };
        let stbl = bx(b"stbl", &[&stsd, &stts, &stsc, &stsz, &stco, &t.stbl_extra]);
        let dinf = bx(b"dinf", &[]);
        let minf = bx(b"minf", &[&dinf, &stbl]);
        let mdia = bx(b"mdia", &[&mdhd, &hdlr, &minf]);
        parts.push(bx(b"trak", &[&tkhd, &bx(b"edts", &[&elst]), &mdia]));
    }
    let refs: Vec<&[u8]> = parts.iter().map(|p| p.as_slice()).collect();
    bx(b"moov", &refs)
}

fn build(m: &M) -> Vec<u8> {
    let ftyp = bx(b"ftyp", &[b"qt  ", &[0; 4], b"qt  "]);
    // mdat payload: each track's samples back to back.
    let mut payload = Vec::new();
    let mut rel = Vec::new();
    for t in &m.tracks {
        rel.push(payload.len() as u64);
        match t.tc {
            Some(f) => payload.extend(f.to_be_bytes()),
            None => payload.extend(vec![0xAB; (samples(t) * u64::from(t.sample_size)) as usize]),
        }
    }
    let mdat_hdr = if m.large_mdat {
        let mut h = 1u32.to_be_bytes().to_vec();
        h.extend(b"mdat");
        h.extend((payload.len() as u64 + 16).to_be_bytes());
        h
    } else {
        bx(b"mdat", &[&payload])[..8].to_vec()
    };
    let moov_len = build_moov(m, &[]).len() as u64;
    let data_start = if m.moov_first {
        ftyp.len() as u64 + moov_len + mdat_hdr.len() as u64
    } else {
        ftyp.len() as u64 + mdat_hdr.len() as u64
    };
    let offs: Vec<u64> = rel.iter().map(|r| r + data_start).collect();
    let moov = build_moov(m, &offs);
    let mdat = [mdat_hdr, payload].concat();
    if m.moov_first {
        [ftyp, moov, mdat].concat()
    } else {
        [ftyp, mdat, moov].concat()
    }
}

fn parse_buf(b: &[u8]) -> Result<Movie, Refusal> {
    parse(&mut Cursor::new(b), b.len() as u64)
}

fn plan_buf(b: &[u8], t: Target) -> Result<Plan, Refusal> {
    plan(&parse_buf(b)?, t)
}

fn apply(b: &[u8], p: &Plan) -> Vec<u8> {
    let mut o = b.to_vec();
    for x in &p.patches {
        let s = x.off as usize;
        o[s..s + x.bytes.len()].copy_from_slice(&x.bytes);
    }
    o
}

fn swap(b: &[u8]) -> Vec<u8> {
    apply(b, &plan_buf(b, Target::Toggle).expect("plan"))
}

fn refusal(b: &[u8]) -> String {
    plan_buf(b, Target::Toggle).expect_err("should refuse").0
}

// ---------- parsing ----------

#[test]
fn parses_v0_and_v1_moov_front_and_back_with_64bit_mdat() {
    for v1 in [false, true] {
        for moov_first in [false, true] {
            for large_mdat in [false, true] {
                let mut m = movie(vec![
                    video(12800, 512, 17),
                    pcm(48000, 100),
                    tmcd(25, 90000),
                ]);
                (m.v1, m.moov_first, m.large_mdat) = (v1, moov_first, large_mdat);
                let b = build(&m);
                let mv = parse_buf(&b).expect("parse");
                assert_eq!(mv.timescale.value, 1000);
                assert_eq!(mv.duration.width, if v1 { 8 } else { 4 });
                assert_eq!(mv.tracks.len(), 3);
                assert_eq!(mv.tracks[0].timescale.value, 12800);
                assert_eq!(mv.tracks[0].sample_count, 17);
                assert_eq!(mv.tracks[0].tkhd_duration.value, 680);
                assert_eq!(mv.tracks[0].elst[0].value, 680);
                assert_eq!(&mv.tracks[2].handler, b"tmcd");
                assert_eq!(mv.tracks[2].tc_samples[0].1, 90000);
                let p = plan(&mv, Target::Toggle).expect("plan");
                assert_eq!((p.from, p.to, p.frames), (25, 24, 17));
                assert_eq!(p.tc_before.as_deref(), Some("01:00:00:00"));
            }
        }
    }
}

#[test]
fn box_size_zero_extends_to_end_of_file() {
    let m = movie(vec![video(25, 1, 9)]);
    let mut b = build(&m);
    // moov is last when moov_first == false; with moov first, mdat is last: make its size 0.
    let mdat = b.windows(4).rposition(|w| w == b"mdat").expect("mdat") - 4;
    b[mdat..mdat + 4].copy_from_slice(&[0; 4]);
    assert_eq!(plan_buf(&b, Target::Toggle).expect("plan").to, 24);
}

#[test]
fn rejects_garbage_truncation_and_missing_moov() {
    assert_eq!(
        parse_buf(b"hello world, not a movie").unwrap_err().0,
        "not a MOV/MP4 file"
    );
    assert!(parse_buf(&[]).unwrap_err().0.contains("no 'moov'"));
    let mdat_only = bx(b"mdat", &[&[0; 32]]);
    assert!(parse_buf(&mdat_only).unwrap_err().0.contains("no 'moov'"));
    let b = build(&movie(vec![video(25, 1, 9)]));
    for cut in [1, 7, 40, b.len() / 2, b.len() - 1] {
        assert!(parse_buf(&b[..b.len() - cut]).is_err(), "cut {cut}");
    }
}

#[test]
fn rejects_fragmented_and_compressed() {
    let b = build(&movie(vec![video(25, 1, 9)]));
    let mut frag = b.clone();
    frag.extend(bx(b"moof", &[&[0; 8]]));
    assert!(refusal(&frag).contains("fragmented"));
    let cmov = [
        bx(b"ftyp", &[b"qt  "]),
        bx(b"moov", &[&bx(b"cmov", &[&[0; 8]])]),
    ]
    .concat();
    assert!(refusal(&cmov).contains("compressed"));
    let mvex = [bx(b"ftyp", &[b"qt  "]), bx(b"moov", &[&bx(b"mvex", &[])])].concat();
    assert!(refusal(&mvex).contains("fragmented"));
}

// ---------- rate checks ----------

#[test]
fn rate_refusals() {
    let r = |tracks| refusal(&build(&movie(tracks)));
    assert!(r(vec![video(24000, 1001, 9)]).contains("23.976"));
    assert!(r(vec![video(30000, 1001, 9)]).contains("29.970"));
    assert!(r(vec![video(30, 1, 9)]).contains("30.000"));
    assert!(r(vec![video(50, 1, 9)]).contains("not exactly 24 or 25"));
    let mut vfr = video(12800, 512, 9);
    vfr.stts = vec![(5, 512), (4, 500)];
    assert!(r(vec![vfr]).contains("non-constant"));
    assert!(r(vec![pcm(48000, 10)]).contains("no video"));
    assert!(r(vec![video(25, 1, 9), video(24, 1, 9)]).contains("different rates"));
    // 44100 * 25/24 is not an integer.
    let p = plan_buf(
        &build(&movie(vec![video(24, 1, 9), pcm(44100, 10)])),
        Target::Toggle,
    );
    assert!(p.unwrap_err().0.contains("cannot be scaled"));
}

#[test]
fn same_rate_video_tracks_are_fine_and_noop_target() {
    let b = build(&movie(vec![video(25, 1, 9), video(12800, 512, 9)]));
    let p = plan_buf(&b, Target::To(25)).expect("plan");
    assert!(p.patches.is_empty());
    assert_eq!(plan_buf(&b, Target::To(24)).expect("plan").to, 24);
}

// ---------- patch computation ----------

#[test]
fn strategy_a_scales_every_timescale_and_round_trips() {
    let b = build(&movie(vec![
        video(12800, 512, 17),
        pcm(48000, 100),
        tmcd(25, 90000),
    ]));
    let p = plan_buf(&b, Target::Toggle).expect("plan");
    assert!(!p.movie_rescaled);
    let once = apply(&b, &p);
    assert_eq!(once.len(), b.len());
    let m = parse_buf(&once).expect("parse");
    assert_eq!(m.timescale.value, 960);
    let ts: Vec<u64> = m.tracks.iter().map(|t| t.timescale.value).collect();
    assert_eq!(ts, [12288, 46080, 2400]);
    assert_eq!(
        &m.tracks[1].entries[0].bytes[32..34],
        &46080u16.to_be_bytes()
    );
    assert_eq!(m.tracks[2].entries[0].bytes[32], 24);
    assert_eq!(m.tracks[2].tc_samples[0].1, 86400);
    let back = swap(&once);
    assert_eq!(back, b, "25 -> 24 -> 25 must be byte-identical (G6)");
}

#[test]
fn strategy_b_rescales_movie_durations() {
    // 24 fps, movie timescale 1000: 1000 * 25/24 is not an integer.
    for v1 in [false, true] {
        let mut m = movie(vec![video(24, 1, 17), pcm(48000, 100)]);
        m.v1 = v1;
        let b = build(&m);
        let p = plan_buf(&b, Target::Toggle).expect("plan");
        assert!(p.movie_rescaled);
        let out = parse_buf(&apply(&b, &p)).expect("parse");
        assert_eq!(out.timescale.value, 1000);
        assert_eq!(out.tracks[0].timescale.value, 25);
        // 708 ms at 24 fps -> 17/25 s = 680 ms.
        assert_eq!(out.tracks[0].tkhd_duration.value, 680);
        assert_eq!(out.tracks[0].elst[0].value, 680);
        assert_eq!(out.tracks[1].timescale.value, 50000);
    }
}

#[test]
fn strategy_b_overflow_on_v0_is_refused() {
    let mut m = movie(vec![video(25, 1, 9)]);
    m.timescale = 1001; // forces Strategy B at 25 -> 24
    let mut b = build(&m);
    let mv = parse_buf(&b).expect("parse");
    let off = mv.duration.off as usize;
    b[off..off + 4].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(refusal(&b).contains("overflow"));
}

#[test]
fn audio_v2_double_rate_is_patched() {
    let mut a = pcm(48000, 10);
    a.entry = sound_entry_v2(48000.0);
    let b = build(&movie(vec![video(25, 1, 9), a]));
    let m = parse_buf(&swap(&b)).expect("parse");
    let e = &m.tracks[1].entries[0].bytes;
    assert_eq!(
        f64::from_bits(u64::from_be_bytes(e[40..48].try_into().unwrap())),
        46080.0
    );
}

#[test]
fn audio_refusals() {
    let mut aac = pcm(48000, 10);
    aac.entry = sound_entry(b"mp4a", 48000);
    assert!(refusal(&build(&movie(vec![video(25, 1, 9), aac]))).contains("'mp4a'"));
    // 64800 * 25/24 = 67500 does not fit the 16.16 field.
    let big = pcm(64800, 10);
    assert!(refusal(&build(&movie(vec![video(24, 1, 9), big]))).contains("overflow"));
}

#[test]
fn timecode_label_is_kept_and_ff24_clamps_with_warning() {
    // 10:00:00:24 at 25 fps.
    let f = (10 * 3600) * 25 + 24;
    let b = build(&movie(vec![video(25, 1, 9), tmcd(25, f)]));
    let p = plan_buf(&b, Target::Toggle).expect("plan");
    assert_eq!(p.tc_before.as_deref(), Some("10:00:00:24"));
    assert_eq!(p.tc_after.as_deref(), Some("10:00:00:23"));
    assert_eq!(p.warnings.len(), 1);
    assert!(p.warnings[0].contains("clamped"));
    // Going up never clamps: 10:00:00:23 at 24 -> 10:00:00:23 at 25.
    let p2 = plan_buf(&apply(&b, &p), Target::Toggle).expect("plan");
    assert_eq!(p2.tc_after.as_deref(), Some("10:00:00:23"));
    assert!(p2.warnings.is_empty());
}

#[test]
fn timecode_refusals() {
    let mut df = tmcd(25, 0);
    df.entry = tmcd_entry(1, 2500, 100, 25);
    assert!(refusal(&build(&movie(vec![video(25, 1, 9), df]))).contains("drop-frame"));
    let mut fmt = tmcd(25, 0);
    fmt.entry = bx(b"xtcd", &[&tmcd_entry(0, 2500, 100, 25)[8..]]);
    assert!(refusal(&build(&movie(vec![video(25, 1, 9), fmt]))).contains("timecode format"));
}

#[test]
fn patches_are_sorted_disjoint_and_fixed_width() {
    let b = build(&movie(vec![
        video(12800, 512, 17),
        pcm(48000, 100),
        tmcd(25, 90000),
    ]));
    let p = plan_buf(&b, Target::Toggle).expect("plan");
    assert!(
        p.patches
            .windows(2)
            .all(|w| w[0].off + w[0].bytes.len() as u64 <= w[1].off)
    );
    assert!(p.patches.iter().all(|x| matches!(x.bytes.len(), 1 | 4 | 8)));
    assert!(p.patches.iter().map(|x| x.bytes.len()).sum::<usize>() < 100);
}

// ---------- hostile and unusual files (adversarial review) ----------

/// Offset of the size field of the `n`-th (0-based) box of type `t`.
fn box_at(b: &[u8], t: &[u8; 4], n: usize) -> usize {
    b.windows(4)
        .enumerate()
        .filter(|(_, w)| w == t)
        .nth(n)
        .expect("box")
        .0
        - 4
}

fn get32(b: &[u8], off: usize) -> u32 {
    u32::from_be_bytes(b[off..off + 4].try_into().unwrap())
}

fn set32(b: &mut [u8], off: usize, v: u32) {
    b[off..off + 4].copy_from_slice(&v.to_be_bytes());
}

/// Shrink the first `t` box to `keep` payload bytes; the rest becomes a 'free' box.
fn shrink(b: &mut [u8], t: &[u8; 4], keep: usize) {
    let s = box_at(b, t, 0);
    let old = get32(b, s) as usize;
    set32(b, s, (8 + keep) as u32);
    set32(b, s + 8 + keep, (old - 8 - keep) as u32);
    b[s + 12 + keep..s + 16 + keep].copy_from_slice(b"free");
}

/// Offset of the first sample entry of the `n`-th track's `stsd`.
fn entry_at(b: &[u8], n: usize) -> usize {
    box_at(b, b"stsd", n) + 16
}

#[test]
fn tmcd_samples_must_be_inside_mdat_and_clear_of_other_tracks() {
    // Point 1: a hostile tmcd chunk offset must never steer the 4-byte patch
    // into video essence (G1) or into moov.
    let b = build(&movie(vec![video(25, 1, 9), tmcd(25, 90000)]));
    let tc_stco = box_at(&b, b"stco", 1) + 16;
    let video_chunk = get32(&b, box_at(&b, b"stco", 0) + 16);
    let video_delta = box_at(&b, b"stts", 0) + 20;
    for target in [video_chunk + 4, video_delta as u32] {
        let mut h = b.clone();
        set32(&mut h, tc_stco, target);
        if target == video_chunk + 4 {
            // Video bytes that read as a plausible frame number (else it is not converted).
            set32(&mut h, target as usize, 90000);
        }
        let r = refusal(&h);
        assert!(r.contains("malformed timecode track 2"), "{target}: {r}");
        // Not converted (counter mode): offsets are irrelevant and nothing there is written.
        let flags = entry_at(&h, 1) + 20;
        set32(&mut h, flags, 0x8);
        let p = plan_buf(&h, Target::Toggle).expect("counter mode is converted");
        let out = apply(&h, &p);
        let t = target as usize;
        assert_eq!(out[t..t + 4], h[t..t + 4], "{target}");
        assert_eq!(
            out[video_delta..video_delta + 4],
            h[video_delta..video_delta + 4]
        );
    }
}

#[test]
fn fields_are_bounded_by_their_box() {
    // Point 2: a short box must be refused, never read or patched past its end.
    let b = build(&movie(vec![video(25, 1, 9), pcm(48000, 10)]));
    for (t, keep) in [
        (b"tkhd", 20),
        (b"mvhd", 16),
        (b"mvhd", 90), // timescale and duration fit, the QuickTime times do not
        (b"mdhd", 16),
        (b"stsz", 4),
    ] {
        let mut h = b.clone();
        shrink(&mut h, t, keep);
        let r = refusal(&h);
        let want = format!("'{}' box too short", std::str::from_utf8(t).unwrap());
        assert!(r.contains(&want), "{want}: {r}");
    }
}

#[test]
fn table_counts_versions_and_chunk_extents_are_validated() {
    // Point 3.
    let mut v = video(25, 1, 9);
    v.stbl_extra = full(b"ctts", 0, &[&u32b(1), &u32b(9), &u32b(0)]);
    let b = build(&movie(vec![v, pcm(48000, 10)]));
    plan_buf(&b, Target::Toggle).expect("valid file");
    for t in [b"stco", b"stsc", b"stts", b"ctts", b"elst", b"stsz"] {
        let mut h = b.clone();
        let s = box_at(&h, t, 0);
        if t == b"stsz" {
            set32(&mut h, s + 12, 0); // variable sizes: the count now needs a table
            set32(&mut h, s + 16, 9);
        } else {
            set32(&mut h, s + 12, 0x1000_0000);
        }
        let r = refusal(&h);
        let want = format!("'{}' entry count", std::str::from_utf8(t).unwrap());
        assert!(r.contains(&want), "{want}: {r}");
    }
    let mut h = b.clone();
    h[box_at(&b, b"stsd", 0) + 8] = 2;
    assert!(refusal(&h).contains("unsupported 'stsd' version 2"));
    // Video chunk (9 bytes) starting 2 bytes before the end of the file.
    let mut h = b.clone();
    let n = h.len() as u32;
    set32(&mut h, box_at(&b, b"stco", 0) + 16, n - 2);
    assert!(refusal(&h).contains("track 1 has data outside the file"));
}

#[test]
fn pcm_rate_mismatch_only_rescales_with_warning() {
    // Points 7 and 13. ISO ipcm at 96 kHz: 16.16 field says 48000, 'srat' says 96000.
    let mut a = pcm(96000, 10);
    let mut e = sound_entry(b"ipcm", 48000);
    e.extend(full(b"srat", 0, &[&u32b(96000)]));
    let len = e.len() as u32;
    set32(&mut e, 0, len);
    a.entry = e;
    let mut qt = pcm(48000, 10);
    qt.entry = sound_entry(b"sowt", 44100);
    for (a, ts, why) in [
        (
            a,
            92160,
            "'srat' box, a sample description layout that is not supported",
        ),
        (
            qt,
            46080,
            "declared sample rate does not match the media timescale",
        ),
    ] {
        let b = build(&movie(vec![video(25, 1, 9), a]));
        let p = plan_buf(&b, Target::Toggle).expect("PCM mismatch is converted");
        assert_eq!(p.warnings.len(), 1, "{:?}", p.warnings);
        assert!(p.warnings[0].contains(why), "{}", p.warnings[0]);
        assert!(p.warnings[0].contains("audio sync is not guaranteed"));
        let (orig, out) = (parse_buf(&b).unwrap(), parse_buf(&apply(&b, &p)).unwrap());
        assert_eq!(out.tracks[1].timescale.value, ts);
        assert_eq!(
            out.tracks[1].entries[0].bytes,
            orig.tracks[1].entries[0].bytes
        );
    }
    // Compressed audio is still refused.
    let mut aac = pcm(48000, 10);
    aac.entry = sound_entry(b"mp4a", 44100);
    assert!(refusal(&build(&movie(vec![video(25, 1, 9), aac]))).contains("'mp4a'"));
}

#[test]
fn strategy_b_rescales_quicktime_mvhd_times() {
    // Point 8: 24 fps, movie timescale 1000 forces Strategy B at 24 -> 25.
    for v1 in [false, true] {
        let mut m = movie(vec![video(24, 1, 17)]);
        m.v1 = v1;
        let mut b = build(&m);
        let mv = parse_buf(&b).unwrap();
        let first = box_at(&b, b"mvhd", 0) + 8 + if v1 { 84 } else { 72 };
        assert_eq!(mv.mvhd_times[0].off as usize, first);
        for (f, v) in mv.mvhd_times.iter().zip([240, 0, 480, 1000, 24, 7]) {
            set32(&mut b, f.off as usize, v);
        }
        let p = plan_buf(&b, Target::Toggle).unwrap();
        assert!(p.movie_rescaled);
        let out = parse_buf(&apply(&b, &p)).unwrap();
        let got: Vec<u64> = out.mvhd_times.iter().map(|f| f.value).collect();
        // x 24/25, round half up; 0 stays 0.
        assert_eq!(got, [230, 0, 461, 960, 23, 7]);
    }
}

#[test]
fn round_trip_with_movie_timescale_24_is_equivalent() {
    // Point 9 / G6: 24 is not a multiple of 25, so 25 -> 24 uses Strategy B and
    // 24 -> 25 Strategy A (24 * 25/24 = 25): not byte-identical, but equivalent.
    let mut m = movie(vec![
        video(12800, 512, 17),
        pcm(48000, 100),
        tmcd(25, 90000),
    ]);
    m.timescale = 24;
    let b = build(&m);
    let p1 = plan_buf(&b, Target::Toggle).unwrap();
    assert!(p1.movie_rescaled);
    let once = apply(&b, &p1);
    let p2 = plan_buf(&once, Target::Toggle).unwrap();
    assert!(!p2.movie_rescaled);
    let back = apply(&once, &p2);
    assert_eq!(back.len(), b.len());
    let (o, r) = (parse_buf(&b).unwrap(), parse_buf(&back).unwrap());
    for (a, z) in o.tracks.iter().zip(&r.tracks) {
        assert_eq!(a.timescale.value, z.timescale.value);
        assert_eq!(a.stts, z.stts);
        assert_eq!(a.sample_count, z.sample_count);
        assert_eq!(a.tc_samples, z.tc_samples);
        let bytes = |t: &crate::atoms::Track| t.entries[0].bytes.clone();
        assert_eq!(bytes(a), bytes(z));
    }
    let tick = 1.0 / o.timescale.value.min(r.timescale.value) as f64;
    let secs = |f: &crate::atoms::Field, m: &Movie| f.value as f64 / m.timescale.value as f64;
    let pairs = std::iter::once((&o.duration, &r.duration))
        .chain(
            o.tracks
                .iter()
                .zip(&r.tracks)
                .map(|(a, z)| (&a.tkhd_duration, &z.tkhd_duration)),
        )
        .chain(
            o.tracks
                .iter()
                .zip(&r.tracks)
                .map(|(a, z)| (&a.elst[0], &z.elst[0])),
        );
    for (a, z) in pairs {
        assert!((secs(a, &o) - secs(z, &r)).abs() <= tick, "{a:?} vs {z:?}");
    }
}

#[test]
fn lock_unsupported_volume_warns_but_held_lock_refuses() {
    // Point 11. The real ENOTSUP path needs a volume without flock support.
    use std::fs::TryLockError;
    use std::io::Error;
    #[cfg(target_os = "linux")]
    let unsupported = [95, 37];
    #[cfg(not(target_os = "linux"))]
    let unsupported = [45, 102, 77];
    for code in unsupported {
        let w = crate::lock_outcome(Err(TryLockError::Error(Error::from_raw_os_error(code))))
            .expect("proceeds")
            .expect("warns");
        assert!(w.contains("not supported on this volume"), "{w}");
    }
    assert_eq!(crate::lock_outcome(Ok(())), Ok(None));
    let held = crate::lock_outcome(Err(TryLockError::WouldBlock)).unwrap_err();
    assert!(held.0.contains("locked by another process"));
    let other = crate::lock_outcome(Err(TryLockError::Error(Error::from_raw_os_error(9))));
    assert!(other.unwrap_err().0.contains("cannot lock file"));
}

#[test]
fn unusual_timecode_gets_timescale_only_with_warning() {
    // Point 12.
    let with = |entry: Vec<u8>, ts: u64, frame: u32| {
        let mut t = tmcd(25, frame);
        (t.entry, t.timescale) = (entry, ts);
        t
    };
    for (t, why) in [
        (
            with(tmcd_entry(0x8, 2500, 100, 25), 2500, 0),
            "counter mode",
        ),
        (
            with(tmcd_entry(0x4, 2500, 100, 25), 2500, 25),
            "negative timecode",
        ),
        (
            with(tmcd_entry(0, 3000, 100, 30), 3000, 108000),
            "3000/100 with 30 frames does not match 25 fps",
        ),
        (tmcd(25, 1 << 31), "2^31"),
    ] {
        let b = build(&movie(vec![video(25, 1, 9), t]));
        let p = plan_buf(&b, Target::Toggle).expect(why);
        assert_eq!(p.warnings.len(), 1, "{why}: {:?}", p.warnings);
        assert!(p.warnings[0].contains(why) && p.warnings[0].contains("not converted"));
        assert!(p.tc_before.is_none());
        let (orig, out) = (parse_buf(&b).unwrap(), parse_buf(&apply(&b, &p)).unwrap());
        let (a, z) = (&orig.tracks[1], &out.tracks[1]);
        assert_eq!(z.timescale.value, a.timescale.value * 24 / 25, "{why}");
        assert_eq!(
            z.entries[0].bytes, a.entries[0].bytes,
            "{why}: stsd untouched"
        );
        assert_eq!(z.tc_samples, a.tc_samples, "{why}: samples untouched");
    }
    // The track timescale must still scale exactly.
    let t = with(tmcd_entry(0x8, 2501, 100, 25), 2501, 0);
    assert!(refusal(&build(&movie(vec![video(25, 1, 9), t]))).contains("cannot be scaled"));
}

// ---------- property tests ----------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Strategy A: 25 -> 24 -> 25 is the identity for any valid timescales and durations.
    #[test]
    fn strategy_a_round_trip_identity(
        delta in 1u32..5000,
        n in 1u32..2000,
        audio_k in 1u32..2600, // audio timescale = 25 * k
        mv_k in 1u64..4000,    // movie timescale = 25 * k
        v1 in any::<bool>(),
        moov_first in any::<bool>(),
        frame in 0u32..(24 * 3600 * 25),
    ) {
        let mut m = movie(vec![video(25 * u64::from(delta), delta, n), pcm(25 * audio_k, 3), tmcd(25, frame - frame % 25)]);
        (m.timescale, m.v1, m.moov_first) = (25 * mv_k, v1, moov_first);
        let b = build(&m);
        let once = swap(&b);
        prop_assert_eq!(once.len(), b.len());
        prop_assert_eq!(plan_buf(&once, Target::Toggle).unwrap().from, 24);
        prop_assert_eq!(swap(&once), b);
    }

    /// Strategy B: a scale and its inverse stay within one tick.
    #[test]
    fn strategy_b_within_one_tick(v in 0u64..(u64::MAX / 64), up in any::<bool>()) {
        let (a, b) = if up { (25, 24) } else { (24, 25) };
        let there = scale_round(v, a, b, u64::MAX).unwrap();
        let back = scale_round(there, b, a, u64::MAX).unwrap();
        prop_assert!(back.abs_diff(v) <= 1);
    }

    /// Strategy B end to end on a real movie: durations come back within one tick.
    #[test]
    fn strategy_b_movie_round_trip(mv_ts in 1u64..100_000, n in 1u32..5000, v1 in any::<bool>()) {
        prop_assume!(mv_ts % 25 != 0 && mv_ts % 24 != 0);
        let mut m = movie(vec![video(25, 1, n)]);
        (m.timescale, m.v1) = (mv_ts, v1);
        let b = build(&m);
        let orig = parse_buf(&b).unwrap();
        let back = parse_buf(&swap(&swap(&b))).unwrap();
        prop_assert_eq!(back.timescale.value, orig.timescale.value);
        prop_assert!(back.duration.value.abs_diff(orig.duration.value) <= 1);
        prop_assert!(back.tracks[0].tkhd_duration.value.abs_diff(orig.tracks[0].tkhd_duration.value) <= 1);
    }

    /// No input bytes make the parser or planner panic.
    #[test]
    fn random_bytes_never_panic(b in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = plan_buf(&b, Target::Toggle);
    }

    /// Nor do mutations and truncations of a valid file.
    #[test]
    fn mutated_files_never_panic(
        flips in proptest::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..8),
        cut in any::<prop::sample::Index>(),
    ) {
        let mut b = build(&movie(vec![video(12800, 512, 17), pcm(48000, 20), tmcd(25, 90000)]));
        for (i, v) in flips {
            let i = i.index(b.len());
            b[i] = v;
        }
        let _ = plan_buf(&b, Target::Toggle);
        let n = cut.index(b.len());
        let _ = plan_buf(&b[..n], Target::Toggle);
    }
}
