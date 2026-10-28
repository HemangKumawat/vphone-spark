#!/bin/bash
# Check on disk, without booting, that two phones cloned from one golden share nothing a write can
# reach (docs/DENSITY.md): a write into phone A's state files changes neither phone B's nor the
# golden's, and unshares only the block written.
#
#   tools/phone-isolation-check.sh GOLDEN PHONE-A PHONE-B
#
# For each of Disk.img, nvram.bin and SEPStorage: find a block A and B both share (it fails if A
# shares none: a full copy, not a reflink), hash B's and the golden's file, write a marker block there in
# A, then require that B's and the golden's hashes are unchanged, that A now shares exactly one
# block fewer and that B still shares that block. A's original block is written back after, so A
# keeps its data; that one block stays unshared. None of the three may be running. Hashing reads
# each file whole: about four minutes for 64 GiB disks.
set -euo pipefail
G=${1:?usage: phone-isolation-check.sh GOLDEN PHONE-A PHONE-B}
A=${2:?usage: phone-isolation-check.sh GOLDEN PHONE-A PHONE-B}
B=${3:?usage: phone-isolation-check.sh GOLDEN PHONE-A PHONE-B}
G=$(realpath -e "$G"); A=$(realpath -e "$A"); B=$(realpath -e "$B")
fail() { echo "FAIL: $*" >&2; exit 1; }
# the shared blocks of $1 as "first last" per extent (filefrag -v's logical blocks), and the block size
shared() { filefrag -v "$1" | awk -F'[:. ]+' '/^ *[0-9]+: / && /shared/ {print $3, $4}'; }
blocks() { shared "$1" | awk '{n += $2 - $1 + 1} END {print n + 0}'; }
is_shared() { shared "$1" | awk -v b="$2" '$1 <= b && b <= $2 {f = 1} END {exit !f}'; }
# A's original block goes back however the check ends
orig=
restore() { [ -z "$orig" ] || dd if="$orig" of="$A/$f" bs="$bs" seek="$first" conv=notrunc,fsync status=none; rm -f "$orig"; orig=; }
trap restore EXIT
trap 'exit 130' INT TERM

# everything that can fail without writing, before the first write
for f in Disk.img nvram.bin SEPStorage; do
  for d in "$G" "$A" "$B"; do [ -f "$d/$f" ] || fail "$d/$f is missing"; done
  [ "$(stat -c %a "$G/$f")" = 444 ] || fail "$G/$f is mode $(stat -c %a "$G/$f"), the golden's state files are 0444"
  for x in "$A" "$B"; do
    [ "$(stat -c %d:%i "$x/$f")" != "$(stat -c %d:%i "$G/$f")" ] || fail "$x/$f is the golden's own file (a hard link)"
  done
  [ "$(stat -c %d:%i "$A/$f")" != "$(stat -c %d:%i "$B/$f")" ] || fail "$A/$f and $B/$f are one file (a hard link)"
  shared "$A/$f" | grep -q . || fail "$A/$f shares no block: a full copy, not a reflink of the golden"
done
busy=$(find /proc/[0-9]*/fd -maxdepth 1 -type l -printf '%l\n' 2>/dev/null |
  grep -Fx -m1 -f <(printf '%s\n' {"$G","$A","$B"}/{Disk.img,nvram.bin,SEPStorage})) || true
[ -z "$busy" ] || fail "$busy is open: stop that phone first"

for f in Disk.img nvram.bin SEPStorage; do
  # a block both phones still share (an earlier check left one block of each phone unshared)
  first=$(awk 'NR == FNR {lo[NR] = $1; hi[NR] = $2; n = NR; next}
    {for (i = 1; i <= n; i++) {l = lo[i] > $1 ? lo[i] : $1; h = hi[i] < $2 ? hi[i] : $2; if (l <= h) {print l; exit}}}' \
    <(shared "$A/$f") <(shared "$B/$f"))
  [ -n "$first" ] || fail "$A/$f and $B/$f share no block: one of them is not a reflink of the golden"
  bs=$(filefrag -v "$A/$f" | sed -nE 's/.*blocks of ([0-9]+) bytes.*/\1/p')
  before=$(blocks "$A/$f")
  sums=$(sha256sum "$B/$f" "$G/$f")
  orig=$(mktemp)
  dd if="$A/$f" of="$orig" bs="$bs" skip="$first" count=1 status=none
  printf 'vphone isolation marker %s\n' "$f" | dd of="$A/$f" bs="$bs" seek="$first" conv=notrunc,sync,fsync status=none
  after=$(blocks "$A/$f")
  bad=
  [ "$(sha256sum "$B/$f" "$G/$f")" = "$sums" ] || bad="the write into $A/$f changed $B/$f or $G/$f"
  [ -n "$bad" ] || [ "$after" = $((before - 1)) ] || bad="$A/$f shared $before blocks, $after after one block was written"
  [ -n "$bad" ] || is_shared "$B/$f" "$first" || bad="$B/$f no longer shares block $first"
  restore
  [ -z "$bad" ] || fail "$bad"
  echo "ok $f: block $first written in A, A shares $before -> $after blocks, B and golden unchanged"
done
echo "PASS: $A and $B are isolated reflinks of $G"
