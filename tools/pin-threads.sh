#!/bin/bash
# pin-threads.sh PID [show]: place a running QEMU's threads on this host's cores (i7-14700K:
# P-cores 0-15 as 8 cores x 2 hyper-threads, siblings (0,1),(2,3)..; E-cores 16-27):
#  - "CPU n/TCG" (n < 8): logical CPU 2n, one physical P-core each, alone;
#  - GPU threads frames wait on (reims-vgpu.*, metal2vulkan-*): the P-cores' siblings 1,3,..,15;
#  - everything else: the E-cores.
# Busy vCPUs sharing a P-core's two hyper-threads each ran ~40% slower. Env: VCPU_CORES (the
# P-core list, default 0 2 4 6 8 10 12 14), GPU_CPUS (default 1,3,5,7,9,11,13,15), OTHER_CPUS.
# arm64 big.LITTLE hosts (DGX Spark: 10 Cortex-X925 + 10 Cortex-A725, one thread per core) rank
# cores by cpu_capacity: there the vCPUs take the first 8 fastest cores, GPU threads the other
# fastest ones (the slower cores when there are none), everything else the slower cores.
# SYSFS (default /sys) points the detection at another tree.
# pin-threads.sh --fast-cpus prints the cores run-vphone.sh keeps QEMU on by default (Intel
# hybrid: devices/cpu_core/cpus; else the fast tier below), empty on a uniform host. vphone-fleet
# gives them to the phone in use.
P=${1:?usage: pin-threads.sh PID [show] | pin-threads.sh --fast-cpus}
SYSFS=${SYSFS:-/sys}
# "<fast cpus> <slow cpus>" as taskset lists. Cores of one type can differ slightly (GB10's X925s
# top out at 3.9-4.0 GHz), so the tiers split halfway between the fastest and slowest capacity, and
# only when the slowest is over 10% below the fastest; nothing on a uniform host or without cpu_capacity.
# ponytail: two tiers; a three-tier host gets its top tier plus the middle one as "fast".
read -r fast slow < <(grep -H . "$SYSFS"/devices/system/cpu/cpu[0-9]*/cpu_capacity 2>/dev/null |
  sed -E 's#.*/cpu([0-9]+)/cpu_capacity:#\1 #' | sort -n |
  awk '{n[NR]=$1; c[NR]=$2+0; if (c[NR]>m) m=c[NR]; if (NR==1 || c[NR]<l) l=c[NR]}
       END{if (l*10 >= m*9) exit; for (i=1;i<=NR;i++) if (c[i]*2 >= m+l) f=f (f?",":"") n[i]; else s=s (s?",":"") n[i]; print f, s}')
if [ "$P" = --fast-cpus ]; then
  if [ -r "$SYSFS/devices/cpu_core/cpus" ]; then cat "$SYSFS/devices/cpu_core/cpus"; else echo "${slow:+$fast}"; fi
  exit 0
fi
if [ -n "$slow" ]; then
  IFS=, read -r -a F <<< "$fast"
  VCPU_CORES=${VCPU_CORES:-${F[*]:0:8}}
  OTHER_CPUS=${OTHER_CPUS:-$slow}
  GPU_CPUS=${GPU_CPUS:-$(IFS=,; echo "${F[*]:8}")}
  GPU_CPUS=${GPU_CPUS:-$OTHER_CPUS}
fi
read -r -a VC <<< "${VCPU_CORES:-0 2 4 6 8 10 12 14}"
GPU=${GPU_CPUS:-1,3,5,7,9,11,13,15}
OTHER=${OTHER_CPUS:-16-27}
for t in /proc/$P/task/*; do
  tid=${t##*/}; name=$(cat $t/comm 2>/dev/null) || continue
  case "$name" in
    "CPU "*"/TCG")
      n=${name#CPU }; n=${n%/TCG}
      if [ "$n" -lt "${#VC[@]}" ]; then cpus=${VC[$n]}; else cpus=$OTHER; fi ;;
    reims-vgpu.*|metal2vulkan*|qemu-system-aar) cpus=$GPU ;;
    *) cpus=$OTHER ;;
  esac
  if [ "$2" = show ]; then echo "$tid $name -> $cpus"; else taskset -pc "$cpus" "$tid" >/dev/null; fi
done
