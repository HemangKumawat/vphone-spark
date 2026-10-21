#!/bin/bash
# pin-threads.sh PID [show]: place a running QEMU's threads on this host's cores (i7-14700K:
# P-cores 0-15 as 8 cores x 2 hyper-threads, siblings (0,1),(2,3)..; E-cores 16-27):
#  - "CPU n/TCG" (n < 8): logical CPU 2n, one physical P-core each, alone;
#  - GPU threads frames wait on (reims-vgpu.*, metal2vulkan-*): the P-cores' siblings 1,3,..,15;
#  - everything else: the E-cores.
# Busy vCPUs sharing a P-core's two hyper-threads each ran ~40% slower. Env: VCPU_CORES (the
# P-core list, default 0 2 4 6 8 10 12 14), GPU_CPUS (default 1,3,5,7,9,11,13,15), OTHER_CPUS.
P=${1:?usage: pin-threads.sh PID [show]}
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
