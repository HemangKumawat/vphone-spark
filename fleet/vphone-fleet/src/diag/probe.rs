//! What the kernel says about a phone and its files: `/proc`, `statvfs` and `FIEMAP`.

use std::ffi::CString;
use std::fs::{self, File};
use std::mem;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// `FS_IOC_FIEMAP`, `_IOWR('f', 11, struct fiemap)`: 0xC020660B, which musl's `ioctl` takes as
/// a C int.
#[cfg(target_env = "musl")]
const FS_IOC_FIEMAP: libc::Ioctl = -1_071_618_549;
#[cfg(not(target_env = "musl"))]
const FS_IOC_FIEMAP: libc::Ioctl = 0xC020_660B;
const FIEMAP_EXTENT_LAST: u32 = 0x1;
const FIEMAP_EXTENT_SHARED: u32 = 0x2000;
/// Extents asked for per `FIEMAP` call.
const EXTENTS: usize = 128;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Extent {
    logical: u64,
    physical: u64,
    length: u64,
    reserved64: [u64; 2],
    flags: u32,
    reserved: [u32; 3],
}

#[repr(C)]
struct Fiemap {
    start: u64,
    length: u64,
    flags: u32,
    mapped: u32,
    count: u32,
    reserved: u32,
    extents: [Extent; EXTENTS],
}

/// `(allocated, shared)` bytes of `file`: shared extents are blocks a reflink shares with the
/// golden or another phone.
pub(crate) fn extents(file: &Path) -> Result<(u64, u64), String> {
    let f = File::open(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let mut map = Box::new(Fiemap {
        start: 0,
        length: 0,
        flags: 0,
        mapped: 0,
        count: 0,
        reserved: 0,
        extents: [Extent::default(); EXTENTS],
    });
    let (mut total, mut shared, mut next) = (0u64, 0u64, 0u64);
    loop {
        map.start = next;
        map.length = u64::MAX - next;
        map.count = u32::try_from(EXTENTS).expect("EXTENTS fits in u32");
        map.mapped = 0;
        // SAFETY: `map` is a live, writable fiemap with room for `count` extents, as the ioctl
        // requires; the kernel writes at most `count` of them.
        let rc = unsafe { libc::ioctl(f.as_raw_fd(), FS_IOC_FIEMAP, &raw mut *map) };
        if rc != 0 {
            return Err(format!(
                "FIEMAP {}: {}",
                file.display(),
                std::io::Error::last_os_error()
            ));
        }
        let mapped = usize::try_from(map.mapped).unwrap_or(0).min(EXTENTS);
        if mapped == 0 {
            break;
        }
        for e in &map.extents[..mapped] {
            total += e.length;
            if e.flags & FIEMAP_EXTENT_SHARED != 0 {
                shared += e.length;
            }
            next = e.logical + e.length;
        }
        if map.extents[mapped - 1].flags & FIEMAP_EXTENT_LAST != 0 {
            break;
        }
    }
    Ok((total, shared))
}

/// `(size, available)` bytes of the filesystem holding `path`.
pub(crate) fn space(path: &Path) -> Result<(u64, u64), String> {
    let c = CString::new(path.as_os_str().as_bytes()).map_err(|e| e.to_string())?;
    // SAFETY: statvfs is plain old data; all zeroes is a valid value to be overwritten.
    let mut st: libc::statvfs = unsafe { mem::zeroed() };
    // SAFETY: `c` is a NUL-terminated path and `st` a live statvfs.
    if unsafe { libc::statvfs(c.as_ptr(), &raw mut st) } != 0 {
        return Err(format!(
            "statvfs {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        ));
    }
    Ok((st.f_blocks * st.f_frsize, st.f_bavail * st.f_frsize))
}

/// Clock ticks per second, the unit of `/proc/<pid>/stat` times.
pub(crate) fn ticks_per_sec() -> u64 {
    // SAFETY: sysconf takes an integer and reads nothing else.
    let t = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    u64::try_from(t).ok().filter(|&t| t > 0).unwrap_or(100)
}

/// `/proc/<pid>/stat` fields after the command name: index 0 is field 3 (state).
fn stat_fields(pid: i32) -> Option<Vec<u64>> {
    let s = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let (_, rest) = s.rsplit_once(") ")?;
    Some(
        rest.split_whitespace()
            .map(|f| f.parse().unwrap_or(0))
            .collect(),
    )
}

/// CPU time `pid` has used (user + system), in clock ticks ([`ticks_per_sec`]).
pub(crate) fn cpu_ticks(pid: i32) -> Option<u64> {
    let f = stat_fields(pid)?;
    // utime and stime are fields 14 and 15.
    Some(f.get(11)? + f.get(12)?)
}

/// Seconds since `pid` started.
pub(crate) fn uptime_secs(pid: i32) -> Option<u64> {
    let start = *stat_fields(pid)?.get(19)? / ticks_per_sec();
    let boot = fs::read_to_string("/proc/uptime").ok()?;
    let up = boot.split('.').next()?.parse::<u64>().ok()?;
    Some(up.saturating_sub(start))
}

/// A `Name: value` line's value from a `/proc/<pid>/` file.
pub(crate) fn proc_field(pid: i32, file: &str, name: &str) -> Option<String> {
    fs::read_to_string(format!("/proc/{pid}/{file}"))
        .ok()?
        .lines()
        .find_map(|l| {
            l.strip_prefix(name)?
                .strip_prefix(':')
                .map(|v| v.trim().to_owned())
        })
}

/// A `smaps_rollup` size (`Rss`, `Pss`) in bytes.
pub(crate) fn mem_bytes(pid: i32, name: &str) -> Option<u64> {
    let kb = proc_field(pid, "smaps_rollup", name)?;
    Some(kb.trim_end_matches("kB").trim().parse::<u64>().ok()? * 1024)
}

/// A run-vphone.sh size (`8G`, `4096M`, `512K`) in bytes.
pub(crate) fn size_bytes(text: &str) -> Option<u64> {
    let t = text.trim();
    let (num, unit) = t.split_at(t.find(|c: char| !c.is_ascii_digit()).unwrap_or(t.len()));
    let shift = match unit.to_ascii_uppercase().as_str() {
        "" | "M" => 20,
        "G" => 30,
        "K" => 10,
        "T" => 40,
        _ => return None,
    };
    num.parse::<u64>().ok()?.checked_shl(shift)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_and_this_process() {
        assert_eq!(size_bytes("8G"), Some(8 << 30));
        assert_eq!(size_bytes("4096M"), Some(4 << 30));
        assert_eq!(size_bytes("x"), None);
        let me = i32::try_from(std::process::id()).unwrap();
        assert!(cpu_ticks(me).is_some());
        assert!(mem_bytes(me, "Rss").unwrap() > 0);
        assert!(proc_field(me, "status", "Cpus_allowed_list").is_some());
        let (size, free) = space(Path::new("/")).unwrap();
        assert!(size >= free && size > 0);
    }

    #[test]
    fn extents_count_a_written_file() {
        let f = std::env::temp_dir().join(format!("vphone-fiemap-{}", std::process::id()));
        fs::write(&f, vec![7u8; 1 << 16]).unwrap();
        File::open(&f).unwrap().sync_all().unwrap();
        // tmpfs has no FIEMAP: only a real filesystem answers.
        if let Ok((total, shared)) = extents(&f) {
            assert!(total >= 1 << 16, "{total}");
            assert_eq!(shared, 0);
        }
        fs::remove_file(&f).unwrap();
    }
}
