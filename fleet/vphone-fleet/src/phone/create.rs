//! Create a phone from a golden: reflinked state, a new identity.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::Path;
use std::process::Command;

use super::{
    NOT_COPIED, PhoneToml, STATE, check_name, golden_digest, list, read_trim, reflink, write_new,
};
use crate::config::{Config, Paths};

/// `n` random bytes.
fn random_bytes<const N: usize>() -> Result<[u8; N], String> {
    let mut b = [0u8; N];
    File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .map_err(|e| format!("reading /dev/urandom: {e}"))?;
    Ok(b)
}

/// A unicast, locally administered MAC.
fn new_mac() -> Result<String, String> {
    let mut m: [u8; 6] = random_bytes()?;
    m[0] = (m[0] & 0xfc) | 0x02;
    Ok(m.map(|b| format!("{b:02x}")).join(":"))
}

/// 12 characters of A-Z 0-9, an Apple-length serial.
fn new_serial() -> Result<String, String> {
    const ALPHABET: &[u8; 36] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut s = String::new();
    while s.len() < 12 {
        for b in random_bytes::<16>()? {
            // 252 = 7 * 36: rejecting above it keeps every character equally likely.
            if b < 252 && s.len() < 12 {
                s.push(char::from(ALPHABET[usize::from(b % 36)]));
            }
        }
    }
    Ok(s)
}

/// A nonzero ECID well inside 64 bits, as `tools/new-vm.sh` makes one.
fn new_ecid() -> Result<u64, String> {
    Ok((u64::from_le_bytes(random_bytes()?) >> 1) | (1 << 40))
}

/// Writes the clone's MAC into `config.plist`, and with `ecid` its new machine identifier,
/// `ecid` and `udid-prediction.txt` (the same formats as `tools/new-vm.sh`).
fn write_identity(dir: &Path, mac: &str, ecid: Option<u64>) -> Result<(), String> {
    const SCRIPT: &str = r#"
import os, plistlib, sys
vm, mac, ecid = sys.argv[1], sys.argv[2], sys.argv[3]
p = os.path.join(vm, "config.plist")
if os.path.exists(p):
    raw = open(p, "rb").read()
    fmt = plistlib.FMT_BINARY if raw.startswith(b"bplist") else plistlib.FMT_XML
    c = plistlib.loads(raw)
    if isinstance(c.get("networkConfig"), dict):
        c["networkConfig"]["macAddress"] = mac
    if ecid:
        c["machineIdentifier"] = plistlib.dumps({"ECID": int(ecid, 16)}, fmt=plistlib.FMT_BINARY)
    with open(p + ".new", "wb") as f:
        plistlib.dump(c, f, fmt=fmt)
    os.replace(p + ".new", p)
if ecid:
    e = int(ecid, 16)
    open(os.path.join(vm, "ecid"), "w").write(f"0x{e:016x}\n")
    open(os.path.join(vm, "udid-prediction.txt"), "w").write(f"ECID={e:016X}\n")
"#;
    let out = Command::new("python3")
        .arg("-I")
        .arg("-c")
        .arg(SCRIPT)
        .arg(dir)
        .arg(mac)
        .arg(ecid.map(|e| format!("{e:x}")).unwrap_or_default())
        .output()
        .map_err(|e| format!("running python3: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "writing the identity into {}: {}",
            dir.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ))
    }
}

/// What a new phone is made from.
#[derive(Debug, Clone)]
pub(crate) struct CreateSpec<'spec> {
    pub(crate) golden: &'spec str,
    pub(crate) profile: Option<&'spec str>,
}

/// The identities every existing phone already uses.
#[derive(Debug, Default)]
struct Taken {
    cids: BTreeSet<u32>,
    ports: BTreeSet<u16>,
    macs: BTreeSet<String>,
    serials: BTreeSet<String>,
}

impl Taken {
    fn scan(paths: &Paths) -> Self {
        let mut t = Self::default();
        for p in list(paths) {
            let dir = paths.phone(&p.id);
            t.cids.insert(p.cid);
            t.ports.insert(p.api_port);
            t.macs.insert(read_trim(&dir.join("mac")).to_lowercase());
            t.serials.insert(read_trim(&dir.join("serial")));
        }
        t
    }
}

/// Makes the phone `id` from a golden: reflinks of its state files, copies of its other files,
/// and the phone's own serial, MAC, vsock CID and API port (and ECID with `rotate_ecid`). Built in
/// a hidden folder and renamed into place, so a failure leaves no half-made phone.
pub(crate) fn create(
    paths: &Paths,
    config: &Config,
    id: &str,
    spec: &CreateSpec<'_>,
) -> Result<String, String> {
    check_name("phone id", id)?;
    check_name("golden name", spec.golden)?;
    if let Some(p) = spec.profile
        && !config.profiles.iter().any(|q| q == p)
    {
        return Err(format!("no [profile.{p}] in config/profiles.toml"));
    }
    let golden = paths.fleet.join("golden").join(spec.golden);
    if !golden.join("MANIFEST").is_file() {
        return Err(format!(
            "{} has no MANIFEST: not a golden (vphone-fleet golden add)",
            golden.display()
        ));
    }
    let target = paths.phone(id);
    if fs::symlink_metadata(&target).is_ok() {
        return Err(format!("phone {id} already exists"));
    }
    let taken = Taken::scan(paths);
    let tmp = paths.fleet.join("phones").join(format!(".{id}.creating"));
    fs::create_dir_all(&tmp).map_err(|e| format!("creating {}: {e}", tmp.display()))?;
    let made = fill(config, id, spec, &golden, &tmp, &taken).and_then(|summary| {
        fs::rename(&tmp, &target)
            .map(|()| summary)
            .map_err(|e| format!("renaming {} to {}: {e}", tmp.display(), target.display()))
    });
    if made.is_err() {
        // Best effort: the error being returned is the one that matters.
        let _ = fs::remove_dir_all(&tmp);
    }
    made
}

fn fill(
    config: &Config,
    id: &str,
    spec: &CreateSpec<'_>,
    golden: &Path,
    tmp: &Path,
    taken: &Taken,
) -> Result<String, String> {
    for f in STATE {
        reflink(&golden.join(f), &tmp.join(f))?;
    }
    for entry in fs::read_dir(golden).map_err(|e| format!("{}: {e}", golden.display()))? {
        let entry = entry.map_err(|e| format!("{}: {e}", golden.display()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if NOT_COPIED.contains(&name.as_str()) || !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        fs::copy(entry.path(), tmp.join(&name)).map_err(|e| format!("copying {name}: {e}"))?;
    }
    let mac = loop {
        let m = new_mac()?;
        if !taken.macs.contains(&m) {
            break m;
        }
    };
    let serial = loop {
        let s = new_serial()?;
        if !taken.serials.contains(&s) {
            break s;
        }
    };
    let ecid = if config.rotate_ecid {
        Some(new_ecid()?)
    } else {
        None
    };
    write_new(&tmp.join("mac"), format!("{mac}\n").as_bytes())?;
    write_new(&tmp.join("serial"), format!("{serial}\n").as_bytes())?;
    write_identity(tmp, &mac, ecid)?;
    // CID 3 is run-vphone.sh's default for a lone VM: phones start at 10.
    let cid = (10..=u32::MAX)
        .find(|c| !taken.cids.contains(c))
        .ok_or("no free vsock CID")?;
    let api_port = (config.tunables.api_port_base..=u16::MAX)
        .find(|p| !taken.ports.contains(p))
        .ok_or("no free API port above api_port_base")?;
    PhoneToml {
        id: id.to_owned(),
        golden: spec.golden.to_owned(),
        golden_sha256: golden_digest(golden)?,
        cid,
        api_port,
        profile: spec.profile.map(str::to_owned),
    }
    .write(tmp)?;
    Ok(format!(
        "phone {id}: from {}, cid {cid}, api port {api_port}, mac {mac}, serial {serial}, ecid {}",
        spec.golden,
        ecid.map_or_else(
            || "the golden's".to_owned(),
            |e| format!("0x{e:016x} (new)")
        )
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identities_have_their_formats() {
        let mac = new_mac().unwrap();
        let first = u8::from_str_radix(&mac[..2], 16).unwrap();
        assert_eq!(first & 0x03, 0x02, "unicast, locally administered: {mac}");
        assert_eq!(mac.len(), 17);
        let serial = new_serial().unwrap();
        assert_eq!(serial.len(), 12);
        assert!(
            serial
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        );
        assert_ne!(new_serial().unwrap(), serial);
        let ecid = new_ecid().unwrap();
        assert!(ecid & (1 << 40) != 0 && ecid >> 63 == 0);
    }
}
