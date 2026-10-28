//! A phone's files: goldens, create from a golden, cold snapshots, reset and delete
//! (docs/DENSITY.md). Nothing here touches a running phone: every operation that writes a
//! phone's state files first takes the locks `tools/phone-up.sh` holds while QEMU runs.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use toml::Table;

use crate::config::Paths;

mod create;
mod snapshot;

pub(crate) use create::{CreateSpec, create};
pub(crate) use snapshot::{delete, reset, restore, snapshot};

/// The three files that hold all of a phone's state (docs/DENSITY.md).
pub(crate) const STATE: [&str; 3] = ["Disk.img", "nvram.bin", "SEPStorage"];
/// Files a snapshot carries when present, as vphone-cli's do (`VPhoneMachineSnapshots`): they
/// describe what is on the disk. Identity (`config.plist`, `ecid`, `mac`, `serial`) is not
/// carried: it must survive a restore, and a restore refuses a snapshot of another identity.
const SNAPSHOT_EXTRAS: [&str; 2] = ["PatchReceipt.plist", "restore-info.json"];
/// A golden's files a clone does not copy: its state (reflinked instead) and the files that make
/// each phone its own.
const NOT_COPIED: [&str; 7] = [
    "Disk.img",
    "nvram.bin",
    "SEPStorage",
    "MANIFEST",
    "phone.toml",
    "serial",
    "mac",
];

/// A phone id, golden or snapshot name: one plain path component, as `tools/phone-up.sh` takes.
pub(crate) fn check_name(what: &str, name: &str) -> Result<(), String> {
    let ok = name.len() <= 64
        && name.starts_with(|c: char| c.is_ascii_alphanumeric())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "{what} '{name}': 1-64 letters, digits, . _ -, starting with a letter or digit"
        ))
    }
}

/// `phone.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PhoneToml {
    pub(crate) id: String,
    pub(crate) golden: String,
    pub(crate) golden_sha256: String,
    pub(crate) cid: u32,
    pub(crate) api_port: u16,
    pub(crate) profile: Option<String>,
}

impl PhoneToml {
    pub(crate) fn read(dir: &Path) -> Result<Self, String> {
        let file = dir.join("phone.toml");
        let text =
            fs::read_to_string(&file).map_err(|e| format!("reading {}: {e}", file.display()))?;
        let t: Table = text
            .parse()
            .map_err(|e| format!("{}: {e}", file.display()))?;
        let s = |k: &str| t.get(k).and_then(|v| v.as_str()).map(str::to_owned);
        let n = |k: &str| t.get(k).and_then(toml::Value::as_integer);
        let bad = |k: &str| format!("{}: {k} is missing or of the wrong type", file.display());
        Ok(Self {
            id: s("id").ok_or_else(|| bad("id"))?,
            golden: s("golden").ok_or_else(|| bad("golden"))?,
            golden_sha256: s("golden_sha256").ok_or_else(|| bad("golden_sha256"))?,
            cid: n("cid")
                .and_then(|c| u32::try_from(c).ok())
                .ok_or_else(|| bad("cid"))?,
            // Phones made by hand before vphone-fleet have no api_port: 0 = none yet.
            api_port: n("api_port")
                .map_or(Ok(0), |p| u16::try_from(p).map_err(|_| bad("api_port")))?,
            profile: s("profile"),
        })
    }

    /// Every value is a checked name, hex or a number, so plain quoting is valid TOML.
    fn write(&self, dir: &Path) -> Result<(), String> {
        let mut text = format!(
            "id = \"{}\"\ngolden = \"{}\"\ngolden_sha256 = \"{}\"\ncid = {}\napi_port = {}\n",
            self.id, self.golden, self.golden_sha256, self.cid, self.api_port
        );
        if let Some(p) = &self.profile {
            let _ = writeln!(text, "profile = \"{p}\"");
        }
        write_new(&dir.join("phone.toml"), text.as_bytes())
    }
}

/// Every phone folder under `phones/`, by id.
pub(crate) fn list(paths: &Paths) -> Vec<PhoneToml> {
    let Ok(dir) = fs::read_dir(paths.fleet.join("phones")) else {
        return Vec::new();
    };
    let mut phones: Vec<PhoneToml> = dir
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| PhoneToml::read(&e.path()).ok())
        .collect();
    phones.sort_by(|a, b| a.id.cmp(&b.id));
    phones
}

/// The checked folder of an existing phone.
pub(crate) fn phone_dir(paths: &Paths, id: &str) -> Result<PathBuf, String> {
    check_name("phone id", id)?;
    let dir = paths.phone(id);
    match fs::symlink_metadata(&dir) {
        Ok(m) if m.is_dir() => Ok(dir),
        Ok(_) => Err(format!("{} is not a folder", dir.display())),
        Err(_) => Err(format!("no phone {id} ({} is missing)", dir.display())),
    }
}

// --- files ---------------------------------------------------------------------------------

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), String> {
    fs::write(path, bytes).map_err(|e| format!("writing {}: {e}", path.display()))
}

/// Copies `src` to the new file `dst` by sharing its blocks (`FICLONE`); refused, as on a Mac
/// off APFS, when the filesystem cannot share them.
pub(crate) fn reflink(src: &Path, dst: &Path) -> Result<(), String> {
    let from = File::open(src).map_err(|e| format!("opening {}: {e}", src.display()))?;
    let to = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(dst)
        .map_err(|e| format!("creating {}: {e}", dst.display()))?;
    // SAFETY: FICLONE reads one fd argument; both fds are open for the whole call.
    if unsafe { libc::ioctl(to.as_raw_fd(), libc::FICLONE, from.as_raw_fd()) } == 0 {
        return Ok(());
    }
    let err = io::Error::last_os_error();
    drop(to);
    // Best effort: an empty file left behind would block a retry with create_new.
    let _ = fs::remove_file(dst);
    Err(format!(
        "cannot reflink {} ({err}): the golden and its phones must share one btrfs or reflink XFS \
         filesystem (tools/phones-volume.sh); a full copy is never made",
        src.display()
    ))
}

/// `sha256sum` of `files` inside `dir`, in its own output format (the MANIFEST format).
fn sha256sum(dir: &Path, files: &[&str]) -> Result<String, String> {
    let out = Command::new("sha256sum")
        .args(files)
        .current_dir(dir)
        .output()
        .map_err(|e| format!("running sha256sum: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "sha256sum in {}: {}",
            dir.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    String::from_utf8(out.stdout).map_err(|e| format!("sha256sum output: {e}"))
}

/// `sha256sum -c MANIFEST` inside `dir`.
fn check_manifest(dir: &Path) -> Result<(), String> {
    let out = Command::new("sha256sum")
        .args(["--quiet", "-c", "MANIFEST"])
        .current_dir(dir)
        .output()
        .map_err(|e| format!("running sha256sum: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{}: files differ from its MANIFEST: {}",
            dir.display(),
            String::from_utf8_lossy(&out.stdout).trim()
        ))
    }
}

fn golden_digest(golden: &Path) -> Result<String, String> {
    Ok(sha256sum(golden, &["MANIFEST"])?.chars().take(64).collect())
}

fn read_trim(path: &Path) -> String {
    fs::read_to_string(path)
        .map(|s| s.trim().to_owned())
        .unwrap_or_default()
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

// --- the stopped check ----------------------------------------------------------------------

/// The state files of a stopped folder, locked as `tools/phone-up.sh` locks them: no phone-up.sh
/// can start that folder while this is held.
#[derive(Debug)]
pub(crate) struct Stopped(#[expect(dead_code, reason = "held only for its locks")] Vec<File>);

/// Refuses when any process has one of `dir`'s state files open, or holds its locks.
pub(crate) fn require_stopped(dir: &Path) -> Result<Stopped, String> {
    let files: Vec<PathBuf> = STATE.iter().map(|f| dir.join(f)).collect();
    let real: BTreeSet<PathBuf> = files.iter().filter_map(|f| f.canonicalize().ok()).collect();
    if let Some((path, pid)) = open_by_any_process(&real) {
        return Err(format!(
            "{} is open in pid {pid}: stop that phone first",
            path.display()
        ));
    }
    let mut locks = Vec::new();
    for f in &files {
        let file = File::open(f).map_err(|e| format!("opening {}: {e}", f.display()))?;
        // SAFETY: flock(2) reads its two integer arguments; the fd is open.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(format!(
                "{} is locked: a tools/phone-up.sh holds it, stop that phone first",
                f.display()
            ));
        }
        locks.push(file);
    }
    Ok(Stopped(locks))
}

/// The first of `paths` some process has open, and its pid (this user's processes; root sees all).
fn open_by_any_process(paths: &BTreeSet<PathBuf>) -> Option<(PathBuf, String)> {
    for proc in fs::read_dir("/proc").ok()?.flatten() {
        let pid = proc.file_name().to_string_lossy().into_owned();
        if !pid.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(fds) = fs::read_dir(proc.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            if let Ok(target) = fs::read_link(fd.path())
                && paths.contains(&target)
            {
                return Some((target, pid));
            }
        }
    }
    None
}

// --- golden ----------------------------------------------------------------------------------

/// Makes the stopped VM folder `dir` a golden: moves it to `golden/<name>` (on the same
/// filesystem), records MANIFEST and makes the state files read-only. Run again on the golden to
/// finish one an earlier run left without MANIFEST.
pub(crate) fn golden_add(paths: &Paths, dir: &Path, name: Option<&str>) -> Result<String, String> {
    let dir = dir
        .canonicalize()
        .map_err(|e| format!("{}: {e}", dir.display()))?;
    let name = match name {
        Some(n) => n.to_owned(),
        None => dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .ok_or("the folder has no name")?,
    };
    check_name("golden name", &name)?;
    if dir.join("phone.toml").exists() {
        return Err(format!(
            "{} is a phone (it has phone.toml): a golden is a VM captured before any sign-in",
            dir.display()
        ));
    }
    for f in STATE.iter().chain(&["ecid"]) {
        if !dir.join(f).is_file() {
            return Err(format!("{} has no {f}: not a VM folder", dir.display()));
        }
    }
    let _stopped = require_stopped(&dir)?;
    let goldens = paths.fleet.join("golden");
    let target = goldens.join(&name);
    if dir != target.canonicalize().unwrap_or_default() {
        if target.exists() {
            return Err(format!("{} already exists", target.display()));
        }
        fs::create_dir_all(&goldens).map_err(|e| format!("{}: {e}", goldens.display()))?;
        fs::rename(&dir, &target).map_err(|e| {
            format!(
                "moving {} to {}: {e} (it must already be on the fleet's filesystem, {})",
                dir.display(),
                target.display(),
                paths.fleet.display()
            )
        })?;
    }
    let manifest = target.join("MANIFEST");
    if !manifest.exists() {
        let sums = sha256sum(&target, &STATE)?;
        write_new(&manifest, sums.as_bytes())?;
    }
    for f in STATE.iter().chain(&["MANIFEST"]) {
        let p = target.join(f);
        fs::set_permissions(&p, fs::Permissions::from_mode(0o444))
            .map_err(|e| format!("chmod 0444 {}: {e}", p.display()))?;
    }
    Ok(format!(
        "golden {name}: {} (sha256 of MANIFEST {})",
        target.display(),
        golden_digest(&target)?
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        for ok in ["p01", "A.b-c_d", "9"] {
            assert!(check_name("x", ok).is_ok(), "{ok}");
        }
        for bad in [
            "",
            ".hidden",
            "-x",
            "a/b",
            "..",
            "a b",
            "é",
            &"a".repeat(65),
        ] {
            assert!(check_name("x", bad).is_err(), "{bad:?}");
        }
    }
}
