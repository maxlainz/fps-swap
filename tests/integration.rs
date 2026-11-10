//! Integration tests on fixtures generated at test time with ffmpeg (SPEC §9).
//! Skipped, with a message, when ffmpeg/ffprobe are not installed.
//!
//! "SHA-256 identical" is checked as byte-for-byte equality of the whole file,
//! which is the same guarantee without needing a hash implementation.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_fps-swap");
const FRAMES: u32 = 17;
const TC: [&str; 2] = ["-timecode", "01:00:00:00"];

fn tool(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .chain(["/opt/homebrew/bin".into(), "/usr/local/bin".into()])
        .map(|d| d.join(name))
        .find(|p| p.is_file())
}

/// (ffmpeg, ffprobe), or None after printing why the test is skipped.
fn tools(test: &str) -> Option<(PathBuf, PathBuf)> {
    match (tool("ffmpeg"), tool("ffprobe")) {
        (Some(a), Some(b)) => Some((a, b)),
        _ => {
            eprintln!(
                "SKIPPED {test}: ffmpeg/ffprobe not found (install ffmpeg to run integration tests)"
            );
            None
        }
    }
}

fn workdir(name: &str) -> PathBuf {
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("fixtures-{name}"));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).expect("mkdir");
    d
}

fn run(cmd: &Path, args: &[&str]) -> Output {
    Command::new(cmd).args(args).output().expect("spawn")
}

fn ok(cmd: &Path, args: &[&str]) -> String {
    let o = run(cmd, args);
    assert!(
        o.status.success(),
        "{} {args:?} failed: {}",
        cmd.display(),
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn fps_swap(args: &[&str]) -> Output {
    run(Path::new(BIN), args)
}

#[derive(Clone, Copy, Debug)]
enum Audio {
    None,
    Pcm,
    Aac,
}

struct Ff {
    ffmpeg: PathBuf,
    ffprobe: PathBuf,
}

impl Ff {
    /// Generate a small fixture. `rate` is an ffmpeg rate expression ("25", "24000/1001").
    fn make(
        &self,
        out: &Path,
        codec: &str,
        rate: &str,
        audio: Audio,
        faststart: bool,
        extra: &[&str],
    ) {
        let src = format!("testsrc2=size=64x64:rate={rate}");
        let mut a: Vec<&str> = vec!["-v", "error", "-y", "-f", "lavfi", "-i", &src];
        if !matches!(audio, Audio::None) {
            // Shorter than the video at every tested rate.
            a.extend(["-f", "lavfi", "-i", "sine=sample_rate=48000:duration=0.5"]);
        }
        a.extend(["-frames:v", "17"]);
        a.extend(match codec {
            "h264" => &["-c:v", "libx264", "-preset", "ultrafast"][..],
            "h265" => &[
                "-c:v",
                "libx265",
                "-preset",
                "ultrafast",
                "-tag:v",
                "hvc1",
                "-x265-params",
                "log-level=error",
            ],
            _ => &["-c:v", "prores"],
        });
        a.extend(match audio {
            Audio::None => &[][..],
            Audio::Pcm => &["-c:a", "pcm_s16le"],
            Audio::Aac => &["-c:a", "aac"],
        });
        if faststart {
            a.extend(["-movflags", "+faststart"]);
        }
        a.extend(extra);
        let o = out.to_str().expect("utf8 path");
        a.push(o);
        ok(&self.ffmpeg, &a);
    }

    fn video_hash(&self, f: &Path) -> String {
        let f = f.to_str().unwrap();
        ok(
            &self.ffmpeg,
            &[
                "-v", "error", "-i", f, "-map", "0:v:0", "-c", "copy", "-f", "hash", "-hash",
                "sha256", "-",
            ],
        )
    }

    /// (r_frame_rate, avg_frame_rate, packet count, timecode tag, duration)
    fn probe(&self, f: &Path) -> (String, String, String, String, f64) {
        let f = f.to_str().unwrap();
        let v = ok(
            &self.ffprobe,
            &[
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-count_packets",
                "-show_entries",
                "stream=r_frame_rate,avg_frame_rate,nb_read_packets:stream_tags=timecode",
                "-of",
                "default=nw=1",
                f,
            ],
        );
        let get = |k: &str| {
            v.lines()
                .find_map(|l| l.strip_prefix(&format!("{k}=")))
                .unwrap_or("")
                .to_string()
        };
        let d = ok(
            &self.ffprobe,
            &[
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "csv=p=0",
                f,
            ],
        );
        (
            get("r_frame_rate"),
            get("avg_frame_rate"),
            get("nb_read_packets"),
            get("TAG:timecode"),
            d.trim().parse().unwrap_or(-1.0),
        )
    }
}

/// Every SPEC §9 check for one fixture.
fn check_conform(ff: &Ff, f: &Path, fps: u32, tc: bool) {
    let name = f.display().to_string();
    let orig = fs::read(f).unwrap();
    let hash = ff.video_hash(f);
    let (r0, a0, n0, tc0, d0) = ff.probe(f);
    assert_eq!(
        (r0.as_str(), a0.as_str()),
        (&*format!("{fps}/1"), &*format!("{fps}/1")),
        "{name} source rate"
    );
    assert_eq!(n0, FRAMES.to_string());
    assert_eq!(tc0, if tc { "01:00:00:00" } else { "" }, "{name} source tc");

    let o = fps_swap(&[f.to_str().unwrap()]);
    assert!(
        o.status.success(),
        "{name}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    let to = 49 - fps;
    let line = String::from_utf8_lossy(&o.stdout);
    assert!(
        line.contains(&format!("{fps} → {to} fps, {FRAMES} frames")),
        "{name}: {line}"
    );

    let once = fs::read(f).unwrap();
    assert_eq!(once.len(), orig.len(), "{name}: G2 size");
    assert_ne!(once, orig, "{name}: something must change");
    assert!(
        once.iter().zip(&orig).filter(|(a, b)| a != b).count() < 64,
        "{name}: tens of bytes"
    );
    assert_eq!(ff.video_hash(f), hash, "{name}: G1 video packets");
    let (r1, a1, n1, tc1, d1) = ff.probe(f);
    assert_eq!(
        (r1.as_str(), a1.as_str()),
        (&*format!("{to}/1"), &*format!("{to}/1")),
        "{name}: G5 rate"
    );
    assert_eq!(n1, n0, "{name}: G4 frame count");
    assert_eq!(tc1, tc0, "{name}: timecode label");
    let expect = d0 * f64::from(fps) / f64::from(to);
    assert!(
        (d1 - expect).abs() < 0.002,
        "{name}: duration {d1} vs {expect}"
    );

    let o = fps_swap(&[f.to_str().unwrap()]);
    assert!(
        o.status.success(),
        "{name} (back): {}",
        String::from_utf8_lossy(&o.stderr)
    );
    let back = fs::read(f).unwrap();
    if fps == 25 {
        assert_eq!(back, orig, "{name}: G6 25 -> 24 -> 25 byte-identical");
    } else {
        // Movie timescale 1000 forces Strategy B on the way up: metadata-equivalent only.
        assert_eq!(back.len(), orig.len());
        assert_eq!(ff.video_hash(f), hash, "{name}: G6 frame hash");
        let (r2, a2, n2, tc2, d2) = ff.probe(f);
        assert_eq!((r2, a2, n2, tc2), (r0, a0, n0, tc0), "{name}: G6 metadata");
        assert!((d2 - d0).abs() < 0.002, "{name}: G6 duration {d2} vs {d0}");
    }
}

fn check_refused(f: &Path, args: &[&str], reason: &str) {
    let before = fs::read(f).unwrap();
    let mut a = args.to_vec();
    a.push(f.to_str().unwrap());
    let o = fps_swap(&a);
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success(), "{}: should be refused", f.display());
    assert!(
        err.contains(reason),
        "{}: expected '{reason}', got: {err}",
        f.display()
    );
    assert_eq!(err.lines().count(), 1, "one-line reason: {err}");
    assert_eq!(
        fs::read(f).unwrap(),
        before,
        "{}: G3 untouched",
        f.display()
    );
}

fn matrix(test: &str, codec: &str, ext: &str) {
    let Some((ffmpeg, ffprobe)) = tools(test) else {
        return;
    };
    let ff = Ff { ffmpeg, ffprobe };
    let dir = workdir(test);
    for fps in [24, 25] {
        for tc in [false, true] {
            for faststart in [false, true] {
                for audio in [Audio::None, Audio::Pcm, Audio::Aac] {
                    let f = dir.join(format!(
                        "{codec}_{fps}_{audio:?}_tc{tc}_fs{faststart}.{ext}"
                    ));
                    let extra: &[&str] = if tc { &TC } else { &[] };
                    ff.make(&f, codec, &fps.to_string(), audio, faststart, extra);
                    match audio {
                        Audio::Aac => check_refused(&f, &[], "unsupported audio codec 'mp4a'"),
                        _ => check_conform(&ff, &f, fps, tc),
                    }
                }
            }
        }
    }
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn h264_mov() {
    matrix("h264_mov", "h264", "mov");
}
#[test]
fn h264_mp4() {
    matrix("h264_mp4", "h264", "mp4");
}
#[test]
fn h265_mov() {
    matrix("h265_mov", "h265", "mov");
}
#[test]
fn h265_mp4() {
    matrix("h265_mp4", "h265", "mp4");
}
#[test]
fn prores_mov() {
    matrix("prores_mov", "prores", "mov");
}

#[test]
fn refusals_leave_files_untouched() {
    let Some((ffmpeg, ffprobe)) = tools("refusals") else {
        return;
    };
    let ff = Ff { ffmpeg, ffprobe };
    let dir = workdir("refusals");
    let mk = |name: &str, rate: &str, extra: &[&str]| {
        let f = dir.join(name);
        ff.make(&f, "h264", rate, Audio::Pcm, true, extra);
        f
    };
    check_refused(&mk("23976.mov", "24000/1001", &[]), &[], "23.976");
    check_refused(&mk("2997.mov", "30000/1001", &[]), &[], "29.970");
    check_refused(&mk("30.mov", "30", &[]), &[], "30.000");
    check_refused(
        &mk("50.mov", "50", &[]),
        &["--to", "24"],
        "not exactly 24 or 25",
    );
    check_refused(
        &mk(
            "vfr.mp4",
            "25",
            &["-vf", "select=not(eq(n\\,5))", "-fps_mode", "passthrough"],
        ),
        &[],
        "non-constant",
    );
    check_refused(
        &mk("frag.mp4", "25", &["-movflags", "frag_keyframe+empty_moov"]),
        &[],
        "fragmented",
    );

    // Truncated: moov at the end loses its tail; moov at the front loses the end of mdat.
    let end = dir.join("moov_end.mov");
    ff.make(&end, "h264", "25", Audio::Pcm, false, &TC);
    let b = fs::read(&end).unwrap();
    fs::write(&end, &b[..b.len() - 100]).unwrap();
    check_refused(&end, &[], "truncated");
    let front = mk("moov_front.mov", "25", &[]);
    let b = fs::read(&front).unwrap();
    fs::write(&front, &b[..b.len() - 100]).unwrap();
    check_refused(&front, &[], "truncated");

    let junk = dir.join("junk.mov");
    fs::write(&junk, b"definitely not a quicktime movie, just text").unwrap();
    check_refused(&junk, &[], "not a MOV/MP4");

    let ro = mk("readonly.mov", "25", &[]);
    let mut perm = fs::metadata(&ro).unwrap().permissions();
    perm.set_readonly(true);
    fs::set_permissions(&ro, perm.clone()).unwrap();
    check_refused(&ro, &[], "not writable");
    #[allow(clippy::permissions_set_readonly_false)]
    perm.set_readonly(false);
    fs::set_permissions(&ro, perm).unwrap();

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn cli_modes() {
    let Some((ffmpeg, ffprobe)) = tools("cli_modes") else {
        return;
    };
    let ff = Ff { ffmpeg, ffprobe };
    let dir = workdir("cli_modes");
    let a = dir.join("a.mov");
    ff.make(&a, "prores", "25", Audio::Pcm, true, &TC);
    let p = a.to_str().unwrap();
    let orig = fs::read(&a).unwrap();

    // --dry-run writes nothing.
    let o = fps_swap(&["--dry-run", p]);
    assert!(o.status.success());
    assert!(
        String::from_utf8_lossy(&o.stdout)
            .contains("25 → 24 fps, 17 frames, TC 01:00:00:00 (dry run")
    );
    assert_eq!(fs::read(&a).unwrap(), orig);

    // --to the current rate is a no-op.
    let o = fps_swap(&["--to", "25", p]);
    assert!(o.status.success());
    assert!(String::from_utf8_lossy(&o.stdout).contains("already 25 fps"));
    assert_eq!(fs::read(&a).unwrap(), orig);

    // info, text and JSON. 17 = 8*2+1, so no 8n+1 note.
    let o = fps_swap(&["info", p]);
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(
        s.contains("25 fps, 17 frames") && s.contains("TC 01:00:00:00") && !s.contains("8n+1"),
        "{s}"
    );
    let o = fps_swap(&["info", "--json", p]);
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(
        s.starts_with('[') && s.contains("\"rate\":\"25/1\"") && s.contains("\"frames\":17"),
        "{s}"
    );

    // Several files: a failure does not stop the others, and the exit code is non-zero.
    let junk = dir.join("junk.mov");
    fs::write(&junk, b"nope nope nope").unwrap();
    let o = fps_swap(&["--json", junk.to_str().unwrap(), p]);
    assert_eq!(o.status.code(), Some(1));
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(
        s.contains("\"ok\":false") && s.contains("\"from\":25,\"to\":24"),
        "{s}"
    );
    assert!(s.contains("\"frames_8n1\":true"), "{s}");
    assert_ne!(fs::read(&a).unwrap(), orig);

    // Explicit --to 25 brings it back.
    assert!(fps_swap(&["--to", "25", p]).status.success());
    assert_eq!(fs::read(&a).unwrap(), orig);

    // Usage errors exit 2.
    assert_eq!(fps_swap(&[]).status.code(), Some(2));
    assert_eq!(fps_swap(&["--to", "30", p]).status.code(), Some(2));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn timecode_frame_24_clamps_with_warning() {
    let Some((ffmpeg, ffprobe)) = tools("tc_clamp") else {
        return;
    };
    let ff = Ff { ffmpeg, ffprobe };
    let dir = workdir("tc_clamp");
    let f = dir.join("ff24.mov");
    ff.make(
        &f,
        "prores",
        "25",
        Audio::None,
        true,
        &["-timecode", "10:00:00:24"],
    );
    let o = fps_swap(&[f.to_str().unwrap()]);
    assert!(o.status.success());
    let s = String::from_utf8_lossy(&o.stdout);
    assert!(
        s.contains("warning:") && s.contains("clamped to 10:00:00:23"),
        "{s}"
    );
    assert_eq!(ff.probe(&f).3, "10:00:00:23");
    let _ = fs::remove_dir_all(&dir);
}
