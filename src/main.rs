//! CLI (SPEC §7).

use fps_swap::atoms::fourcc;
use fps_swap::conform::{frames, tc_label, video_rate};
use fps_swap::{Movie, Target, conform_file, inspect};
use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "\
usage: fps-swap [--to 24|25] [--dry-run] [--json] <file>...
       fps-swap info [--json] <file>...

Conforms MOV/MP4 files between exactly 24 and 25 fps by rewriting container
timing in place. Without --to, each file toggles (24 -> 25, 25 -> 24).";

fn main() -> ExitCode {
    let Ok(args) = std::env::args_os()
        .skip(1)
        .map(|a| a.into_string())
        .collect::<Result<Vec<_>, _>>()
    else {
        return usage("arguments must be valid UTF-8");
    };
    let mut args = args.into_iter().peekable();
    let info = args.next_if(|a| a == "info").is_some();
    let (mut target, mut dry_run, mut json, mut files) = (Target::Toggle, false, false, Vec::new());
    let mut flags_done = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            _ if flags_done => files.push(a),
            "--" => flags_done = true,
            "--json" => json = true,
            "--dry-run" if !info => dry_run = true,
            "--to" if !info => match args.next().as_deref() {
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
            _ if a.starts_with('-') && a.len() > 1 => return usage(&format!("unknown option {a}")),
            _ => files.push(a),
        }
    }
    if files.is_empty() {
        return usage("no files given");
    }

    let mut ok = true;
    let mut out = Vec::new();
    for file in &files {
        let (good, line) = if info {
            run_info(file, json)
        } else {
            run_conform(file, target, dry_run, json)
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
fn run_conform(file: &str, target: Target, dry_run: bool, json: bool) -> (bool, String) {
    let o = match conform_file(Path::new(file), target, dry_run) {
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

fn run_info(file: &str, json: bool) -> (bool, String) {
    let m = match inspect(Path::new(file)) {
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
