#!/bin/bash
# Mount an XFS volume kept in one file, for a host whose filesystem cannot reflink (ext4). A golden
# and its phones must sit on one filesystem that can (btrfs, or XFS with reflink), or every phone
# costs a full copy of the golden (docs/DENSITY.md). Needs root (losetup, mkfs.xfs, mount).
#
#   sudo tools/phones-volume.sh IMAGE SIZE [MOUNTPOINT]
#
# IMAGE is made sparse on first use (e.g. /data/phones.xfs, SIZE 2T): only written blocks take
# space on the host disk, so watch its free space (df -h), not SIZE. MOUNTPOINT (default vms, the
# checkout's; elsewhere, point tools/phone-up.sh at it with FLEET=) must be empty. Nothing is added
# to fstab or systemd: run it again after a reboot, with the same arguments.
set -euo pipefail
[ "$(id -u)" = 0 ] || { echo "phones-volume.sh needs root (losetup, mkfs.xfs, mount): sudo $0 $*" >&2; exit 1; }
IMG=${1:?usage: phones-volume.sh IMAGE SIZE [MOUNTPOINT]}
SIZE=${2:?usage: phones-volume.sh IMAGE SIZE [MOUNTPOINT]}
MNT=${3:-$(cd "$(dirname "$0")/.." && pwd)/vms}
if mountpoint -q "$MNT"; then echo "$MNT is already mounted"; exit 0; fi
mkdir -p "$MNT"
[ -z "$(ls -A "$MNT")" ] || { echo "$MNT is not empty: mounting would hide what is there" >&2; exit 1; }
if [ ! -e "$IMG" ]; then
  truncate -s "$SIZE" "$IMG"
  mkfs.xfs -q -m reflink=1 "$IMG"
fi
# Direct I/O: without it every block a phone reads is cached twice, once per filesystem.
dev=$(losetup -f --show --direct-io=on "$IMG")
mount "$dev" "$MNT" || { losetup -d "$dev"; exit 1; }
# the user who ran sudo owns the volume's top folder
chown "${SUDO_UID:-0}:${SUDO_GID:-0}" "$MNT"
echo "$IMG on $dev mounted at $MNT (XFS, reflink): put golden/ and phones/ there"
