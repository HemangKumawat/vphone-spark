#!/bin/bash
# Measure what one phone costs at each launch setting, to find how many phones fit on this host
# (docs/DENSITY.md, docs/SPARK-BOOT-CHECKLIST.md). Boots real phones; untested on Spark.
#
#   tools/density-bench.sh OUT.csv ID...
#
# ID... are phone folders made for the bench (vphone-fleet create b1 b2 --from GOLDEN): the bench
# boots them over and over, and iOS writes to their disks. For every combination of
#   MEMS     (default "3G 4G 6G")      guest RAM
#   SMPS     (default "2 4 8")         vCPUs
#   TB_SIZES (default "512 1024 2048") TCG code cache, MiB
#   GFXS     (default "0 1")           GPU device off or on
# it starts all the IDs at once through tools/phone-up.sh (lite profile; these variables override
# it), waits for each to unlock, lets them settle SETTLE seconds (default 60), samples them idle
# for 20 s, and stops them. One CSV row per phone and setting:
#   mem,smp,tb_size,gfx,phones,id,unlock_s,cpu_s_per_20s,pss_mib,ksm_run,ksm_pages_sharing,scope,status
# cpu_s_per_20s is the CPU time the phone used in the 20 s sample: spare cores x 20 / that is how
# many such phones fit idle. pss_mib is the phone's proportional memory: guest RAM (with GFX=1 a
# /dev/shm file), code cache, QEMU and its GPU processes. Both cover every process of the phone's
# systemd scope when it has one (scope=yes), else QEMU alone. KSM merges only GFX=0 phones' RAM,
# and only with /sys/kernel/mm/ksm/run = 1. A phone that does not unlock within BOOT_SECS (default
# 900) gets status no-unlock and its log path; the bench goes on with the next setting.
# Logs: runs/bench/<setting>/<id>.log. Stop vphone-fleet serve first: it would park the phones.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1
out=${1:?usage: density-bench.sh OUT.csv ID...}; shift
[ $# -gt 0 ] || { echo "usage: density-bench.sh OUT.csv ID..." >&2; exit 1; }
FLEET=${FLEET:-$PWD/vms} SETTLE=${SETTLE:-60} BOOT_SECS=${BOOT_SECS:-900}
if [ -e runs/fleet.lock ] && ! flock -n runs/fleet.lock true; then
  echo "density-bench: vphone-fleet serve is running and would park the phones: stop it first" >&2
  exit 1
fi
ids=("$@") cids=()
for id in "$@"; do
  cid=$(python3 -I -c 'import sys, tomllib; print(tomllib.load(open(sys.argv[1], "rb"))["cid"])' \
    "$FLEET/phones/$id/phone.toml") || { echo "density-bench: $FLEET/phones/$id is not a phone" >&2; exit 1; }
  cids+=("$cid")
done
tck=$(getconf CLK_TCK)
[ -s "$out" ] ||
  echo mem,smp,tb_size,gfx,phones,id,unlock_s,cpu_s_per_20s,pss_mib,ksm_run,ksm_pages_sharing,scope,status >"$out"

# Our own child, still running: bash reaps exited children by itself, so a pid seen later may
# belong to another process unless its parent is this shell.
alive() { local st; st=$(cat "/proc/$1/stat" 2>/dev/null) || return 1; st=${st##*) }
  [ "${st%% *}" != Z ] && [ "$(echo "$st" | cut -d' ' -f2)" = $$ ]; }
# The phone's processes: its scope's (QEMU and reims-vgpu's GPU processes), else QEMU's pid.
cg=()
procs() { if [ -n "${cg[$1]}" ]; then cat "${cg[$1]}/cgroup.procs"; else echo "${pids[$1]}"; fi; }
cpu_us() {
  if [ -n "${cg[$1]}" ]; then awk '/^usage_usec/{print $2}' "${cg[$1]}/cpu.stat"
  else awk -v t="$tck" '{printf "%d", ($14 + $15) * 1e6 / t}' "/proc/${pids[$1]}/stat"; fi
}
pids=()
stop_all() {
  local p live
  for _ in $(seq 10); do
    live=()
    for p in "${pids[@]}"; do alive "$p" && live+=("$p"); done
    [ ${#live[@]} -gt 0 ] || break
    kill "${live[@]}" 2>/dev/null
    sleep 1
  done
  for p in "${pids[@]}"; do alive "$p" && kill -9 "$p" 2>/dev/null; done
  wait
  pids=()
}
trap 'stop_all; exit 130' INT TERM

for MEM in ${MEMS:-3G 4G 6G}; do for SMP in ${SMPS:-2 4 8}; do
for TB in ${TB_SIZES:-512 1024 2048}; do for GFX in ${GFXS:-0 1}; do
  tag=mem$MEM-smp$SMP-tb$TB-gfx$GFX logs=runs/bench/$tag
  mkdir -p "$logs"
  echo "== $tag: $# phones" >&2
  start=$SECONDS
  for id in "$@"; do
    MEM=$MEM SMP=$SMP TB_SIZE=$TB GFX=$GFX WINDOW=0 tools/phone-up.sh "$id" lite >"$logs/$id.log" 2>&1 &
    pids+=("$!")  # phone-up.sh execs down to QEMU: this is QEMU's pid
  done
  # Poll every phone each round, so each gets its own unlock time (to one round) and one phone
  # that never unlocks does not use up the others' time.
  unlock=() left=$#
  for i in "${!pids[@]}"; do unlock[i]=; done
  while [ "$left" -gt 0 ] && [ $((SECONDS - start)) -lt "$BOOT_SECS" ]; do
    left=0
    for i in "${!pids[@]}"; do
      if [ -n "${unlock[i]}" ] || ! alive "${pids[i]}"; then continue; fi
      if CID=${cids[i]} timeout --foreground 90 tools/vphoned.sh POST /v1/rpc \
          '{"method":"screen.unlock","params":{},"id":1}' 2>/dev/null | grep -q '"locked":false'; then
        unlock[i]=$((SECONDS - start))
      else
        left=$((left + 1))
      fi
    done
    [ "$left" = 0 ] || sleep 10
  done
  sleep "$SETTLE"
  for i in "${!pids[@]}"; do
    s=$(cat "runs/${ids[i]}/scope" 2>/dev/null)
    cg[i]=${s:+$(find /sys/fs/cgroup -type d -name "$s" -print -quit 2>/dev/null)}
  done
  # Every phone's CPU time read back to back, 20 s apart, scaled to the measured interval.
  before=() after=()
  t0=$(date +%s%N)
  for i in "${!pids[@]}"; do before[i]=$(cpu_us "$i" 2>/dev/null); done
  sleep 20
  for i in "${!pids[@]}"; do after[i]=$(cpu_us "$i" 2>/dev/null); done
  t1=$(date +%s%N)
  ksm_run=$(cat /sys/kernel/mm/ksm/run 2>/dev/null || echo na)
  ksm=$(cat /sys/kernel/mm/ksm/pages_sharing 2>/dev/null || echo na)
  i=0
  for id in "$@"; do
    status=ok cpu='' pss=''
    if ! alive "${pids[i]}"; then status="exited:$logs/$id.log"
    elif [ -z "${unlock[i]}" ]; then status="no-unlock:$logs/$id.log"; fi
    if alive "${pids[i]}" && [ -n "${before[i]}" ] && [ -n "${after[i]}" ]; then
      cpu=$(awk -v a="${before[i]}" -v b="${after[i]}" -v ns=$((t1 - t0)) \
        'BEGIN{printf "%.1f", (b - a) / 1e6 * 20e9 / ns}')
      pss=$(procs "$i" | while read -r p; do cat "/proc/$p/smaps_rollup" 2>/dev/null; done |
        awk '/^Pss:/{k += $2} END{printf "%d", k / 1024}')
    fi
    scope=no; [ -z "${cg[i]}" ] || scope=yes
    echo "$MEM,$SMP,$TB,$GFX,$#,$id,${unlock[i]},$cpu,$pss,$ksm_run,$ksm,$scope,$status" | tee -a "$out"
    [ "$status" = ok ] || echo "density-bench: $id at $tag: $status" >&2
    i=$((i + 1))
  done
  stop_all
done; done; done; done
