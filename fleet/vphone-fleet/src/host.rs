//! The host-side handles on a running phone: its QMP socket, its cgroup, its threads' CPU
//! affinity, and its vphoned (over vsock, without the API proxy, so checks never count as use).

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::mem;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

use vphone_api_proxy::Upstream;

/// How long one QMP command or one vphoned request may take. vphoned answers `/v1/health` at
/// once; a phone that takes longer counts as not answering.
const CALL_TIMEOUT: Duration = Duration::from_secs(5);

/// Sends one QMP command (`stop`, `cont`, `query-status`) and returns its `return` line.
pub(crate) fn qmp(socket: &Path, command: &str) -> Result<String, String> {
    let fail = |e: &dyn std::fmt::Display| format!("QMP {} {command}: {e}", socket.display());
    let stream = UnixStream::connect(socket).map_err(|e| fail(&e))?;
    stream
        .set_read_timeout(Some(CALL_TIMEOUT))
        .map_err(|e| fail(&e))?;
    let mut writer = stream.try_clone().map_err(|e| fail(&e))?;
    let mut lines = BufReader::new(stream).lines();
    let mut answer = |what: &str| -> Result<String, String> {
        // Events ({"event": ...}) can come before the answer.
        for line in lines.by_ref() {
            let line = line.map_err(|e| fail(&e))?;
            if line.contains("\"return\"") || line.contains("\"QMP\"") {
                return Ok(line);
            }
            if line.contains("\"error\"") {
                return Err(fail(&line));
            }
        }
        Err(fail(&format!("closed before the answer to {what}")))
    };
    answer("the greeting")?;
    for cmd in ["qmp_capabilities", command] {
        writeln!(writer, "{{\"execute\":\"{cmd}\"}}").map_err(|e| fail(&e))?;
        let line = answer(cmd)?;
        if cmd == command {
            return Ok(line);
        }
    }
    unreachable!("the loop returns at the command")
}

/// The phone's own cgroup: the systemd scope `tools/phone-up.sh` named in `runs/<id>/scope`, if
/// QEMU (`pid`) is in it. Never a shared cgroup: a phone started without a scope has none.
pub(crate) fn scope_cgroup(cgroup_root: &Path, run: &Path, pid: i32) -> Option<PathBuf> {
    let scope = fs::read_to_string(run.join("scope")).ok()?;
    let line = fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    let path = line.lines().find_map(|l| l.strip_prefix("0::"))?;
    path.ends_with(&format!("/{}", scope.trim()))
        .then(|| cgroup_root.join(path.trim_start_matches('/')))
}

/// Writes `value` to the cgroup file `name`.
pub(crate) fn cgroup_write(cgroup: &Path, name: &str, value: &str) -> Result<(), String> {
    let file = cgroup.join(name);
    fs::write(&file, value).map_err(|e| format!("writing {value} to {}: {e}", file.display()))
}

/// Parses a CPU list (`0-3,8,10-11`).
pub(crate) fn parse_cpus(list: &str) -> Result<Vec<usize>, String> {
    let mut cpus = Vec::new();
    for part in list.trim().split(',').filter(|p| !p.is_empty()) {
        let num = |s: &str| {
            s.trim()
                .parse::<usize>()
                .map_err(|_| format!("CPU list '{list}': '{s}' is not a CPU number"))
        };
        match part.split_once('-') {
            Some((a, b)) => cpus.extend(num(a)?..=num(b)?),
            None => cpus.push(num(part)?),
        }
    }
    Ok(cpus)
}

/// Every process of the phone: its scope's `cgroup.procs`, else QEMU alone.
fn processes(cgroup: Option<&Path>, pid: i32) -> Vec<i32> {
    cgroup
        .and_then(|c| fs::read_to_string(c.join("cgroup.procs")).ok())
        .map_or_else(
            || vec![pid],
            |s| s.lines().filter_map(|l| l.trim().parse().ok()).collect(),
        )
}

/// Sets the CPU affinity of every thread of every process of the phone. Threads and processes
/// started later inherit it from the thread that starts them.
pub(crate) fn pin(cgroup: Option<&Path>, pid: i32, cpus: &[usize]) -> Result<(), String> {
    // SAFETY: cpu_set_t is a plain bit array; all zeroes is the empty set.
    let mut set: libc::cpu_set_t = unsafe { mem::zeroed() };
    for &cpu in cpus {
        // SAFETY: CPU_SET only sets a bit inside `set`; it ignores CPUs past its capacity.
        unsafe { libc::CPU_SET(cpu, &mut set) };
    }
    let mut failed = None;
    for p in processes(cgroup, pid) {
        let Ok(tasks) = fs::read_dir(format!("/proc/{p}/task")) else {
            continue;
        };
        for task in tasks.flatten() {
            let Ok(tid) = task.file_name().to_string_lossy().parse::<libc::pid_t>() else {
                continue;
            };
            // SAFETY: sched_setaffinity reads `set` for its size; a thread that has exited fails
            // with ESRCH and nothing else happens.
            let rc =
                unsafe { libc::sched_setaffinity(tid, mem::size_of_val(&set), &raw const set) };
            let err = std::io::Error::last_os_error();
            if rc != 0 && err.raw_os_error() != Some(libc::ESRCH) {
                failed = Some(format!("thread {tid}: {err}"));
            }
        }
    }
    failed.map_or(Ok(()), |e| Err(format!("setting CPU affinity: {e}")))
}

/// One HTTP request to the phone's vphoned; `(status code, body)`.
pub(crate) fn vphoned(
    upstream: &Upstream,
    method: &str,
    path: &str,
    body: &str,
) -> Result<(u16, String), String> {
    let mut s = upstream.connect().map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(CALL_TIMEOUT))
        .map_err(|e| e.to_string())?;
    s.set_write_timeout(Some(CALL_TIMEOUT))
        .map_err(|e| e.to_string())?;
    // The write side stays open until the answer is read: vphoned requires it (tools/vphoned.sh).
    write!(
        s,
        "{method} {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .map_err(|e| format!("vphoned {path}: {e}"))?;
    let mut got = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(end) = got.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&got[..end]).to_ascii_lowercase();
            let want = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|n| n.trim().parse::<usize>().ok());
            if want.is_some_and(|n| got.len() >= end + 4 + n) {
                break;
            }
        }
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => got.extend_from_slice(&chunk[..n]),
            Err(e) => return Err(format!("vphoned {path}: {e}")),
        }
    }
    let text = String::from_utf8_lossy(&got);
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let code = head
        .split(' ')
        .nth(1)
        .and_then(|c| c.parse().ok())
        .ok_or_else(|| format!("vphoned {path}: no HTTP status in {head:?}"))?;
    Ok((code, body.to_owned()))
}

/// Whether vphoned answers `/v1/health` with `"status":"ok"`.
pub(crate) fn healthy(upstream: &Upstream) -> bool {
    matches!(vphoned(upstream, "GET", "/v1/health", ""), Ok((200, body)) if body.contains("\"status\":\"ok\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_lists() {
        assert_eq!(parse_cpus("0-3,8,10-11").unwrap(), [0, 1, 2, 3, 8, 10, 11]);
        assert_eq!(parse_cpus("\n").unwrap(), Vec::<usize>::new());
        assert!(parse_cpus("0-x").is_err());
    }

    #[test]
    fn pin_sets_every_thread_of_the_process() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let pid = i32::try_from(child.id()).unwrap();
        pin(None, pid, &[0]).unwrap();
        let status = fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
        let line = status
            .lines()
            .find(|l| l.starts_with("Cpus_allowed_list"))
            .unwrap();
        child.kill().unwrap();
        child.wait().unwrap();
        assert!(line.ends_with("\t0"), "{line}");
    }
}
