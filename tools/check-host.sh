#!/bin/bash
# Check this PC against vphone-linux's requirements (docs/SETUP.md). Changes nothing.
ok=0; warn() { echo "  ! $*"; ok=1; }; pass() { echo "  ok $*"; }
echo "vphone-linux host check"
arch=$(uname -m)
case $arch in
  x86_64) pass "x86_64" ;;
  aarch64) pass "aarch64 (the DGX Spark port: untested on a real Spark)"
    # Core types by MIDR part number (Linux cputype.h, util-linux lscpu): run-vphone.sh and
    # tools/pin-threads.sh keep QEMU on the fastest ones (highest cpu_capacity)
    cores=$(cat /sys/devices/system/cpu/cpu[0-9]*/regs/identification/midr_el1 2>/dev/null |
      awk '{p=substr($1, length($1)-3, 3); print (p=="d85" ? "Cortex-X925" : p=="d87" ? "Cortex-A725" : "part 0x" p)}' |
      sort | uniq -c | awk '{c=$1; sub(/^ *[0-9]+ /, ""); printf "%s%s x %s", (NR>1 ? ", " : ""), c, $0}')
    [ -n "$cores" ] && pass "cores: $cores" ;;
  *) warn "$arch; only x86_64 and aarch64 hosts are supported" ;;
esac
# reims-vgpu's GPU task processes map guest RAM in whole 16 KiB guest pages, finer than a
# 64 KiB-page kernel can map (Ubuntu arm64: boot linux-generic, not linux-generic-64k)
ps=$(getconf PAGESIZE)
[ "$ps" = 4096 ] && pass "4 KiB host pages" || warn "host page size $ps; a 4 KiB-page kernel is needed for GPU process isolation"
n=$(nproc); [ "$n" -ge 12 ] && pass "$n CPU threads" || warn "$n CPU threads; 12 or more recommended (the guest has 8 vCPUs, SMP= lowers it)"
mem=$(awk '/MemTotal/{print int($2/1048576)}' /proc/meminfo)
[ "$mem" -ge 15 ] && pass "${mem} GB RAM" || warn "${mem} GB RAM; 16 GB minimum"
shm=$(df -BG --output=avail /dev/shm | tail -1 | tr -dc 0-9)
[ "$shm" -ge 9 ] && pass "/dev/shm has ${shm} GB free" || warn "/dev/shm has ${shm} GB free; the guest's RAM (MEM, default 8G) lives there"
if command -v vulkaninfo >/dev/null; then
  g=$(vulkaninfo --summary 2>/dev/null | awk -F= '/deviceName/{print $2; exit}')
  case $g in
    "") warn "vulkaninfo found no Vulkan GPU" ;;
    *llvmpipe*) warn "Vulkan GPU:$g is the CPU renderer; install the NVIDIA driver's Vulkan part (DGX Spark: libnvidia-gl-580; community reports need driver 580.105.08 or newer for GB10)" ;;
    *) pass "Vulkan GPU:$g" ;;
  esac
else warn "vulkaninfo not installed (package vulkan-tools); can't check the GPU"; fi
[ -r /dev/vhost-vsock ] && [ -w /dev/vhost-vsock ] && pass "/dev/vhost-vsock usable" \
  || warn "/dev/vhost-vsock missing or not read/write for you (sudo modprobe vhost_vsock; see docs/SETUP.md)"
m=$(cat /proc/sys/vm/max_map_count)
[ "$m" -ge 1048576 ] && pass "vm.max_map_count = $m" || warn "vm.max_map_count = $m; set 1048576 or more (docs/SETUP.md)"
command -v cargo >/dev/null && pass "Rust (cargo)" || warn "Rust not installed (https://rustup.rs)"
command -v ninja >/dev/null && command -v meson >/dev/null && pass "meson and ninja" || warn "meson or ninja missing"
[ $ok = 0 ] && echo "All checks passed." || echo "Fix the items marked ! before building or booting."
exit $ok
