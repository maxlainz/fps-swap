//! fps-swap: lossless, in-place 24 <-> 25 fps conform of MOV/MP4 files.
//! See SPEC.md for the contract this implements.

pub mod atoms;
pub mod conform;
#[cfg(test)]
mod tests;

use std::fmt;
use std::fs::{File, OpenOptions, TryLockError};
use std::io::ErrorKind;
use std::os::unix::fs::FileExt;
use std::path::Path;

pub use atoms::{Movie, parse};
pub use conform::{Plan, Target, plan};

/// A one-line reason why a file was left untouched (or, rarely, why a write failed).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal(pub String);

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Refusal {}

/// Result of [`conform_file`].
#[derive(Clone, Debug)]
pub struct Outcome {
    pub plan: Plan,
    /// Bytes actually written (0 for dry runs and no-ops).
    pub bytes_written: usize,
}

fn open(path: &Path, write: bool) -> Result<File, Refusal> {
    // Checked before opening: opening a FIFO would block.
    if std::fs::metadata(path).is_ok_and(|m| !m.is_file()) {
        return Err(Refusal("not a regular file".into()));
    }
    OpenOptions::new()
        .read(true)
        .write(write)
        .open(path)
        .map_err(|e| match e.kind() {
            ErrorKind::PermissionDenied | ErrorKind::ReadOnlyFilesystem if write => {
                Refusal("file not writable".into())
            }
            _ => Refusal(format!("cannot open: {e}")),
        })
}

/// Parse a file without modifying it.
pub fn inspect(path: &Path) -> Result<Movie, Refusal> {
    let mut f = open(path, false)?;
    let len = f.metadata().map_err(|e| Refusal(e.to_string()))?.len();
    parse(&mut f, len)
}

/// Conform one file. Every check runs and every patch is computed before the
/// first byte is written (G3); only fixed-width fields are overwritten (G2).
/// A dry run opens and locks the file exactly like a real run (writing nothing),
/// so it reaches the same verdict.
pub fn conform_file(path: &Path, target: Target, dry_run: bool) -> Result<Outcome, Refusal> {
    let mut f = open(path, true)?;
    let lock_warning = lock_outcome(f.try_lock())?;
    let len = f.metadata().map_err(|e| Refusal(e.to_string()))?.len();
    let movie = parse(&mut f, len)?;
    let mut plan = plan(&movie, target)?;
    plan.warnings.extend(lock_warning);
    if dry_run || plan.patches.is_empty() {
        return Ok(Outcome {
            plan,
            bytes_written: 0,
        });
    }
    let bytes_written = write_patches(&f, &plan)?;
    Ok(Outcome {
        plan,
        bytes_written,
    })
}

/// errno values meaning "this volume cannot lock", not "someone holds the lock".
#[cfg(target_os = "linux")]
const NO_LOCK_SUPPORT: [i32; 2] = [95, 37]; // EOPNOTSUPP (= ENOTSUP), ENOLCK
#[cfg(not(target_os = "linux"))]
const NO_LOCK_SUPPORT: [i32; 3] = [45, 102, 77]; // ENOTSUP, EOPNOTSUPP, ENOLCK

/// SPEC §6: a file locked by another process is refused; a volume that does not
/// support locking gets a warning and the conform proceeds without the lock.
pub(crate) fn lock_outcome(r: Result<(), TryLockError>) -> Result<Option<String>, Refusal> {
    match r {
        Ok(()) => Ok(None),
        Err(TryLockError::WouldBlock) => Err(Refusal("file is locked by another process".into())),
        Err(TryLockError::Error(e))
            if e.kind() == ErrorKind::Unsupported
                || e.raw_os_error()
                    .is_some_and(|c| NO_LOCK_SUPPORT.contains(&c)) =>
        {
            Ok(Some(
                "could not lock file (not supported on this volume); proceeding without lock"
                    .into(),
            ))
        }
        Err(TryLockError::Error(e)) => Err(Refusal(format!("cannot lock file: {e}"))),
    }
}

fn write_patches(f: &File, plan: &Plan) -> Result<usize, Refusal> {
    // Keep the original bytes so a failed write can be rolled back.
    let mut originals = Vec::with_capacity(plan.patches.len());
    for p in &plan.patches {
        let mut b = vec![0; p.bytes.len()];
        f.read_exact_at(&mut b, p.off)
            .map_err(|e| Refusal(format!("read error: {e}")))?;
        originals.push(b);
    }
    for (i, p) in plan.patches.iter().enumerate() {
        if let Err(e) = f.write_all_at(&p.bytes, p.off) {
            let restored = plan.patches[..=i]
                .iter()
                .zip(&originals)
                .all(|(p, o)| f.write_all_at(o, p.off).is_ok());
            let _ = f.sync_all();
            return Err(Refusal(format!(
                "write failed ({e}); {}",
                if restored {
                    "original bytes restored"
                } else {
                    "FILE MAY BE INCONSISTENT, restore from a copy"
                }
            )));
        }
    }
    f.sync_all()
        .map_err(|e| Refusal(format!("fsync failed after writing: {e}")))?;
    Ok(plan.patches.iter().map(|p| p.bytes.len()).sum())
}
