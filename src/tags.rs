//! Finder fps tags (SPEC §8): the `com.apple.metadata:_kMDItemUserTags` extended
//! attribute, a binary property list holding an array of tag strings. Each string
//! is the tag name, optionally followed by "\n" and a Finder colour index.

use std::ffi::{CString, c_char, c_int, c_void};
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

const ATTR: &std::ffi::CStr = c"com.apple.metadata:_kMDItemUserTags";
const ENOATTR: i32 = 93; // macOS: the attribute does not exist

unsafe extern "C" {
    fn getxattr(
        p: *const c_char,
        n: *const c_char,
        v: *mut c_void,
        s: usize,
        pos: u32,
        o: c_int,
    ) -> isize;
    fn setxattr(
        p: *const c_char,
        n: *const c_char,
        v: *const c_void,
        s: usize,
        pos: u32,
        o: c_int,
    ) -> c_int;
}

/// "25 fps" for 25/1; fractional rates to 3 decimals with trailing zeros trimmed:
/// 24000/1001 -> "23.976 fps", 30000/1001 -> "29.97 fps".
pub fn tag_name(num: u64, den: u64) -> String {
    if den == 1 {
        return format!("{num} fps");
    }
    let s = format!("{:.3}", num as f64 / den as f64);
    format!("{} fps", s.trim_end_matches('0').trim_end_matches('.'))
}

/// Tag name without the colour suffix.
fn name(tag: &str) -> &str {
    tag.split('\n').next().unwrap_or("")
}

/// An fps tag is "<digits> fps" or "<digits>.<digits> fps" (any colour). Only these
/// are ever removed; every other tag is kept.
pub fn is_fps_tag(tag: &str) -> bool {
    let Some(n) = name(tag).strip_suffix(" fps") else {
        return false;
    };
    let (int, frac) = n.split_once('.').unwrap_or((n, "0"));
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    digits(int) && digits(frac)
}

/// Movies a folder is tagged for: non-hidden .mov/.mp4/.m4v files, not recursive.
pub fn videos_in(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| Some(e.ok()?.path()))
        .filter(|p| is_video_name(p) && p.is_file())
        .collect();
    v.sort();
    Ok(v)
}

fn is_video_name(p: &Path) -> bool {
    let hidden = p.file_name().is_none_or(|n| n.as_bytes().starts_with(b"."));
    let ext = p
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase());
    !hidden && matches!(ext.as_deref(), Some("mov" | "mp4" | "m4v"))
}

/// Replace any fps tag on `path` with `tag` (None removes it), keeping every other
/// tag. Returns false, writing nothing, when the file already has exactly that fps
/// tag (in any colour) and no other.
pub fn set_fps_tag(path: &Path, tag: Option<&str>) -> io::Result<bool> {
    let old = read(path)?;
    let fps: Vec<&str> = old
        .iter()
        .filter(|t| is_fps_tag(t))
        .map(|t| name(t))
        .collect();
    if fps == tag.into_iter().collect::<Vec<_>>() {
        return Ok(false);
    }
    let mut new: Vec<String> = old.into_iter().filter(|t| !is_fps_tag(t)).collect();
    new.extend(tag.map(|t| format!("{t}\n0"))); // "\n0" = no colour, as Finder writes it
    write(path, &new)?;
    Ok(true)
}

fn cpath(path: &Path) -> io::Result<CString> {
    CString::new(path.as_os_str().as_bytes()).map_err(io::Error::other)
}

fn read(path: &Path) -> io::Result<Vec<String>> {
    let p = cpath(path)?;
    let get = |buf: &mut [u8]| unsafe {
        getxattr(
            p.as_ptr(),
            ATTR.as_ptr(),
            // NULL asks for the size; an empty non-NULL buffer fails with ERANGE.
            if buf.is_empty() {
                std::ptr::null_mut()
            } else {
                buf.as_mut_ptr().cast()
            },
            buf.len(),
            0,
            0,
        )
    };
    let n = get(&mut []);
    if n < 0 {
        let e = io::Error::last_os_error();
        return if e.raw_os_error() == Some(ENOATTR) {
            Ok(Vec::new())
        } else {
            Err(e)
        };
    }
    let mut buf = vec![0; n as usize];
    let n = get(&mut buf);
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    buf.truncate(n as usize);
    // Never rewrite tags we cannot fully read: that would drop the user's tags.
    decode(&buf).ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "unreadable Finder tags"))
}

fn write(path: &Path, tags: &[String]) -> io::Result<()> {
    let p = cpath(path)?;
    let b = encode(tags);
    let r = unsafe { setxattr(p.as_ptr(), ATTR.as_ptr(), b.as_ptr().cast(), b.len(), 0, 0) };
    if r == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

fn be(b: &[u8]) -> u64 {
    b.iter().fold(0, |a, &x| a << 8 | u64::from(x))
}

/// Decode a bplist00 whose top object is an array of strings; None for anything else.
pub(crate) fn decode(b: &[u8]) -> Option<Vec<String>> {
    if b.len() < 40 || !b.starts_with(b"bplist00") {
        return None;
    }
    let t = &b[b.len() - 32..];
    let (osz, rsz) = (usize::from(t[6]), usize::from(t[7]));
    if !(1..=8).contains(&osz) || !(1..=8).contains(&rsz) {
        return None;
    }
    let (count, top, table) = (
        be(&t[8..16]),
        be(&t[16..24]),
        usize::try_from(be(&t[24..32])).ok()?,
    );
    let offset = |i: u64| -> Option<usize> {
        if i >= count {
            return None;
        }
        let p = table.checked_add(usize::try_from(i).ok()?.checked_mul(osz)?)?;
        usize::try_from(be(b.get(p..p.checked_add(osz)?)?)).ok()
    };
    // Object header at p: (marker, length, start of payload).
    let header = |p: usize| -> Option<(u8, usize, usize)> {
        let m = *b.get(p)?;
        if m & 0xF != 0xF {
            return Some((m >> 4, usize::from(m & 0xF), p + 1));
        }
        let k = *b.get(p + 1)?;
        if k >> 4 != 1 || k & 0xF > 3 {
            return None;
        }
        let w = 1usize << (k & 0xF);
        let n = usize::try_from(be(b.get(p + 2..p + 2 + w)?)).ok()?;
        Some((m >> 4, n, p + 2 + w))
    };
    let (m, n, start) = header(offset(top)?)?;
    if m != 0xA {
        return None;
    }
    (0..n)
        .map(|i| {
            let r = start.checked_add(i.checked_mul(rsz)?)?;
            let (m, len, s) = header(offset(be(b.get(r..r + rsz)?))?)?;
            match m {
                0x5 => Some(String::from_utf8_lossy(b.get(s..s.checked_add(len)?)?).into_owned()),
                0x6 => {
                    let raw = b.get(s..s.checked_add(len.checked_mul(2)?)?)?;
                    let u: Vec<u16> = raw
                        .chunks(2)
                        .map(|c| u16::from_be_bytes([c[0], c[1]]))
                        .collect();
                    String::from_utf16(&u).ok()
                }
                _ => None,
            }
        })
        .collect()
}

/// Encode an array of strings as bplist00 (8-byte offsets, 2-byte object refs).
pub(crate) fn encode(tags: &[String]) -> Vec<u8> {
    fn header(o: &mut Vec<u8>, marker: u8, n: usize) {
        if n < 15 {
            o.push(marker << 4 | n as u8);
        } else {
            o.push(marker << 4 | 0xF);
            o.push(0x13);
            o.extend((n as u64).to_be_bytes());
        }
    }
    let mut o = b"bplist00".to_vec();
    let mut offsets = vec![o.len() as u64];
    header(&mut o, 0xA, tags.len());
    for i in 1..=tags.len() {
        o.extend((i as u16).to_be_bytes());
    }
    for t in tags {
        offsets.push(o.len() as u64);
        if t.is_ascii() {
            header(&mut o, 0x5, t.len());
            o.extend(t.as_bytes());
        } else {
            let u: Vec<u16> = t.encode_utf16().collect();
            header(&mut o, 0x6, u.len());
            o.extend(u.iter().flat_map(|c| c.to_be_bytes()));
        }
    }
    let table = o.len() as u64;
    for off in &offsets {
        o.extend(off.to_be_bytes());
    }
    o.extend([0, 0, 0, 0, 0, 0, 8, 2]);
    o.extend((offsets.len() as u64).to_be_bytes());
    o.extend(0u64.to_be_bytes());
    o.extend(table.to_be_bytes());
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_pattern() {
        assert_eq!(tag_name(25, 1), "25 fps");
        assert_eq!(tag_name(24000, 1001), "23.976 fps");
        assert_eq!(tag_name(30000, 1001), "29.97 fps");
        assert_eq!(tag_name(60000, 1001), "59.94 fps");
        assert_eq!(tag_name(5, 2), "2.5 fps");
        for t in ["25 fps", "23.976 fps", "29.97 fps\n0", "50 fps\n4"] {
            assert!(is_fps_tag(t), "{t}");
        }
        for t in [
            "Red\n6", "fps", " fps", "25fps", "25 FPS", "25. fps", ".5 fps", "x25 fps", "25 fps!",
        ] {
            assert!(!is_fps_tag(t), "{t}");
        }
        assert!(is_video_name(Path::new("/a/B.MOV")) && is_video_name(Path::new("c.m4v")));
        assert!(!is_video_name(Path::new("._c.mov")) && !is_video_name(Path::new("c.txt")));
    }

    #[test]
    fn bplist_round_trip_and_apple_vector() {
        // Written by NSURL setResourceValue:forKey:NSURLTagNamesKey on macOS 26.
        let hex = "62706C6973743030A401020304555265640A365953656C656374730A30583235206670730A306465E5672C000A0030080D131D26000000000000010100000000000000050000000000000000000000000000002F";
        let apple: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        let tags = decode(&apple).unwrap();
        assert_eq!(tags, ["Red\n6", "Selects\n0", "25 fps\n0", "日本\n0"]);
        let many: Vec<String> = (0..40)
            .map(|i| format!("tag number {i} with a long name é"))
            .collect();
        for t in [tags, many, vec![]] {
            assert_eq!(decode(&encode(&t)).unwrap(), t);
        }
        // Truncated or garbage input never panics and is rejected.
        for n in 0..apple.len() {
            assert!(decode(&apple[..n]).is_none());
        }
        assert!(decode(b"bplist00 not really a property list at all, no").is_none());
    }
}
