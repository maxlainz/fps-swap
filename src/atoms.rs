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
}

#[derive(Clone, Debug)]
pub struct Movie {
    pub file_len: u64,
    pub timescale: Field,
    pub duration: Field,
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

/// Read a field of `width` bytes at `buf[off]`, which lives at file offset `base + off`.
fn field(buf: &[u8], base: u64, off: usize, width: u8, what: &str) -> Result<Field, Refusal> {
    let value = match width {
        4 => be32(buf, off).map(u64::from),
        _ => be64(buf, off),
    }
    .ok_or_else(|| bad(what))?;
    Ok(Field {
        off: base + off as u64,
        width,
        value,
    })
}

/// Full-box version byte; only 0 and 1 are defined for the boxes we patch.
fn version(buf: &[u8], b: &BoxRef, what: &str) -> Result<u8, Refusal> {
    match buf.get(b.body) {
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
    parse_moov(&buf, body, file_len, r)
}

fn parse_moov<R: Read + Seek>(
    buf: &[u8],
    base: u64,
    file_len: u64,
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
    let timescale = field(buf, base, mvhd.body + ts_off, 4, "mvhd")?;
    let duration = field(buf, base, mvhd.body + ts_off + 4, w, "mvhd")?;
    if timescale.value == 0 {
        return Err(bad("movie timescale is zero"));
    }
    let tracks = kids
        .iter()
        .filter(|b| &b.typ == b"trak")
        .map(|t| parse_trak(buf, base, t, file_len, r))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Movie {
        file_len,
        timescale,
        duration,
        tracks,
    })
}

fn parse_trak<R: Read + Seek>(
    buf: &[u8],
    base: u64,
    trak: &BoxRef,
    file_len: u64,
    r: &mut R,
) -> Result<Track, Refusal> {
    let kids = children(buf, trak.body, trak.end)?;
    let tkhd = need(&kids, b"tkhd", "trak")?;
    let v = version(buf, tkhd, "tkhd")?;
    let (id_off, w) = if v == 1 { (20, 8) } else { (12, 4) };
    let id = be32(buf, tkhd.body + id_off).ok_or_else(|| bad("tkhd"))?;
    // track_ID is followed by 4 reserved bytes, then duration.
    let tkhd_duration = field(buf, base, tkhd.body + id_off + 8, w, "tkhd")?;

    let mut elst = Vec::new();
    if let Some(edts) = find(&kids, b"edts") {
        let ek = children(buf, edts.body, edts.end)?;
        if let Some(e) = find(&ek, b"elst") {
            let v = version(buf, e, "elst")?;
            let n = be32(buf, e.body + 4).ok_or_else(|| bad("elst"))? as usize;
            let (step, w) = if v == 1 { (20, 8) } else { (12, 4) };
            if n.checked_mul(step)
                .is_none_or(|len| e.body + 8 + len > e.end)
            {
                return Err(bad("elst entry count"));
            }
            for i in 0..n {
                elst.push(field(buf, base, e.body + 8 + i * step, w, "elst")?);
            }
        }
    }

    let mdia = need(&kids, b"mdia", "trak")?;
    let mk = children(buf, mdia.body, mdia.end)?;
    let mdhd = need(&mk, b"mdhd", "mdia")?;
    let v = version(buf, mdhd, "mdhd")?;
    let (ts_off, w) = if v == 1 { (20, 8) } else { (12, 4) };
    let timescale = field(buf, base, mdhd.body + ts_off, 4, "mdhd")?;
    let media_duration = field(buf, base, mdhd.body + ts_off + 4, w, "mdhd")?.value;
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
    let n = be32(buf, stsd.body + 4).ok_or_else(|| bad("stsd"))?;
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
    let n = be32(buf, stts_b.body + 4).ok_or_else(|| bad("stts"))? as usize;
    if n.checked_mul(8)
        .is_none_or(|len| stts_b.body + 8 + len > stts_b.end)
    {
        return Err(bad("stts entry count"));
    }
    let stts = (0..n)
        .map(|i| {
            let o = stts_b.body + 8 + i * 8;
            (be32(buf, o).unwrap_or(0), be32(buf, o + 4).unwrap_or(0))
        })
        .collect();

    let stsz = need(&sk, b"stsz", "stbl")?;
    let fixed_size = be32(buf, stsz.body + 4).ok_or_else(|| bad("stsz"))?;
    let sample_count = be32(buf, stsz.body + 8).ok_or_else(|| bad("stsz"))?;
    if fixed_size == 0
        && (sample_count as usize)
            .checked_mul(4)
            .is_none_or(|len| stsz.body + 12 + len > stsz.end)
    {
        return Err(bad("stsz entry count"));
    }

    let mut tc_samples = Vec::new();
    if &handler == b"tmcd" {
        for (off, size) in sample_offsets(buf, &sk, stsz, fixed_size, sample_count)? {
            if size < 4 || off.checked_add(4).is_none_or(|e| e > file_len) {
                return Err(bad("timecode sample outside the file"));
            }
            let b = read_at(r, off, 4)?;
            tc_samples.push((off, be32(&b, 0).expect("4 bytes")));
        }
    }

    Ok(Track {
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
    })
}

/// File offsets of every sample (used for the few samples of a `tmcd` track).
fn sample_offsets(
    buf: &[u8],
    sk: &[BoxRef],
    stsz: &BoxRef,
    fixed_size: u32,
    count: u32,
) -> Result<Vec<(u64, u32)>, Refusal> {
    let count = count as usize;
    if count > MAX_TC_SAMPLES {
        return Err(Refusal("too many timecode samples".into()));
    }
    let chunks: Vec<u64> = if let Some(c) = find(sk, b"stco") {
        table(buf, c, 4)?
    } else if let Some(c) = find(sk, b"co64") {
        table(buf, c, 8)?
    } else {
        return Err(bad("no chunk offset table"));
    };
    let stsc = need(sk, b"stsc", "stbl")?;
    let rows: Vec<u64> = table(buf, stsc, 12)?;
    let n = rows.len();
    let row = |i: usize, k: usize| be32(buf, stsc.body + 8 + i * 12 + k * 4).unwrap_or(0) as usize;
    let mut out = Vec::with_capacity(count);
    for i in 0..n {
        let first = row(i, 0);
        let next = if i + 1 < n {
            row(i + 1, 0)
        } else {
            chunks.len() + 1
        };
        let per = row(i, 1);
        if first == 0 || next < first || next > chunks.len() + 1 {
            return Err(bad("stsc"));
        }
        for &chunk_off in &chunks[first - 1..next - 1] {
            let mut off = chunk_off;
            for _ in 0..per {
                if out.len() == count {
                    return Ok(out);
                }
                let size = if fixed_size != 0 {
                    fixed_size
                } else {
                    be32(buf, stsz.body + 12 + out.len() * 4).ok_or_else(|| bad("stsz"))?
                };
                out.push((off, size));
                off = off
                    .checked_add(u64::from(size))
                    .ok_or_else(|| bad("stsz"))?;
            }
        }
    }
    if out.len() != count {
        return Err(bad("sample table does not cover every sample"));
    }
    Ok(out)
}

/// A full box holding a 32-bit count and `count` rows of `row` bytes.
/// For 4/8-byte rows the values are returned; for other widths only the count matters.
fn table(buf: &[u8], b: &BoxRef, row: usize) -> Result<Vec<u64>, Refusal> {
    let n = be32(buf, b.body + 4).ok_or_else(|| bad("table"))? as usize;
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
