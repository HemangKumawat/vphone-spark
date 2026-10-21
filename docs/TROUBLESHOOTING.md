# Troubleshooting

Problems seen while developing vphone-linux, and their fixes. Setup steps are in [SETUP.md](SETUP.md).

**The VM runs old GPU code after a reims-vgpu change.** QEMU links `libreims_vgpu.a` statically. Run
`ninja -C qemu/build qemu-system-aarch64` after every `make -C reims-vgpu`.

**"task N not created", "Trying to restart GPU" loops.** reims-vgpu's per-process GPU tasks map guest
RAM in whole guest pages, so the RAM must be a shared file mapping aligned to 2 MiB. With `GFX=1` the
launcher sets this up (`memory-backend-file,mem-path=/dev/shm,share=on,align=2M`). Check that
`/dev/shm` has room for `MEM`. Keep `GFX_ISOLATION=on` (the default): local GPU tasks cannot serve
iOS's 256 GiB user tasks.

**A GPU task process uses llvmpipe.** Each GPU task process must use the same Vulkan device as QEMU.
The launcher sets `VK_DRIVER_FILES` to NVIDIA's ICD. On other GPUs, set `VK_DRIVER_FILES` to your
ICD's JSON.

**vphoned returns empty replies.** vphoned drops a half-closed connection before it answers.
`tools/vphoned.sh` keeps its write side open (`STDIN,ignoreeof`). Do the same in your own clients.

**Uploads to the guest stall after 256 KiB.** With `VSOCK=vhost`, Linux caps a vsock socket's send
credit at its own buffer size (256 KiB by default). iOS sends a credit update only when its 512 KiB
window is nearly full, so it never does. Set `SO_VM_SOCKETS_BUFFER_SIZE` (and `_MAX_SIZE`) to at least
512 KiB on the host socket: `tools/vphoned.sh`, `tools/vphoned-update.sh` and `tools/guest-get.py` set
4 MiB.

**Downloading a file over 64 KiB fails** (`GET /v1/files/content` closes after the first chunk on
vhost-vsock). Use `tools/guest-read.py` (base64 over the `files.read` RPC). The alternative is the
`virtio-vsock-uds-pci` device from QEMU branch `vsock-uds` with `VSOCK=uds`.

**vphoned never answers on vsock.** In earlier builds the iOS virtio driver's rings overlapped when
QEMU's vhost-vsock queues were smaller than 256 entries. qemu-vphone now uses 256-entry queues. If you see
it again, `tools/vsock-rings.py SOCK` prints the ring layout. A vsock CID is released only some time
after its VM exits, so give a new VM a new `CID` when you restart quickly.

**Touches are ignored.** If backboardd logs `ignoring digitizer event ... builtIn: NO`, the
touchscreen is not marked built-in. Keep `VPHONE_XHCI_NONREMOVABLE` at its default `0x1fe`.

**The guest is sluggish at idle.** Disable background services ([ADVANCED.md](ADVANCED.md#background-services)). Look for crash loops with
the `logs.crashes` RPC, and look for a running ReportCrash.

**Black widgets or wallpaper.** Under heavy CPU starvation, FrontBoard's 10 s scene-create watchdog
kills the widget renderer and the wallpaper extension. Disabling services fixes it.

**NVIDIA Xid errors, a lost device, a frozen display.** Run the failing workload under the Vulkan
validation layer (`tools/validate-run.sh RUN CID BUNDLE_ID`) and report the errors. One such bug, a
transfer buffer freed while a copy into it was in flight, was fixed in the reims-vgpu fork.

**The VM died at an unsupported Metal feature.** By default the launcher runs reims-vgpu in census
mode (`REIMS_VGPU_CENSUS_DIR=runs/census`): a degradation that would kill the VM is recorded in
`census-<pid>.json` instead. `REIMS_VGPU_CENSUS_DIR=` (empty) restores the fatal behaviour.

**A corrupted VM disk.** The drives are opened with `file.locking=off` (the same `Disk.img` is
attached twice), so QEMU does not stop a second VM from opening the same disk. Never run two VMs
on one `Disk.img`. `tools/boot-test.sh` refuses to boot a disk that another QEMU holds. Work on reflink
copies.

**Huge logs, full `/tmp`.** `DFLAGS=int` and `-trace` patterns for the GIC write gigabytes per minute.
Stream them through a FIFO (as `tools/firstfault.py` does), or trace briefly.

**Stopping QEMU from a script.** Don't `pkill -f` a pattern that also matches your own shell's command
line. Find QEMU with `pgrep -x qemu-system-aar` (the kernel truncates the process name), or by its
`-qmp` socket path as `tools/boot-test.sh` does.

**Screenshots slow the guest.** QMP `screendump` runs under QEMU's global lock. PNG compression there
stalls every vCPU, so timing tools use PPM.

**Debugging the boot.** `DFLAGS=guest_errors,unimp LOG=q.log` logs unimplemented device accesses.
`GDB=1234` waits for `gdb -ex 'set architecture aarch64' -ex 'target remote :1234'`. Hardware
breakpoints do not fire on the SEP core (an extra CPU outside the SMP list). `tools/stuck.sh` and
`tools/firstfault.py` help when a boot stalls.
