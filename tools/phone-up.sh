#!/bin/bash
# Boot one phone of a many-phones host (docs/DENSITY.md), in the foreground, with a profile.
#
#   tools/phone-up.sh ID|DIR [PROFILE] [extra QEMU args...]
#
# ID names $FLEET/phones/ID (FLEET: default vms). PROFILE is a [profile.NAME] of
# config/profiles.toml (default: phone.toml's `profile`, else lite). The phone's QMP socket and
# census go in runs/ID/; its vsock CID comes from phone.toml, its MAC and serial from its own files.
#
# API_LISTEN=HOST:PORT also starts the phone's token-checked HTTP/WebSocket API there
# (fleet/vphone-api-proxy: `cargo spark` in fleet/ on aarch64, `cargo build --release` elsewhere;
# VPHONE_API_PROXY names another binary).
# It prints its token unless VPHONE_API_TOKEN sets one, logs each connection with API_VERBOSE=1,
# and exits with QEMU.
#
# Exit 2: the phone's Disk.img, nvram.bin or SEPStorage is already open. The drives are opened with
# file.locking=off, so QEMU itself would let a second guest write the same disk. Exit 1: a golden,
# a folder without phone.toml or with read-only state files, or a serial, MAC or CID that another
# folder beside it also has.
set -euo pipefail
cd "$(dirname "$0")/.."
FLEET=${FLEET:-$PWD/vms}
arg=${1:?usage: phone-up.sh ID|DIR [PROFILE] [QEMU args...]}
case $arg in */*) VM=$arg ;; *) VM=$FLEET/phones/$arg ;; esac
shift
case ${1:-} in -*|'') profile= ;; *) profile=$1; shift ;; esac
VM=$(realpath -e "$VM")
id=${VM##*/}
fail() { echo "phone-up: $2" >&2; exit "$1"; }
[[ $id =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]] || fail 1 "phone id '$id': letters, digits, . _ - only"
case $VM in *,*) fail 1 "$VM: a comma in the path breaks QEMU's -drive options" ;; esac
case $VM/ in */golden/*) fail 1 "$VM is a golden: boot a phone cloned from it (docs/DENSITY.md)" ;; esac
[ -f "$VM/phone.toml" ] || fail 1 "$VM has no phone.toml: not a phone folder (docs/DENSITY.md)"
for f in Disk.img nvram.bin SEPStorage; do
  [ -w "$VM/$f" ] || fail 1 "$VM/$f is missing or read-only (a reflink of the golden keeps its 0444: chmod u+w)"
done
# Any process with one of the three files open (this user's processes; root sees all).
busy=$(find /proc/[0-9]*/fd -maxdepth 1 -type l -printf '%l\t%h\n' 2>/dev/null |
  grep -F -e "$VM/Disk.img	" -e "$VM/nvram.bin	" -e "$VM/SEPStorage	" | head -1) || true
[ -z "$busy" ] || fail 2 "refusing to boot: ${busy%%	*} is open in pid $(echo "${busy#*	}" | cut -d/ -f3)"
# And no two launchers between that check and QEMU's open: the locks pass to QEMU with these fds.
exec 7<"$VM/Disk.img" 8<"$VM/nvram.bin" 9<"$VM/SEPStorage"
{ flock -n 7 && flock -n 8 && flock -n 9; } || fail 2 "refusing to boot: another phone-up.sh holds $VM"

# phone.toml, the profile and the identity checks; prints shell assignments (profile CID mac serial).
mac='' serial=''
env=$(python3 -I - "$VM" "$PWD/config/profiles.toml" "$profile" <<'PY'
import os, re, shlex, sys, tomllib
vm, profiles, want = sys.argv[1:4]

def die(msg):
    sys.exit(f"phone-up: {msg}")

def phone(d):
    try:
        with open(os.path.join(d, "phone.toml"), "rb") as f:
            t = tomllib.load(f)
    except (OSError, tomllib.TOMLDecodeError) as e:
        die(f"{d}/phone.toml: {e}")
    def read(name):
        try:
            return open(os.path.join(d, name)).read().strip()
        except FileNotFoundError:
            return ""
    return t, {"cid": t.get("cid"), "mac": read("mac").lower(), "serial": read("serial")}

t, me = phone(vm)
if not isinstance(me["cid"], int) or me["cid"] < 3:
    die(f"{vm}/phone.toml: cid must be an integer >= 3 (vsock reserves 0-2)")
for k in ("mac", "serial"):
    if not me[k]:
        die(f"{vm}/{k} is missing: every phone has its own (config/profiles.toml [identity])")
parent = os.path.dirname(vm)
for other in sorted(os.listdir(parent)):
    d = os.path.join(parent, other)
    if d == vm or not os.path.isfile(os.path.join(d, "phone.toml")):
        continue
    for k, v in phone(d)[1].items():
        if v == me[k]:
            die(f"{vm} and {d} have the same {k} {v}: each phone needs its own")
# the restore reads the ECID from udid-prediction.txt, the launcher from ecid
try:
    udid = dict(l.split("=", 1) for l in open(os.path.join(vm, "udid-prediction.txt")).read().split() if "=" in l)
    if "ECID" in udid and int(udid["ECID"], 16) != int(open(os.path.join(vm, "ecid")).read().strip(), 16):
        die(f"{vm}: udid-prediction.txt's ECID {udid['ECID']} is not the ecid file's")
except FileNotFoundError:
    pass

with open(profiles, "rb") as f:
    table = tomllib.load(f).get("profile", {})
name = want or t.get("profile") or "lite"
if name not in table:
    die(f"no [profile.{name}] in {profiles} (have: {', '.join(table)})")
print(f"profile={shlex.quote(name)} CID={me['cid']} mac={shlex.quote(me['mac'])} serial={shlex.quote(me['serial'])}")
for k, v in table[name].items():
    if not re.fullmatch(r"[A-Z][A-Z0-9_]*", k) or not isinstance(v, (str, int)) or isinstance(v, bool):
        die(f"[profile.{name}] {k}: keys are run-vphone.sh variables, values strings or integers")
    # a variable already set in the caller's environment wins
    print(f'[ -n "${{{k}+x}}" ] || export {k}={shlex.quote(str(v))}')
PY
)
eval "$env"
export CID
R=$PWD/runs/$id
export REIMS_VGPU_CENSUS_DIR=${REIMS_VGPU_CENSUS_DIR-$R/census}
mkdir -p "$R"
echo "phone $id: profile $profile, cid $CID, mac $mac, serial $serial, census ${REIMS_VGPU_CENSUS_DIR:-off}," \
  "qmp $R/qmp.sock, state $VM/{Disk.img,nvram.bin,SEPStorage}" >&2
if [ -n "${API_LISTEN:-}" ]; then
  case $(uname -m) in
    aarch64) proxy=fleet/target/aarch64-unknown-linux-musl/release/vphone-api-proxy ;;
    *) proxy=fleet/target/release/vphone-api-proxy ;;
  esac
  proxy=${VPHONE_API_PROXY:-$proxy}
  "$proxy" --help >/dev/null 2>&1 ||
    fail 1 "API_LISTEN needs $proxy to run here: 'cargo spark' (aarch64) or 'cargo build --release' in fleet/, or set VPHONE_API_PROXY"
  # This shell's pid becomes QEMU's at the exec below, so --exit-with-parent ends it with the
  # phone. The lock fds stay out of it: a proxy outliving QEMU must not keep the files locked.
  "$proxy" --listen "$API_LISTEN" --cid "$CID" --vsock-dir "${VSOCK_DIR:-/tmp/vphone-vsock}" \
    --exit-with-parent $$ ${API_VERBOSE:+--verbose} 7<&- 8<&- 9<&- &
fi
# -name marks it a phone: tools/boot-test.sh leaves phones running.
exec ./run-vphone.sh "$VM" -name "phone-$id" -qmp "unix:$R/qmp.sock,server,nowait" "$@"
