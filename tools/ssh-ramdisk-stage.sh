#!/bin/bash
# Make the SSH ramdisk's staging tree (runs/sshramdisk/ssh-root) from SSHRD_Script's tools.
#
#   tools/ssh-ramdisk-stage.sh [out-dir]       (default: runs/sshramdisk/ssh-root)
#
# Downloads ssh.tar.gz (dropbear, bash, coreutils, sftp-server, ... for iOS) from
# github.com/verygenericname/sshtars at a pinned commit and checks its SHA-256. This repository does
# not redistribute those binaries. Then:
#   - removes usr/share/misc/magic.mgc and usr/trollstore/ (not needed). It keeps
#     usr/lib/libresolv.9.dylib: sftp-server needs it, and the stock restore ramdisk has none.
# The binaries stay exactly as they ship (they are not re-signed): they run because
# tools/ssh-ramdisk-trustcache.py adds their code hashes to the ramdisk's trust cache and
# `fw patch --restore-txm-precheck` lets TXM admit them (tools/restore-tree-mode.sh does both).
# This reproduces the staging tree the tested SSH ramdisk was built from, without its optional PRNG
# seeder (usr/libexec/prng_seedctl, which exited without effect there). No root needed.
set -euo pipefail
cd "$(dirname "$0")/.."
OUT=${1:-runs/sshramdisk/ssh-root}
URL=https://github.com/verygenericname/sshtars/raw/9f9551a14a268148d920370635e9c1bce33bf49c/ssh.tar.gz
SHA256=ddfa230acd2789c7e61ddb0d2ec3df9a6c741f2ddcab58fc1a826d278be1d74d
CACHE=runs/sshramdisk/ssh.tar.gz

[ -e "$OUT" ] && { echo "$OUT exists; remove it first" >&2; exit 1; }
mkdir -p "$(dirname "$CACHE")"
if [ ! -f "$CACHE" ]; then
  echo "=== downloading ssh.tar.gz (SSHRD_Script's sshtars)"
  curl -sSfL -o "$CACHE.part" "$URL"
  mv "$CACHE.part" "$CACHE"
fi
sha256sum "$CACHE" | grep -q "^$SHA256" || { echo "$CACHE: unexpected SHA-256" >&2; exit 1; }

echo "=== unpacking into $OUT"
mkdir -p "$OUT"
tar -xzf "$CACHE" -C "$OUT"
rm -f "$OUT/usr/share/misc/magic.mgc"
rm -rf "$OUT/usr/trollstore"

echo "=== $OUT: $(find "$OUT" -type f | wc -l) files"
