#!/bin/bash
# scroll-measure.sh RUN CID: on a running VM (booted with REIMS_VGPU_STATS=1 and debug-threads),
# go Home, open Settings, scroll it twice to load its icons, then scroll its list up and down
# for ~15 s; print the guest frame rate per second (frames/longest gap), its average and the
# vCPU load.
cd "$(dirname "$0")/.."
R=$1; CID=$2
Q=runs/$R/qmp.sock
P=
for p in $(pgrep -f -- "-qmp unix:$PWD/runs/$R/"); do
  [ "$(cat /proc/$p/comm 2>/dev/null)" = qemu-system-aar ] && P=$p
done
# Home, then Settings: retry the tap until the screen changes (a tap during the Home animation
# is lost), and give up if Settings never opens.
tools/qmp.py $Q home; sleep 4
opened=
for try in 1 2 3; do
  tools/screen-changed.py $Q save "${TMPDIR:-/tmp}/screen-home.ppm"
  tools/qmp.py $Q tap 1089 1953; sleep 8
  if tools/screen-changed.py $Q "${TMPDIR:-/tmp}/screen-home.ppm"; then opened=1; break; fi
done
[ -n "$opened" ] || { echo "Settings did not open"; exit 1; }
for i in 1 2; do
  tools/qmp.py $Q swipe 645 2200 645 900 250 15; sleep 1.5
  tools/qmp.py $Q swipe 645 900 645 2200 250 15; sleep 1.5
done
sleep 3
n0=$(wc -l < runs/$R/stderr.log)
python3 tools/thread-cpu.py $P 14 2 > runs/$R/scroll-thr.txt &
for i in 1 2 3 4 5 6; do
  tools/qmp.py $Q swipe 645 2200 645 900 250 15; sleep 0.8
  tools/qmp.py $Q swipe 645 900 645 2200 250 15; sleep 0.8
done
wait; sleep 2
tail -n +$((n0+1)) runs/$R/stderr.log | grep -a "reims-vgpu stats" | sed -E 's/.*guest ([0-9.]+)\/s, max gap ([0-9.]+) ms.*/\1 \2/' |
  awk '$1>0 {n++; f+=$1; if ($2>g) g=$2; printf "%s/%s ", $1, int($2)} END {printf "\nframes/s avg %.1f over %d s, worst gap %d ms\n", f/n, n, g}'
awk '{s=0; for(i=5;i<=12;i++) s+=$i; print s/8}' runs/$R/scroll-thr.txt | awk '{t+=$1; n++} END {printf "vCPU busy avg %.0f%%\n", t/n}'
