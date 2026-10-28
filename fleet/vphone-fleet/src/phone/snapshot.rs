//! Cold snapshots, restore, reset and delete of a stopped phone.

use std::fs;
use std::path::{Path, PathBuf};

use toml::Table;

use super::{
    PhoneToml, SNAPSHOT_EXTRAS, STATE, check_manifest, check_name, golden_digest, now_unix,
    phone_dir, read_trim, reflink, require_stopped, sha256sum, write_new,
};
use crate::config::Paths;

fn snapshot_dir(paths: &Paths, id: &str, name: &str) -> Result<PathBuf, String> {
    check_name("snapshot name", name)?;
    Ok(phone_dir(paths, id)?.join("snapshots").join(name))
}

/// Takes the cold snapshot `name` of the stopped phone `id`: reflinks of its state files, a
/// MANIFEST of their hashes and `snapshot.toml` (the phone's id and identity).
pub(crate) fn snapshot(paths: &Paths, id: &str, name: &str) -> Result<String, String> {
    let dir = phone_dir(paths, id)?;
    let target = snapshot_dir(paths, id, name)?;
    if fs::symlink_metadata(&target).is_ok() {
        return Err(format!("snapshot {name} of {id} already exists"));
    }
    let _stopped = require_stopped(&dir)?;
    let tmp = dir.join("snapshots").join(format!(".{name}.taking"));
    fs::create_dir_all(&tmp).map_err(|e| format!("creating {}: {e}", tmp.display()))?;
    let taken = (|| {
        for f in STATE {
            reflink(&dir.join(f), &tmp.join(f))?;
        }
        for f in SNAPSHOT_EXTRAS {
            if dir.join(f).is_file() {
                fs::copy(dir.join(f), tmp.join(f)).map_err(|e| format!("copying {f}: {e}"))?;
            }
        }
        write_new(&tmp.join("MANIFEST"), sha256sum(&tmp, &STATE)?.as_bytes())?;
        let meta = format!(
            "id = \"{id}\"\necid = \"{}\"\nserial = \"{}\"\nmac = \"{}\"\ntaken_unix = {}\n",
            read_trim(&dir.join("ecid")),
            read_trim(&dir.join("serial")),
            read_trim(&dir.join("mac")),
            now_unix()
        );
        write_new(&tmp.join("snapshot.toml"), meta.as_bytes())?;
        fs::rename(&tmp, &target).map_err(|e| format!("renaming {}: {e}", tmp.display()))
    })();
    if let Err(e) = taken {
        let _ = fs::remove_dir_all(&tmp);
        return Err(e);
    }
    Ok(format!("snapshot {name} of {id}: {}", target.display()))
}

/// Puts the stopped phone `id` back to its snapshot `name`. Refuses a snapshot whose recorded
/// phone or ECID is not this phone's, or whose files no longer match its MANIFEST. The three
/// files are replaced one after another: a crash between them leaves a mixed phone, and running
/// the same restore again repairs it.
pub(crate) fn restore(paths: &Paths, id: &str, name: &str) -> Result<String, String> {
    let dir = phone_dir(paths, id)?;
    let snap = snapshot_dir(paths, id, name)?;
    if !fs::symlink_metadata(&snap).is_ok_and(|m| m.is_dir()) {
        return Err(format!(
            "no snapshot {name} of {id} ({} is missing)",
            snap.display()
        ));
    }
    let meta_file = snap.join("snapshot.toml");
    let meta: Table = fs::read_to_string(&meta_file)
        .map_err(|e| format!("reading {}: {e}", meta_file.display()))?
        .parse()
        .map_err(|e| format!("{}: {e}", meta_file.display()))?;
    let field = |k: &str| meta.get(k).and_then(|v| v.as_str()).unwrap_or("");
    if field("id") != id {
        return Err(format!(
            "snapshot {name} in {id}'s folder was taken of phone '{}': refusing to restore it into {id}",
            field("id")
        ));
    }
    let ecid = read_trim(&dir.join("ecid"));
    if field("ecid") != ecid {
        return Err(format!(
            "snapshot {name} was taken with ECID {}, {id} now has {ecid}: its SEP storage would not match",
            field("ecid")
        ));
    }
    let _stopped = require_stopped(&dir)?;
    check_manifest(&snap)?;
    put_back(&dir, &snap, "restoring")?;
    // One a snapshot lacks was written after it (as vphone-cli's revert: `absentFiles`).
    for f in SNAPSHOT_EXTRAS {
        if snap.join(f).is_file() {
            fs::copy(snap.join(f), dir.join(f)).map_err(|e| format!("copying {f}: {e}"))?;
        } else if let Err(e) = fs::remove_file(dir.join(f))
            && e.kind() != std::io::ErrorKind::NotFound
        {
            return Err(format!("removing {f}: {e}"));
        }
    }
    Ok(format!("phone {id} restored to snapshot {name}"))
}

/// Reflinks `from`'s state files beside `dir`'s, then renames each over its original.
fn put_back(dir: &Path, from: &Path, why: &str) -> Result<(), String> {
    let tmp = |f: &str| dir.join(format!(".{f}.{why}"));
    for f in STATE {
        let _ = fs::remove_file(tmp(f));
        reflink(&from.join(f), &tmp(f))?;
    }
    for f in STATE {
        fs::rename(tmp(f), dir.join(f)).map_err(|e| format!("replacing {f}: {e}"))?;
    }
    Ok(())
}

fn confirmed(id: &str, confirm: Option<&str>, what: &str) -> Result<(), String> {
    if confirm == Some(id) {
        Ok(())
    } else {
        Err(format!(
            "{what} destroys {id}'s data: repeat the id to confirm (--confirm {id})"
        ))
    }
}

/// Gives the stopped phone `id` fresh state files from its golden, keeping its identity, CID,
/// API port and snapshots.
pub(crate) fn reset(paths: &Paths, id: &str, confirm: Option<&str>) -> Result<String, String> {
    confirmed(id, confirm, "reset")?;
    let dir = phone_dir(paths, id)?;
    let phone = PhoneToml::read(&dir)?;
    let golden = paths.fleet.join("golden").join(&phone.golden);
    if golden_digest(&golden)? != phone.golden_sha256 {
        return Err(format!(
            "golden {} is not the one {id} was made from (its MANIFEST changed)",
            phone.golden
        ));
    }
    let _stopped = require_stopped(&dir)?;
    put_back(&dir, &golden, "resetting")?;
    Ok(format!("phone {id} reset from golden {}", phone.golden))
}

/// Deletes the stopped phone `id`: its folder, snapshots included, and its run folder.
pub(crate) fn delete(paths: &Paths, id: &str, confirm: Option<&str>) -> Result<String, String> {
    confirmed(id, confirm, "delete")?;
    let dir = phone_dir(paths, id)?;
    // Held through the removal: no phone-up.sh can start it meanwhile.
    let _stopped = require_stopped(&dir)?;
    fs::remove_dir_all(&dir).map_err(|e| format!("deleting {}: {e}", dir.display()))?;
    let run = paths.run(id);
    if run.is_dir() {
        fs::remove_dir_all(&run).map_err(|e| format!("deleting {}: {e}", run.display()))?;
    }
    Ok(format!("phone {id} deleted"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_needs_the_id_repeated() {
        assert!(confirmed("p01", None, "delete").is_err());
        assert!(confirmed("p01", Some("p02"), "delete").is_err());
        assert!(confirmed("p01", Some("p01"), "delete").is_ok());
    }
}
