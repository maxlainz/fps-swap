//! Patch computation (SPEC §4). Pure: takes a parsed [`Movie`], returns the
//! list of fixed-width overwrites. Nothing here touches the file.

use crate::Refusal;
use crate::atoms::{Field, Movie, Track, be16, be32, be64, fourcc};

/// Uncompressed audio sample entry formats (QuickTime and ISO).
const PCM: [&[u8; 4]; 9] = [
    b"lpcm", b"sowt", b"twos", b"in24", b"in32", b"fl32", b"fl64", b"raw ", b"ipcm",
];
/// QuickTime `tmcd` flags.
const TC_DROP_FRAME: u32 = 0x1;
const TC_COUNTER: u32 = 0x8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Toggle,
    To(u32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Patch {
    pub off: u64,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub from: u32,
    pub to: u32,
    pub frames: u64,
    /// Sorted by offset, non-overlapping. Empty when already at the target.
    pub patches: Vec<Patch>,
    pub warnings: Vec<String>,
    /// Strategy B was used for the movie timescale.
    pub movie_rescaled: bool,
    pub tc_before: Option<String>,
    pub tc_after: Option<String>,
}

fn refuse<T>(msg: impl Into<String>) -> Result<T, Refusal> {
    Err(Refusal(msg.into()))
}

/// `v * num / den` when exact and within `max`.
pub fn scale_exact(v: u64, num: u64, den: u64, max: u64) -> Option<u64> {
    let p = u128::from(v) * u128::from(num);
    (p % u128::from(den) == 0)
        .then(|| p / u128::from(den))
        .and_then(|q| u64::try_from(q).ok())
        .filter(|&q| q <= max)
}

/// `v * num / den`, rounded half up, or None if it exceeds `max`.
pub fn scale_round(v: u64, num: u64, den: u64, max: u64) -> Option<u64> {
    let (num, den) = (u128::from(num), u128::from(den));
    let q = (u128::from(v) * num * 2 + den) / (den * 2);
    u64::try_from(q).ok().filter(|&q| q <= max)
}

fn max_of(f: &Field) -> u64 {
    if f.width == 4 {
        u64::from(u32::MAX)
    } else {
        u64::MAX
    }
}

fn put(patches: &mut Vec<Patch>, f: &Field, value: u64) {
    let bytes = if f.width == 4 {
        (value as u32).to_be_bytes().to_vec()
    } else {
        value.to_be_bytes().to_vec()
    };
    patches.push(Patch { off: f.off, bytes });
}

/// Exact video rate as (timescale, sample delta), if every sample has the same duration.
pub fn video_rate(t: &Track) -> Option<(u64, u64)> {
    let d = t.stts.iter().find(|e| e.0 > 0)?.1;
    (d > 0 && t.stts.iter().all(|e| e.0 == 0 || e.1 == d))
        .then_some((t.timescale.value, u64::from(d)))
}

pub fn frames(t: &Track) -> u64 {
    t.stts.iter().map(|e| u64::from(e.0)).sum()
}

/// HH:MM:SS:FF for frame number `n` at integer rate `fps`.
pub fn tc_label(n: u64, fps: u64) -> String {
    let (f, s) = (n % fps, n / fps);
    format!("{:02}:{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60, f)
}

pub fn plan(m: &Movie, target: Target) -> Result<Plan, Refusal> {
    let videos: Vec<&Track> = m.tracks.iter().filter(|t| &t.handler == b"vide").collect();
    if videos.is_empty() {
        return refuse("no video track");
    }
    let mut from = None;
    let mut total = 0;
    for v in &videos {
        let n = frames(v);
        if n != u64::from(v.sample_count) {
            return refuse("malformed file: stts and stsz disagree on the video sample count");
        }
        let Some((ts, d)) = video_rate(v) else {
            return refuse("non-constant sample durations (variable frame rate)");
        };
        let fps = match (ts % d == 0).then(|| ts / d) {
            Some(f @ (24 | 25)) => f as u32,
            _ => {
                return refuse(format!(
                    "video rate is {:.3} fps, not exactly 24 or 25",
                    ts as f64 / d as f64
                ));
            }
        };
        if from.is_some_and(|f| f != fps) {
            return refuse("several video tracks with different rates");
        }
        from = Some(fps);
        total = total.max(n);
    }
    let from = from.expect("at least one video track");
    let to = match target {
        Target::Toggle => 49 - from,
        Target::To(t @ (24 | 25)) => t,
        Target::To(t) => return refuse(format!("target must be 24 or 25, not {t}")),
    };
    let (num, den) = (u64::from(to), u64::from(from));
    let mut p = Plan {
        from,
        to,
        frames: total,
        patches: Vec::new(),
        warnings: Vec::new(),
        movie_rescaled: false,
        tc_before: None,
        tc_after: None,
    };
    let tc = m
        .tracks
        .iter()
        .find(|t| &t.handler == b"tmcd" && !t.tc_samples.is_empty());
    if let Some(t) = tc {
        p.tc_before = Some(tc_label(u64::from(t.tc_samples[0].1), den));
    }
    if to == from {
        p.tc_after = p.tc_before.clone();
        return Ok(p);
    }

    for t in &m.tracks {
        let ts = t.timescale.value;
        let Some(new_ts) = scale_exact(ts, num, den, u64::from(u32::MAX)) else {
            return refuse(format!(
                "track {} ('{}') timescale {ts} cannot be scaled by {to}/{from} exactly",
                t.id,
                fourcc(&t.handler)
            ));
        };
        put(&mut p.patches, &t.timescale, new_ts);
        match &t.handler {
            b"soun" => audio(t, ts, new_ts, &mut p.patches)?,
            b"tmcd" => timecode(t, den, num, &mut p)?,
            _ => {}
        }
    }

    // Movie timescale: Strategy A if exact, else Strategy B on movie-timescale durations.
    if let Some(new) = scale_exact(m.timescale.value, num, den, u64::from(u32::MAX)) {
        put(&mut p.patches, &m.timescale, new);
    } else {
        p.movie_rescaled = true;
        let fields = std::iter::once(&m.duration)
            .chain(m.tracks.iter().map(|t| &t.tkhd_duration))
            .chain(m.tracks.iter().flat_map(|t| &t.elst));
        for f in fields {
            let Some(v) = scale_round(f.value, den, num, max_of(f)) else {
                return refuse("field overflow: a movie duration does not fit its 32-bit field");
            };
            put(&mut p.patches, f, v);
        }
    }

    p.patches.sort_by_key(|x| x.off);
    if p.patches
        .windows(2)
        .any(|w| w[0].off + w[0].bytes.len() as u64 > w[1].off)
        || p.patches
            .iter()
            .any(|x| x.off + x.bytes.len() as u64 > m.file_len)
    {
        return refuse("malformed file: overlapping metadata fields");
    }
    Ok(p)
}

fn audio(t: &Track, old: u64, new: u64, patches: &mut Vec<Patch>) -> Result<(), Refusal> {
    for e in &t.entries {
        if !PCM.contains(&&e.format) {
            return refuse(format!(
                "unsupported audio codec '{}' (only uncompressed PCM can be relabelled safely)",
                fourcc(&e.format)
            ));
        }
        let b = &e.bytes;
        let bad = || Refusal("malformed file: short sound sample description".into());
        // Byte 16 is the QuickTime sound description version (0 in ISO files).
        match be16(b, 16).ok_or_else(bad)? {
            0 | 1 => {
                let rate = be32(b, 32).ok_or_else(bad)?;
                if u64::from(rate) != old << 16 {
                    return refuse(
                        "unsupported audio: sample rate does not match the media timescale",
                    );
                }
                if new > 0xFFFF {
                    return refuse(format!("field overflow: audio rate {new} does not fit"));
                }
                patches.push(Patch {
                    off: e.off + 32,
                    bytes: ((new as u32) << 16).to_be_bytes().to_vec(),
                });
            }
            2 => {
                let rate = f64::from_bits(be64(b, 40).ok_or_else(bad)?);
                if rate != old as f64 {
                    return refuse(
                        "unsupported audio: sample rate does not match the media timescale",
                    );
                }
                patches.push(Patch {
                    off: e.off + 40,
                    bytes: (new as f64).to_bits().to_be_bytes().to_vec(),
                });
            }
            v => return refuse(format!("unsupported audio: sound description version {v}")),
        }
    }
    Ok(())
}

fn timecode(t: &Track, from: u64, to: u64, p: &mut Plan) -> Result<(), Refusal> {
    for e in &t.entries {
        let b = &e.bytes;
        let (Some(flags), Some(ts), Some(fd), Some(nf)) =
            (be32(b, 20), be32(b, 24), be32(b, 28), b.get(32))
        else {
            return refuse("malformed file: short timecode sample description");
        };
        if &e.format != b"tmcd" {
            return refuse(format!(
                "unsupported timecode format '{}'",
                fourcc(&e.format)
            ));
        }
        if flags & TC_DROP_FRAME != 0 {
            return refuse("drop-frame timecode is not supported");
        }
        if flags & TC_COUNTER != 0 {
            return refuse("counter-mode timecode is not supported");
        }
        if fd == 0 || u64::from(ts) != from * u64::from(fd) || u64::from(*nf) != from {
            return refuse(format!(
                "unsupported timecode: {ts}/{fd} with {nf} frames does not match {from} fps"
            ));
        }
        let Some(new_ts) = scale_exact(u64::from(ts), to, from, u64::from(u32::MAX)) else {
            return refuse("timecode timescale cannot be scaled exactly");
        };
        p.patches.push(Patch {
            off: e.off + 24,
            bytes: (new_ts as u32).to_be_bytes().to_vec(),
        });
        p.patches.push(Patch {
            off: e.off + 32,
            bytes: vec![to as u8],
        });
    }
    for (i, &(off, n)) in t.tc_samples.iter().enumerate() {
        let n = u64::from(n);
        let (secs, ff) = (n / from, n % from);
        let new_ff = ff.min(to - 1);
        let new = secs * to + new_ff;
        let Ok(new32) = u32::try_from(new) else {
            return refuse("field overflow: timecode frame number");
        };
        if new_ff != ff {
            p.warnings.push(format!(
                "timecode {} has frame {ff}, which does not exist at {to} fps; clamped to {} (not reversible)",
                tc_label(n, from),
                tc_label(new, to)
            ));
        }
        if i == 0 {
            p.tc_after = Some(tc_label(new, to));
        }
        p.patches.push(Patch {
            off,
            bytes: new32.to_be_bytes().to_vec(),
        });
    }
    Ok(())
}
