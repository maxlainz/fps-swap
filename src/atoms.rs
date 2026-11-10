//! Minimal, defensive MOV/MP4 atom reader (SPEC §4, §5).
//!
//! Only the boxes the conform needs are decoded. Every read is bounds-checked
//! and every size computation is checked: input files are untrusted.

use crate::Refusal;
use std::io::{Read, Seek, SeekFrom};

/// Largest `moov` we load into memory. Real files are a few MB at most.
pub const MAX_MOOV: u64 = 256 * 1024 * 1024;
/// Most timecode samples we are willing to visit in a `tmcd` track.
const MAX_TC_SAMPLES: usize = 100_000;

fn bad(what: &str) -> Refusal {
    Refusal(format!("malformed file: {what}"))
}

/// A fixed-width big-endian integer field somewhere in the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Field {
    /// Absolute file offset of the first byte.
    pub off: u64,
    /// 4 or 8 bytes.
    pub width: u8,
    pub value: u64,
}

/// One `stsd` sample entry, copied out of `moov`.
#[derive(Clone, Debug)]
pub struct Entry {
    pub format: [u8; 4],
    /// Absolute file offset of the entry's size field.
    pub off: u64,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct Track {
    pub id: u32,
    pub handler: [u8; 4],
    pub tkhd_duration: Field,
    /// `elst` segment durations (movie timescale).
    pub elst: Vec<Field>,
    /// `mdhd.timescale`.
    pub timescale: Field,
    pub media_duration: u64,
    pub stts: Vec<(u32, u32)>,
    pub sample_count: u32,
    pub entries: Vec<Entry>,
    /// Timecode tracks only: (absolute offset, frame number) of each sample.
    pub tc_samples: Vec<(u64, u32)>,
    /// Timecode tracks only: every chunk lies inside a top-level `mdat` payload
    /// and overlaps no other track's chunk, so its samples may be patched (G1).
    pub tc_safe: bool,
}

#[derive(Clone, Debug)]
pub struct Movie {
    pub file_len: u64,
    pub timescale: Field,
    pub duration: Field,
    /// QuickTime `mvhd` preview time, preview duration, poster time, selection
    /// time, selection duration and current time (movie timescale; zero in ISO files).
    pub mvhd_times: Vec<Field>,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Copy, Debug)]
pub struct BoxRef {
    pub typ: [u8; 4],
    pub start: usize,
    /// Offset of the payload (after the header).
    pub body: usize,
    pub end: usize,
}

/// Decode a box header. `h` holds the bytes available at `pos` (up to 16).
/// Returns (type, header length, end offset), with `end <= limit`.
pub fn parse_header(h: &[u8], pos: u64, limit: u64) -> Result<([u8; 4], u64, u64), Refusal> {
    let size32 = be32(h, 0).ok_or_else(|| bad("truncated box header"))?;
    let typ: [u8; 4] = h[4..8].try_into().expect("checked by be32 above");
    let (size, hdr) = match size32 {
        1 => (
            be64(h, 8).ok_or_else(|| bad("truncated 64-bit box header"))?,
            16,
        ),
        0 => (limit - pos, 8),
        n => (u64::from(n), 8),
    };
    if size < hdr {
        return Err(bad("box smaller than its header"));
    }
    let end = pos
        .checked_add(size)
        .ok_or_else(|| bad("box size overflow"))?;
    if end > limit {
        return Err(Refusal(format!(
            "truncated or malformed file: '{}' box runs past its container",
            fourcc(&typ)
        )));
    }
    Ok((typ, hdr, end))
}

/// Children of the container payload `buf[start..end]`.
pub fn children(buf: &[u8], start: usize, end: usize) -> Result<Vec<BoxRef>, Refusal> {
    let mut out = Vec::new();
    let mut pos = start;
    while pos < end {
        let avail = buf.get(pos..end.min(pos + 16)).ok_or_else(|| bad("box"))?;
        if avail.len() < 8 {
            // QuickTime allows a 32-bit zero terminator at the end of a container.
            if avail.iter().all(|&b| b == 0) {
                break;
            }
            return Err(bad("trailing bytes in container"));
        }
        let (typ, hdr, bend) = parse_header(avail, pos as u64, end as u64)?;
        out.push(BoxRef {
            typ,
            start: pos,
            body: pos + hdr as usize,
            end: bend as usize,
        });
        pos = bend as usize;
    }
    Ok(out)
}

fn find<'a>(kids: &'a [BoxRef], typ: &[u8; 4]) -> Option<&'a BoxRef> {
    kids.iter().find(|b| &b.typ == typ)
}

fn need<'a>(kids: &'a [BoxRef], typ: &[u8; 4], parent: &str) -> Result<&'a BoxRef, Refusal> {
    find(kids, typ).ok_or_else(|| bad(&format!("'{}' missing in '{parent}'", fourcc(typ))))
}

pub fn be16(b: &[u8], off: usize) -> Option<u16> {
    Some(u16::from_be_bytes(
        b.get(off..off.checked_add(2)?)?.try_into().ok()?,
    ))
}
pub fn be32(b: &[u8], off: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        b.get(off..off.checked_add(4)?)?.try_into().ok()?,
    ))
}
pub fn be64(b: &[u8], off: usize) -> Option<u64> {
    Some(u64::from_be_bytes(
        b.get(off..off.checked_add(8)?)?.try_into().ok()?,
    ))
}

pub fn fourcc(t: &[u8; 4]) -> String {
    t.iter()
        .map(|&c| {
            if c.is_ascii_graphic() || c == b' ' {
                c as char
            } else {
                '?'
            }
        })
        .collect()
}

/// The bytes of `buf` up to the end of box `b`: reads through this never leave the box.
fn upto<'a>(buf: &'a [u8], b: &BoxRef) -> &'a [u8] {
    &buf[..b.end]
}

/// Read a field of `width` bytes at payload offset `rel` of box `b`; `buf[i]` lives at
/// file offset `base + i`. A field that does not fit inside the box is a refusal.
fn field(
    buf: &[u8],
    base: u64,
    b: &BoxRef,
    rel: usize,
    width: u8,
    what: &str,
) -> Result<Field, Refusal> {
    let (buf, off) = (upto(buf, b), b.body + rel);
    let value = match width {
        4 => be32(buf, off).map(u64::from),
        _ => be64(buf, off),
    }
    .ok_or_else(|| bad(&format!("'{what}' box too short")))?;
    Ok(Field {
        off: base + off as u64,
        width,
        value,
    })
}

/// Full-box version byte; only 0 and 1 are defined for the boxes we patch.
fn version(buf: &[u8], b: &BoxRef, what: &str) -> Result<u8, Refusal> {
    match upto(buf, b).get(b.body) {
        Some(v @ (0 | 1)) => Ok(*v),
        Some(v) => Err(Refusal(format!("unsupported '{what}' version {v}"))),
        None => Err(bad(what)),
    }
}

fn read_at<R: Read + Seek>(r: &mut R, off: u64, n: usize) -> Result<Vec<u8>, Refusal> {
    let mut v = vec![0; n];
    r.seek(SeekFrom::Start(off))
        .and_then(|_| r.read_exact(&mut v))
        .map_err(|e| Refusal(format!("read error: {e}")))?;
    Ok(v)
}

/// Locate and parse the movie. Refuses anything that is not a plain,
/// non-fragmented, uncompressed QuickTime/ISO BMFF file.
pub fn parse<R: Read + Seek>(r: &mut R, file_len: u64) -> Result<Movie, Refusal> {
    let mut pos = 0u64;
    let mut moov: Option<(u64, u64)> = None;
    let mut mdats = Vec::new();
    let mut first = true;
    while pos < file_len {
        let n = (file_len - pos).min(16) as usize;
        if n < 8 {
            return Err(bad("trailing bytes after last box"));
        }
        let h = read_at(r, pos, n)?;
        let (typ, hdr, end) = parse_header(&h, pos, file_len).map_err(|e| {
            if first {
                Refusal("not a MOV/MP4 file".into())
            } else {
                e
            }
        })?;
        if first
            && !matches!(
                &typ,
                b"ftyp" | b"moov" | b"mdat" | b"free" | b"skip" | b"wide" | b"pnot" | b"uuid"
            )
        {
            return Err(Refusal("not a MOV/MP4 file".into()));
        }
        first = false;
        match &typ {
            b"moof" | b"mfra" => return Err(Refusal("fragmented MP4 is not supported".into())),
            b"moov" if moov.is_some() => return Err(bad("more than one 'moov'")),
            b"moov" => moov = Some((pos + hdr, end)),
            b"mdat" => mdats.push((pos + hdr, end)),
            _ => {}
        }
        pos = end;
    }
    let (body, end) = moov.ok_or_else(|| Refusal("no 'moov' box (not a playable movie)".into()))?;
    if end - body > MAX_MOOV {
        return Err(Refusal("'moov' box too large".into()));
    }
    // Load the moov payload; offsets inside `buf` map to file offset `body + i`.
    let buf = read_at(r, body, (end - body) as usize)?;
    parse_moov(&buf, body, file_len, &mdats, r)
}

fn parse_moov<R: Read + Seek>(
    buf: &[u8],
    base: u64,
    file_len: u64,
    mdats: &[(u64, u64)],
    r: &mut R,
) -> Result<Movie, Refusal> {
    let kids = children(buf, 0, buf.len())?;
    if find(&kids, b"cmov").is_some() {
        return Err(Refusal("compressed 'moov' (cmov) is not supported".into()));
    }
    if find(&kids, b"mvex").is_some() {
        return Err(Refusal("fragmented MP4 is not supported".into()));
    }
    let mvhd = need(&kids, b"mvhd", "moov")?;
    let v = version(buf, mvhd, "mvhd")?;
    let (ts_off, w) = if v == 1 { (20, 8) } else { (12, 4) };
    let timescale = field(buf, base, mvhd, ts_off, 4, "mvhd")?;
    let duration = field(buf, base, mvhd, ts_off + 4, w, "mvhd")?;
    if timescale.value == 0 {
        return Err(bad("movie timescale is zero"));
    }
    // After duration: rate (4), volume (2), reserved (10), matrix (36), then six times.
    let times_off = ts_off + 4 + usize::from(w) + 52;
    let mvhd_times = (0..6)
        .map(|i| field(buf, base, mvhd, times_off + 4 * i, 4, "mvhd"))
        .collect::<Result<Vec<_>, _>>()?;
    let (mut tracks, chunks): (Vec<Track>, Vec<Vec<(u64, u64)>>) = kids
        .iter()
        .filter(|b| &b.typ == b"trak")
        .map(|t| parse_trak(buf, base, t, file_len, r))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .unzip();
    for (i, t) in tracks.iter_mut().enumerate() {
        if &t.handler == b"tmcd" {
            t.tc_safe = tc_chunks_safe(i, &chunks, mdats);
        }
    }
    Ok(Movie {
        file_len,
        timescale,
        duration,
        mvhd_times,
        tracks,
    })
}

/// True when every non-empty chunk of track `i` sits inside one top-level `mdat`
/// payload and overlaps no non-empty chunk of any other track.
fn tc_chunks_safe(i: usize, chunks: &[Vec<(u64, u64)>], mdats: &[(u64, u64)]) -> bool {
    // Other tracks' extents sorted by start, with the running maximum of their ends.
    let mut others: Vec<(u64, u64)> = chunks
        .iter()
        .enumerate()
        .filter(|&(j, _)| j != i)
        .flat_map(|(_, c)| c.iter().filter(|c| c.1 > 0).map(|&(o, l)| (o, o + l)))
        .collect();
    others.sort_unstable();
    let mut max_end = 0;
    let max_ends: Vec<u64> = others
        .iter()
        .map(|&(_, e)| {
            max_end = max_end.max(e);
            max_end
        })
        .collect();
    chunks[i].iter().filter(|c| c.1 > 0).all(|&(off, len)| {
        // Extents were checked against the file length, so this cannot overflow.
        let end = off + len;
        let in_mdat = mdats.iter().any(|&(s, e)| s <= off && end <= e);
        // Every other extent starting before `end`; one of them overlaps iff its end > off.
        let n = others.partition_point(|&(s, _)| s < end);
        in_mdat && (n == 0 || max_ends[n - 1] <= off)
    })
}

fn parse_trak<R: Read + Seek>(
    buf: &[u8],
    base: u64,
    trak: &BoxRef,
    file_len: u64,
    r: &mut R,
) -> Result<(Track, Vec<(u64, u64)>), Refusal> {
    let kids = children(buf, trak.body, trak.end)?;
    let tkhd = need(&kids, b"tkhd", "trak")?;
    let v = version(buf, tkhd, "tkhd")?;
    let (id_off, w) = if v == 1 { (20, 8) } else { (12, 4) };
    let id = field(buf, base, tkhd, id_off, 4, "tkhd")?.value as u32;
    // track_ID is followed by 4 reserved bytes, then duration.
    let tkhd_duration = field(buf, base, tkhd, id_off + 8, w, "tkhd")?;

    let mut elst = Vec::new();
    if let Some(edts) = find(&kids, b"edts") {
        let ek = children(buf, edts.body, edts.end)?;
        if let Some(e) = find(&ek, b"elst") {
            let v = version(buf, e, "elst")?;
            let (step, w) = if v == 1 { (20, 8) } else { (12, 4) };
            let n = table(buf, e, step)?.len();
            for i in 0..n {
                elst.push(field(buf, base, e, 8 + i * step, w, "elst")?);
            }
        }
    }

    let mdia = need(&kids, b"mdia", "trak")?;
    let mk = children(buf, mdia.body, mdia.end)?;
    let mdhd = need(&mk, b"mdhd", "mdia")?;
    let v = version(buf, mdhd, "mdhd")?;
    let (ts_off, w) = if v == 1 { (20, 8) } else { (12, 4) };
    let timescale = field(buf, base, mdhd, ts_off, 4, "mdhd")?;
    let media_duration = field(buf, base, mdhd, ts_off + 4, w, "mdhd")?.value;
    if timescale.value == 0 {
        return Err(bad("media timescale is zero"));
    }
    let hdlr = need(&mk, b"hdlr", "mdia")?;
    let handler: [u8; 4] = buf
        .get(hdlr.body + 8..hdlr.body + 12)
        .filter(|_| hdlr.body + 12 <= hdlr.end)
        .and_then(|s| s.try_into().ok())
        .ok_or_else(|| bad("hdlr"))?;

    let minf = need(&mk, b"minf", "mdia")?;
    let nk = children(buf, minf.body, minf.end)?;
    let stbl = need(&nk, b"stbl", "minf")?;
    let sk = children(buf, stbl.body, stbl.end)?;

    // stsd: version/flags, count, then sample entries shaped like boxes.
    let stsd = need(&sk, b"stsd", "stbl")?;
    version(buf, stsd, "stsd")?;
    let n = field(buf, base, stsd, 4, 4, "stsd")?.value;
    let ents = children(buf, (stsd.body + 8).min(stsd.end), stsd.end)?;
    if ents.len() != n as usize || n == 0 {
        return Err(bad("stsd entry count"));
    }
    let entries = ents
        .iter()
        .map(|e| Entry {
            format: e.typ,
            off: base + e.start as u64,
            bytes: buf[e.start..e.end].to_vec(),
        })
        .collect();

    let stts_b = need(&sk, b"stts", "stbl")?;
    let n = table(buf, stts_b, 8)?.len();
    if let Some(ctts) = find(&sk, b"ctts") {
        table(buf, ctts, 8)?;
    }
    let stts = (0..n)
        .map(|i| {
            let o = stts_b.body + 8 + i * 8;
            (be32(buf, o).unwrap_or(0), be32(buf, o + 4).unwrap_or(0))
        })
        .collect();

    let stsz = need(&sk, b"stsz", "stbl")?;
    let fixed_size = field(buf, base, stsz, 4, 4, "stsz")?.value as u32;
    let sample_count = field(buf, base, stsz, 8, 4, "stsz")?.value as u32;
    if fixed_size == 0
        && (sample_count as usize)
            .checked_mul(4)
            .is_none_or(|len| stsz.body + 12 + len > stsz.end)
    {
        return Err(bad("'stsz' entry count"));
    }

    let is_tc = &handler == b"tmcd";
    if is_tc && sample_count as usize > MAX_TC_SAMPLES {
        return Err(Refusal("too many timecode samples".into()));
    }
    let mut samples = Vec::new();
    let chunks = chunk_extents(
        buf,
        &sk,
        stsz,
        fixed_size,
        sample_count,
        is_tc.then_some(&mut samples),
    )?;
    if chunks
        .iter()
        .any(|&(off, len)| off.checked_add(len).is_none_or(|e| e > file_len))
    {
        return Err(Refusal(format!(
            "truncated or malformed file: track {id} has data outside the file"
        )));
    }
    let mut tc_samples = Vec::new();
    for (off, size) in samples {
        if size < 4 {
            return Err(bad("timecode sample shorter than 4 bytes"));
        }
        let b = read_at(r, off, 4)?;
        tc_samples.push((off, be32(&b, 0).expect("4 bytes")));
    }

    let track = Track {
        id,
        handler,
        tkhd_duration,
        elst,
        timescale,
        media_duration,
        stts,
        sample_count,
        entries,
        tc_samples,
        tc_safe: false,
    };
    Ok((track, chunks))
}

/// Extent (file offset, byte length) of every chunk, from `stsc`, `stsz` and
/// `stco`/`co64`, with checked arithmetic. If `samples` is given, it also receives
/// the (offset, size) of every sample (used for the few samples of a `tmcd` track).
fn chunk_extents(
    buf: &[u8],
    sk: &[BoxRef],
    stsz: &BoxRef,
    fixed_size: u32,
    count: u32,
    mut samples: Option<&mut Vec<(u64, u32)>>,
) -> Result<Vec<(u64, u64)>, Refusal> {
    let offsets: Vec<u64> = match (find(sk, b"stco"), find(sk, b"co64")) {
        (Some(c), _) => table(buf, c, 4)?,
        (None, Some(c)) => table(buf, c, 8)?,
        (None, None) => Vec::new(),
    };
    let stsc = find(sk, b"stsc");
    let n = stsc
        .map(|b| table(buf, b, 12))
        .transpose()?
        .map_or(0, |t| t.len());
    let row = |i: usize, k: usize| {
        stsc.and_then(|b| be32(upto(buf, b), b.body + 8 + i * 12 + k * 4))
            .unwrap_or(0) as usize
    };
    let short = || bad("sample table does not cover every sample");
    let size_of = |i: usize| {
        if fixed_size != 0 {
            Some(fixed_size)
        } else {
            be32(upto(buf, stsz), stsz.body + 12 + i * 4)
        }
    };
    let count = count as usize;
    let mut done = 0usize;
    let mut out = Vec::with_capacity(offsets.len());
    for i in 0..n {
        let first = row(i, 0);
        let next = if i + 1 < n {
            row(i + 1, 0)
        } else {
            offsets.len() + 1
        };
        if first == 0 || next < first || next > offsets.len() + 1 {
            return Err(bad("stsc"));
        }
        for &chunk_off in &offsets[first - 1..next - 1] {
            // Samples in this chunk; chunks past the last sample are empty.
            let take = row(i, 1).min(count - done);
            let len = if fixed_size != 0 && samples.is_none() {
                u64::from(fixed_size) * take as u64
            } else {
                let mut len = 0u64;
                for k in done..done + take {
                    let size = size_of(k).ok_or_else(short)?;
                    if let Some(s) = samples.as_deref_mut() {
                        s.push((chunk_off.checked_add(len).ok_or_else(|| bad("stco"))?, size));
                    }
                    len += u64::from(size);
                }
                len
            };
            out.push((chunk_off, len));
            done += take;
        }
    }
    if done != count {
        return Err(short());
    }
    Ok(out)
}

/// A full box holding a 32-bit count and `count` rows of `row` bytes.
/// For 4/8-byte rows the values are returned; for other widths only the count matters.
fn table(buf: &[u8], b: &BoxRef, row: usize) -> Result<Vec<u64>, Refusal> {
    let buf = upto(buf, b);
    let n = be32(buf, b.body + 4)
        .ok_or_else(|| bad(&format!("'{}' box too short", fourcc(&b.typ))))? as usize;
    if n.checked_mul(row)
        .is_none_or(|len| b.body + 8 + len > b.end)
    {
        return Err(bad(&format!("'{}' entry count", fourcc(&b.typ))));
    }
    Ok((0..n)
        .map(|i| {
            let o = b.body + 8 + i * row;
            match row {
                4 => be32(buf, o).map_or(0, u64::from),
                8 => be64(buf, o).unwrap_or(0),
                _ => 0,
            }
        })
        .collect())
}
