//! CLI (SPEC §7).

use fps_swap::atoms::fourcc;
use fps_swap::conform::{frames, tc_label, video_rate};
use fps_swap::{Movie, Target, conform_file, inspect, tags};
use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::SystemTime;

const USAGE: &str = "\
usage: fps-swap [--to 24|25] [--dry-run] [--json] <file>...
       fps-swap info [--json] <file>...
       fps-swap tag <file|folder>...

Conforms MOV/MP4 files between exactly 24 and 25 fps by rewriting container
timing in place. Without --to, each file toggles (24 -> 25, 25 -> 24).
tag sets a Finder tag such as '25 fps' on each file, or on every movie in a
folder, keeping all other tags.";

fn main() -> ExitCode {
    // File names need not be UTF-8: keep them as OsString, print them lossily.
    let mut args = std::env::args_os().skip(1).peekable();
    let info = args.next_if(|a| a == "info").is_some();
    let tag = !info && args.next_if(|a| a == "tag").is_some();
    let (mut target, mut dry_run, mut json) = (Target::Toggle, false, false);
    let mut files: Vec<OsString> = Vec::new();
    let mut flags_done = false;
    while let Some(a) = args.next() {
        let s = a.to_str().unwrap_or("");
        match s {
            _ if flags_done => files.push(a),
            "--" => flags_done = true,
            "--json" if !tag => json = true,
            "--dry-run" if !info && !tag => dry_run = true,
            "--to" if !info && !tag => match args.next().as_ref().and_then(|a| a.to_str()) {
                Some("24") => target = Target::To(24),
                Some("25") => target = Target::To(25),
                _ => return usage("--to takes 24 or 25"),
            },
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            "-V" | "--version" => {
                println!("fps-swap {}", env!("CARGO_PKG_VERSION"));
                return ExitCode::SUCCESS;
            }
            _ if s.starts_with('-') && s.len() > 1 => return usage(&format!("unknown option {s}")),
            _ => files.push(a),
        }
    }
    if files.is_empty() {
        return usage("no files given");
    }
    if tag {
        return run_tag(&files);
    }

    let mut ok = true;
    let mut out = Vec::new();
    // (device, inode) -> first name: the same file given twice must not toggle twice.
    let mut seen: HashMap<(u64, u64), String> = HashMap::new();
    for path in &files {
        let file = &path.to_string_lossy();
        let id = std::fs::metadata(path).ok().map(|m| (m.dev(), m.ino()));
        if let Some(first) = id.and_then(|id| seen.get(&id)) {
            let reason = format!("duplicate of {first}");
            if json {
                out.push(format!(
                    "{{\"file\":{},\"ok\":true,\"skipped\":true,\"reason\":{}}}",
                    q(file),
                    q(&reason)
                ));
            } else {
                println!("{file}: skipped: {reason}");
            }
            continue;
        }
        if let Some(id) = id {
            seen.insert(id, file.to_string());
        }
        let (good, line) = if info {
            run_info(Path::new(path), file, json)
        } else {
            run_conform(Path::new(path), file, target, dry_run, json)
        };
        ok &= good;
        if json {
            out.push(line);
        }
    }
    if json {
        println!("[{}]", out.join(",\n "));
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn usage(msg: &str) -> ExitCode {
    eprintln!("fps-swap: {msg}\n{USAGE}");
    ExitCode::from(2)
}

fn eight_n_plus_one(n: u64) -> bool {
    n % 8 == 1
}

fn note_8n1(file: &str, n: u64) {
    if !eight_n_plus_one(n) {
        println!("{file}: note: {n} frames is not of the form 8n+1");
    }
}

/// Returns (success, JSON object). Text output is printed directly.
fn run_conform(
    path: &Path,
    file: &str,
    target: Target,
    dry_run: bool,
    json: bool,
) -> (bool, String) {
    let o = match conform_file(path, target, dry_run) {
        Ok(o) => o,
        Err(e) => return refused(file, &e.0, json),
    };
    let p = &o.plan;
    let changed = !p.patches.is_empty();
    if json {
        return (
            true,
            format!(
                "{{\"file\":{},\"ok\":true,\"from\":{},\"to\":{},\"changed\":{changed},\"dry_run\":{dry_run},\
\"frames\":{},\"frames_8n1\":{},\"timecode\":{},\"timecode_after\":{},\"movie_rescaled\":{},\
\"patches\":{},\"bytes_written\":{},\"warnings\":[{}]}}",
                q(file),
                p.from,
                p.to,
                p.frames,
                eight_n_plus_one(p.frames),
                opt(&p.tc_before),
                opt(&p.tc_after),
                p.movie_rescaled,
                p.patches.len(),
                o.bytes_written,
                p.warnings
                    .iter()
                    .map(|w| q(w))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        );
    }
    let tc = p
        .tc_before
        .as_ref()
        .map(|t| format!(", TC {t}"))
        .unwrap_or_default();
    if changed {
        let dry = if dry_run {
            " (dry run, nothing written)"
        } else {
            ""
        };
        println!(
            "{file}: {} → {} fps, {} frames{tc}{dry}",
            p.from, p.to, p.frames
        );
    } else {
        println!(
            "{file}: already {} fps, {} frames{tc}, unchanged",
            p.to, p.frames
        );
    }
    for w in &p.warnings {
        println!("{file}: warning: {w}");
    }
    note_8n1(file, p.frames);
    (true, String::new())
}

fn refused(file: &str, reason: &str, json: bool) -> (bool, String) {
    if json {
        return (
            false,
            format!(
                "{{\"file\":{},\"ok\":false,\"error\":{}}}",
                q(file),
                q(reason)
            ),
        );
    }
    eprintln!("{file}: refused: {reason}");
    (false, String::new())
}

/// SPEC §8: tag each file (folders expand to their movies) with its rate. A
/// folder whose tags changed gets its modification date bumped once, so network
/// clients refresh; the movies' own dates are left alone.
fn run_tag(args: &[OsString]) -> ExitCode {
    let mut ok = true;
    let mut files = Vec::new();
    for a in args {
        let p = Path::new(a);
        if !p.is_dir() {
            files.push(p.to_path_buf());
            continue;
        }
        match tags::videos_in(p) {
            Ok(v) => files.extend(v),
            Err(e) => {
                eprintln!("{}: skipped: {e}", p.display());
                ok = false;
            }
        }
    }
    let (mut changed, mut same, mut skipped) = (0, 0, 0);
    let mut seen = HashSet::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for f in &files {
        if let Ok(m) = std::fs::metadata(f)
            && !seen.insert((m.dev(), m.ino()))
        {
            continue;
        }
        let file = f.display();
        // A file we cannot read keeps its tags: the failure may be transient.
        let m = match inspect(f) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("{file}: skipped: {e}");
                (ok, skipped) = (false, skipped + 1);
                continue;
            }
        };
        let fps = m
            .tracks
            .iter()
            .position(|t| &t.handler == b"vide")
            .and_then(|i| rate(&m, i));
        let name = fps.map(|(a, b)| tags::tag_name(a, b));
        let what = name.as_deref().unwrap_or("no constant frame rate");
        match tags::set_fps_tag(f, name.as_deref()) {
            Ok(true) => {
                match &name {
                    Some(n) => println!("{file}: tagged {n}"),
                    None => println!("{file}: {what}, fps tag removed"),
                }
                changed += usize::from(name.is_some());
                let dir = f.parent().map_or_else(|| ".".into(), |d| d.to_path_buf());
                if !dirs.contains(&dir) {
                    dirs.push(dir);
                }
            }
            Ok(false) => {
                println!("{file}: {what}, unchanged");
                same += usize::from(name.is_some());
            }
            Err(e) => {
                eprintln!("{file}: skipped: cannot set tag: {e}");
                (ok, skipped) = (false, skipped + 1);
                continue;
            }
        }
        // No constant rate: any stale fps tag was removed above; counted as skipped.
        skipped += usize::from(name.is_none());
    }
    for d in &dirs {
        let d = if d.as_os_str().is_empty() {
            Path::new(".")
        } else {
            d
        };
        if let Err(e) = std::fs::File::open(d).and_then(|h| h.set_modified(SystemTime::now())) {
            eprintln!(
                "{}: warning: could not update folder date: {e}",
                d.display()
            );
        }
    }
    println!("{changed} tagged, {same} already tagged, {skipped} skipped");
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn rate(m: &Movie, i: usize) -> Option<(u64, u64)> {
    let t = &m.tracks[i];
    (&t.handler == b"vide")
        .then(|| video_rate(t))
        .flatten()
        .map(|(ts, d)| {
            let g = gcd(ts, d);
            (ts / g, d / g)
        })
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a.max(1) } else { gcd(b, a % b) }
}

fn run_info(path: &Path, file: &str, json: bool) -> (bool, String) {
    let m = match inspect(path) {
        Ok(m) => m,
        Err(e) => return refused(file, &e.0, json),
    };
    let video = m.tracks.iter().position(|t| &t.handler == b"vide");
    let fps = video.and_then(|i| rate(&m, i));
    let n = video.map_or(0, |i| frames(&m.tracks[i]));
    let duration = m.duration.value as f64 / m.timescale.value as f64;
    let tc = m.tracks.iter().find_map(|t| {
        let &(_, n) = t.tc_samples.first()?;
        let nf = u64::from(*t.entries.first()?.bytes.get(32)?);
        (nf > 0).then(|| tc_label(u64::from(n), nf))
    });
    let rate_s = |r: Option<(u64, u64)>| r.map(|(a, b)| format!("{a}/{b}"));
    if json {
        let tracks: Vec<String> = m
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| {
                format!(
                    "{{\"id\":{},\"handler\":{},\"format\":{},\"timescale\":{},\"samples\":{},\"duration_s\":{:.6},\"rate\":{}}}",
                    t.id,
                    q(&fourcc(&t.handler)),
                    q(&t.entries.first().map(|e| fourcc(&e.format)).unwrap_or_default()),
                    t.timescale.value,
                    t.sample_count,
                    t.media_duration as f64 / t.timescale.value as f64,
                    opt(&rate_s(rate(&m, i)))
                )
            })
            .collect();
        return (
            true,
            format!(
                "{{\"file\":{},\"ok\":true,\"rate\":{},\"frames\":{n},\"frames_8n1\":{},\"duration_s\":{duration:.6},\
\"timecode\":{},\"tracks\":[{}]}}",
                q(file),
                opt(&rate_s(fps)),
                eight_n_plus_one(n),
                opt(&tc),
                tracks.join(",")
            ),
        );
    }
    let fps_s = match fps {
        Some((a, 1)) => format!("{a} fps"),
        Some((a, b)) => format!("{:.3} fps ({a}/{b})", a as f64 / b as f64),
        None if video.is_some() => "variable frame rate".into(),
        None => "no video".into(),
    };
    let tc_s = tc.map(|t| format!(", TC {t}")).unwrap_or_default();
    println!("{file}: {fps_s}, {n} frames, {duration:.3} s{tc_s}");
    for t in &m.tracks {
        println!(
            "  track {} {} '{}': timescale {}, {} samples, {:.3} s",
            t.id,
            fourcc(&t.handler),
            t.entries
                .first()
                .map(|e| fourcc(&e.format))
                .unwrap_or_default(),
            t.timescale.value,
            t.sample_count,
            t.media_duration as f64 / t.timescale.value as f64
        );
    }
    if video.is_some() {
        note_8n1(file, n);
    }
    (true, String::new())
}

/// JSON string literal.
fn q(s: &str) -> String {
    let mut o = String::from('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn opt(s: &Option<String>) -> String {
    s.as_deref().map_or_else(|| "null".into(), q)
}
