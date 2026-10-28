//! The connection to a phone's vphoned, over QEMU's hybrid-vsock socket or host `AF_VSOCK`.

use std::fs;
use std::io::{self, ErrorKind, Read, Write};
use std::mem;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use crate::{HEAD_TIMEOUT, Refusal, VPHONED_PORT};

/// `AF_VSOCK` buffer size. Linux caps a socket's send credit at it and iOS grants more only near
/// its 512 KiB window, so with the 256 KiB default a request body over 256 KiB stalls for good.
const VSOCK_BUFFER: u64 = 4 << 20;
/// `linux/vm_sockets.h` options (the libc crate does not export them), set at level `AF_VSOCK`.
const SO_VM_SOCKETS_BUFFER_SIZE: libc::c_int = 0;
const SO_VM_SOCKETS_BUFFER_MAX_SIZE: libc::c_int = 2;

/// Where one phone's vphoned is reached: the Unix socket `<vsock dir>/<cid>.sock` when it exists
/// (a `VSOCK=uds` phone), else host `AF_VSOCK`, chosen per connection as `tools/vphoned.sh` does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Upstream {
    cid: u32,
    uds: PathBuf,
}

impl Upstream {
    pub fn new(cid: u32, vsock_dir: &Path) -> Self {
        Self {
            cid,
            uds: vsock_dir.join(format!("{cid}.sock")),
        }
    }

    /// Opens one connection to vphoned. The socket is used only if this user owns it and its
    /// directory, and the directory is writable by nobody else.
    ///
    /// An `AF_VSOCK` stream comes back as a [`UnixStream`]: its read, write, shutdown and
    /// `try_clone` are plain socket calls, so it carries either kind.
    ///
    /// # Errors
    ///
    /// [`Refusal::Unreachable`] saying which datapath failed and how.
    pub fn connect(&self) -> Result<UnixStream, Refusal> {
        let Ok(socket) = fs::symlink_metadata(&self.uds) else {
            return self.connect_vsock();
        };
        // Anyone can create a file in a shared directory such as /tmp. A socket counts only when
        // it and its directory are this user's and nobody else can write the directory; else
        // another user could plant one and receive the phone's admitted requests.
        // SAFETY: geteuid(2) takes no arguments and cannot fail.
        let me = unsafe { libc::geteuid() };
        let dir = self.uds.parent().and_then(|d| fs::symlink_metadata(d).ok());
        let trusted = socket.file_type().is_socket()
            && socket.uid() == me
            && dir.is_some_and(|d| d.is_dir() && d.uid() == me && d.mode() & 0o022 == 0);
        if !trusted {
            return Err(Refusal::Unreachable(format!(
                "{} not used: it and its directory must belong to uid {me}, and the directory must not be \
                 group- or world-writable (another user may have put it there)",
                self.uds.display()
            )));
        }
        self.connect_uds()
    }

    fn connect_uds(&self) -> Result<UnixStream, Refusal> {
        let fail = |why: String| {
            Refusal::Unreachable(format!("{} (cid {}): {why}", self.uds.display(), self.cid))
        };
        let mut guest = UnixStream::connect(&self.uds)
            .map_err(|e| fail(format!("{e} (is the phone running?)")))?;
        // QEMU's hybrid vsock: "CONNECT <port>\n", answered by one "OK <host port>\n" line.
        guest
            .set_read_timeout(Some(HEAD_TIMEOUT))
            .map_err(|e| fail(e.to_string()))?;
        guest
            .write_all(format!("CONNECT {VPHONED_PORT}\n").as_bytes())
            .map_err(|e| fail(e.to_string()))?;
        let mut line = Vec::new();
        let mut byte = [0u8];
        while line.last() != Some(&b'\n') && line.len() < 64 {
            match guest.read(&mut byte) {
                Ok(0) => break,
                Ok(_) => line.push(byte[0]),
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => return Err(fail(format!("no answer to CONNECT {VPHONED_PORT}: {e}"))),
            }
        }
        if !line.starts_with(b"OK") {
            let answer = String::from_utf8_lossy(&line);
            return Err(fail(format!(
                "vsock refused: guest port {VPHONED_PORT} did not accept (vphoned not up yet?); QEMU answered {:?}",
                answer.trim_end()
            )));
        }
        guest
            .set_read_timeout(None)
            .map_err(|e| fail(e.to_string()))?;
        Ok(guest)
    }

    fn connect_vsock(&self) -> Result<UnixStream, Refusal> {
        let cid = self.cid;
        let fail = |what: &str, e: io::Error| {
            let hint = match e.raw_os_error() {
                Some(libc::ECONNREFUSED | libc::ECONNRESET) => {
                    "vsock refused: nothing listens on the guest port (vphoned not up yet?)"
                }
                Some(libc::EAFNOSUPPORT) => "no AF_VSOCK on this host (modprobe vhost_vsock)",
                _ => "is a phone with this cid booted with VSOCK=vhost? is vhost_vsock loaded?",
            };
            Refusal::Unreachable(format!(
                "vsock cid {cid} port {VPHONED_PORT}: {what}: {e} ({hint})"
            ))
        };
        // SAFETY: socket(2) takes no pointers; a negative return is checked below.
        let fd = unsafe { libc::socket(libc::AF_VSOCK, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0) };
        if fd < 0 {
            return Err(fail("socket", io::Error::last_os_error()));
        }
        // SAFETY: fd is a socket that was just opened and that nothing else owns.
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        let size_len =
            libc::socklen_t::try_from(mem::size_of::<u64>()).expect("8 fits in socklen_t");
        let buffer = VSOCK_BUFFER;
        for opt in [SO_VM_SOCKETS_BUFFER_MAX_SIZE, SO_VM_SOCKETS_BUFFER_SIZE] {
            // SAFETY: optval points at a live u64 and optlen is its size.
            let r = unsafe {
                libc::setsockopt(
                    fd.as_raw_fd(),
                    libc::AF_VSOCK,
                    opt,
                    (&raw const buffer).cast(),
                    size_len,
                )
            };
            if r < 0 {
                return Err(fail("setting its 4 MiB buffer", io::Error::last_os_error()));
            }
        }
        let addr = libc::sockaddr_vm {
            svm_family: libc::sa_family_t::try_from(libc::AF_VSOCK)
                .expect("AF_VSOCK fits in sa_family_t"),
            svm_reserved1: 0,
            svm_port: VPHONED_PORT,
            svm_cid: cid,
            svm_zero: [0; 4],
        };
        let addr_len = libc::socklen_t::try_from(mem::size_of_val(&addr))
            .expect("sockaddr_vm fits in socklen_t");
        // SAFETY: addr is a live sockaddr_vm and addr_len is its size.
        let r = unsafe { libc::connect(fd.as_raw_fd(), (&raw const addr).cast(), addr_len) };
        if r < 0 {
            return Err(fail("connect", io::Error::last_os_error()));
        }
        Ok(UnixStream::from(fd))
    }
}
