#!/bin/bash
# Boot the VM, wait, then show where every CPU is (pc, sp, lr, cpsr) and the
# code around CPU0's pc.   stuck.sh <vm-dir> <seconds> [extra qemu args]
VM=$1; WAIT=${2:-90}; shift 2
PORT=$((12400 + RANDOM % 500))
SMP=${SMP:-8} timeout $((WAIT + 60)) "$(dirname "$0")/../run-vphone.sh" "$VM" -gdb tcp::$PORT "$@" > "$VM/stuck.out" 2>&1 &
sleep "$WAIT"
cmd=$(mktemp)
cat > "$cmd" <<GDB
set pagination off
set architecture aarch64
target remote :$PORT
thread apply all printf "pc=%#lx sp=%#lx lr=%#lx cpsr=%#lx\n", \$pc, \$sp, \$x30, \$cpsr
thread 1
printf "ELR_EL1=%#lx ESR_EL1=%#lx FAR_EL1=%#lx\n", \$ELR_EL1, \$ESR_EL1, \$FAR_EL1
printf "TTBR0_EL1=%#lx TTBR1_EL1=%#lx TCR_EL1=%#lx\n", \$TTBR0_EL1, \$TTBR1_EL1, \$TCR_EL1
x/12i \$ELR_EL1-24
x/16i \$pc-32
kill
GDB
gdb -q -batch -x "$cmd" 2>&1 | grep -E "^Thread|pc=|ELR_EL1|TTBR|Error|No symbol|^ *0x|=>"
rm -f "$cmd"
wait
