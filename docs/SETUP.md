# Setup

This guide takes you from an empty Linux machine to an iOS home screen. Creating the VM needs an
Apple Silicon Mac once; after that the VM runs on Linux only.

1. [Host requirements](#1-host-requirements)
2. [Dependencies](#2-dependencies)
3. [Get the sources](#3-get-the-sources)
4. [Build](#4-build)
5. [Create the VM on a Mac](#5-create-the-vm-on-a-mac)
6. [Copy the bundle to Linux](#6-copy-the-bundle-to-linux)
7. [First boot](#7-first-boot)
8. [Display scale](#8-display-scale)
9. [Recommended guest settings](#9-recommended-guest-settings)
10. [Host GPU clocks](#10-host-gpu-clocks)
11. [Troubleshooting](#11-troubleshooting)

## 1. Host requirements

| | Tested | Notes |
|---|---|---|
| OS | Fedora 44, kernel 7.2, x86_64 | Other distributions should work; only Fedora 44 was tested. |
| CPU | Intel i7-14700K (8 P-cores + 12 E-cores, 28 threads) | The guest has 8 vCPUs (one host thread each), plus the SEP core, the GPU and I/O threads. On a hybrid CPU the launcher pins QEMU to the P-cores. |
| RAM | 62 GB | The guest's RAM (`MEM`, default 8G) lives in `/dev/shm`, so `/dev/shm` must have room for it. Add QEMU's code cache (up to 2 GB, `TB_SIZE`) and the GPU processes. 16 GB is a guess at the minimum; it has not been measured. |
| GPU | NVIDIA RTX 4080 SUPER, driver 615.71.09 | Any Vulkan GPU that reims-vgpu supports. Only NVIDIA was tested with an iOS guest. AMD GPUs need `/dev/udmabuf` access (reims-vgpu's README). |
| Disk | | The VM disk is a 64 GB sparse file, about 15 GB used after restore. The build trees take tens of GB: reims-vgpu's `target/` reached 38 GB on the test host after many builds, and QEMU's build is about 0.6 GB. |
| Kernel | `vhost_vsock` module, `/dev/vhost-vsock` read/write for your user | Used by the default vsock datapath (`VSOCK=vhost`). On the test host `/dev/vhost-vsock` is mode 0666. |
| sysctl | `vm.max_map_count = 1048576` | reims-vgpu maps guest memory in many pieces. reims-vgpu's README suggests `2097152` for guests up to 30 GiB. |

No `/dev/kvm` is needed: an arm64 guest on an x86_64 host runs under TCG.

## 2. Dependencies

The package names are Fedora's. They are the packages the test host has for QEMU's configure options
and reims-vgpu's build.

```sh
sudo dnf install git gcc gcc-c++ clang binutils make cmake ninja-build meson python3 \
  glib2-devel pixman-devel lzfse-devel libslirp-devel libssh-devel libzstd-devel \
  nettle-devel gnutls-devel gtk3-devel sdl2-compat-devel bzip2-devel \
  libX11-devel wayland-devel libxkbcommon-devel \
  vulkan-loader vulkan-loader-devel vulkan-tools \
  socat python3-pillow perf gdb
# Optional: Vulkan validation (tools/validate-run.sh)
sudo dnf install vulkan-validation-layers
```

Also needed:

- reims-vgpu's README lists clang, lld and binutils for its `cargo xtask dist`. The test host
  built without the `lld` package.
- **Rust**, through [rustup](https://rustup.rs). reims-vgpu pins its toolchain in
  `rust-toolchain.toml` (1.98.1 at the pinned commit); rustup installs it on the first build.
- **The GPU driver's Vulkan ICD.** On NVIDIA, the launcher uses
  `/usr/share/vulkan/icd.d/nvidia_icd.x86_64.json` when it exists.
- **Network access during the build.** Cargo fetches crates, and QEMU's meson fetches some
  subprojects (for example `keycodemapdb`).

The list comes from the test host; a clean install may need more.

## 3. Get the sources

```sh
git clone https://github.com/YahiaAngelo/vphone-linux.git
cd vphone-linux
git submodule update --init            # qemu, reims-vgpu, metal2vulkan
git -C qemu submodule update --init util/mlib
```

Do **not** use `--recursive`. reims-vgpu has its own `vendor/qemu` submodule, a separate QEMU tree that
vphone-linux does not use, and QEMU's `roms/` submodules are not needed for this machine.

The submodules are pinned to these commits (each fork's `vphone` branch):

| Submodule | Fork | Upstream | Fork branch | Commit |
|---|---|---|---|---|
| `qemu` | [YahiaAngelo/qemu-vphone](https://github.com/YahiaAngelo/qemu-vphone) | [ChefKissInc/QEMUAppleSilicon](https://github.com/ChefKissInc/QEMUAppleSilicon) `master` @ `cc4302a9` | `vphone` | `bae5f26d605c65771dd3864deb48918daa8b0095` |
| `reims-vgpu` | [YahiaAngelo/reims-vgpu](https://github.com/YahiaAngelo/reims-vgpu) | [steelbrain/reims-vgpu](https://github.com/steelbrain/reims-vgpu) `development` @ `56eed1eb` | `vphone` | `8ca9b8fb22c29f0e7ca66bdb8afa28f692361006` |
| `metal2vulkan` | [YahiaAngelo/metal2vulkan](https://github.com/YahiaAngelo/metal2vulkan) | [steelbrain/metal2vulkan](https://github.com/steelbrain/metal2vulkan) `development` @ `6b322fb5` | `vphone` | `fb0bd570ecfead540ce82b670a52c7245ccb616c` |

reims-vgpu's `Cargo.toml` has a `[patch]` section that takes metal2vulkan from `../metal2vulkan`, so the
`vphone` metal2vulkan is used only when both submodules sit side by side, as they do here.

## 4. Build

The order matters, because QEMU links reims-vgpu **statically**.

**1. reims-vgpu** (also builds the patched metal2vulkan through the Cargo patch):

```sh
make -C reims-vgpu
```

On Linux this runs `cargo xtask dist` for the Vulkan platform. The output is
`reims-vgpu/target/dist-vulkan/` (`lib/libreims_vgpu.a`, `include/reims_vgpu.h`,
`lib/pkgconfig/reims-vgpu.pc`). The static library is about 800 MB, so expect a long first build.

**2. QEMU**, configured once against that pkg-config file. This is the configure line the test host
uses (from `qemu/build/config.log`):

```sh
mkdir -p qemu/build && cd qemu/build
PKG_CONFIG_PATH=$(realpath ../../reims-vgpu/target/dist-vulkan/lib/pkgconfig) \
  ../configure --target-list=aarch64-softmmu --enable-pvg --enable-lzfse --enable-slirp \
  --enable-libssh --enable-zstd --enable-nettle --enable-gnutls --enable-gtk --enable-sdl \
  --disable-werror --disable-qom-cast-debug
cd ../..
ninja -C qemu/build qemu-system-aarch64
```

`--enable-pvg` builds the `apple-gfx-mmio` device on reims-vgpu's C API. Meson reads `reims-vgpu.pc`
only at configure time, so run configure again if that file changes.

**After every reims-vgpu rebuild, relink QEMU**, or the VM runs the old GPU code:

```sh
make -C reims-vgpu && ninja -C qemu/build qemu-system-aarch64
```

If ninja does not notice the new archive, remove the binary first:
`rm qemu/build/qemu-system-aarch64 && ninja -C qemu/build qemu-system-aarch64`.

## 5. Create the VM on a Mac

The VM is created, restored and patched by **vphone-cli** on an Apple Silicon Mac. Follow its
documentation; this section only says what vphone-linux needs from it.

- vphone-cli README: <https://github.com/Lakr233/vphone-cli>
- Host setup (SIP and research-guest settings): `Documents/Guides/host-setup.md`
- Creating a VM: `Documents/Guides/create-and-run.md`
- Tested firmware pairings: `Documents/Guides/compatibility.md`

What was tested with vphone-linux:

- vphone-cli **2.9.4**.
- iPhone restore IPSW **iPhone17,3 26.6.2 (23G90)** with the cloudOS IPSW **26.4 (23E5207q)**. Per
  vphone-cli's compatibility notes, 26.4-23E5207q is the newest cloudOS that has the `vphone600ap`
  identity the guest needs.
- vphone-cli's custom firmware (CFW) installed, which provides vphoned in the guest. vphone-linux's tools
  use vphoned to unlock the guest, launch apps and change services.
- The bundle was copied to Linux **before its first boot on the Mac**.

vphone-cli 2.9 creates machines from templates by default, and a template clone "gets a new ECID,
UDID and MAC address on its first start". Check that a clone that never started on the Mac carries
its final identity in `config.plist` before you rely on it. Not yet documented: the exact
`vphone-cli vm create` options used for the test VM (with or without `--no-template`, and the
preset).

If `cfw install` refuses with "Less than 50 GiB of disk space" on an external drive or a disk image
that has the space, `patches/vphone-cli-cfw-freespace.patch` makes vphone-cli fall back to the
volume's plain available capacity. It is a local patch for vphone-cli, not part of vphone-cli.

Do not boot the same bundle on the Mac and on Linux. If you want a Mac reference boot, boot a clone.

## 6. Copy the bundle to Linux

vphone-cli keeps machines under `~/.vphone/machines/<name>/` by default (or in the library given with
`-l`). The launcher needs these files from the bundle, in one directory:

| File | What it is |
|---|---|
| `Disk.img` | the 64 GB root disk (sparse) |
| `nvram.bin` | Virtualization.framework's auxiliary storage: a 16 KiB header, then the firmware store AVPBooter reads |
| `AVPBooter.vresearch1.bin` | the AP boot ROM |
| `AVPSEPBooter.vresearch1.bin` | the SEP boot ROM |
| `SEPStorage` | the SEP's persistent storage (512 KiB) |
| `config.plist` | the machine's configuration: screen size, machine identifier |

Copy the bundle **sparse**, for example with `rsync -aS`. A plain copy writes all 64 GB.

Then add the files vphone-linux reads that vphone-cli does not write:

- **`ecid`** (required): the machine's ECID in hex, from `config.plist`'s `machineIdentifier`:

  ```sh
  VM=vms/myphone
  python3 -c 'import plistlib,sys; d=plistlib.load(open(sys.argv[1],"rb")); print(hex(plistlib.loads(d["machineIdentifier"])["ECID"]))' \
    $VM/config.plist > $VM/ecid
  ```

- **`serial`** (optional): the serial number Virtualization.framework reports for this machine.
  The recovery-mode USB serial string shows it as `SRNM:[...]`. Not yet documented: where to read it
  on a normally booted machine.
- **`gfx-res`** (optional): the display size in pixels. `448x932` is the recommended size at 1x
  (section 8).
  The default is the bundle's `screenConfig`, 1290x2796.

Keep a pristine copy before the first boot. On btrfs or XFS, reflink copies cost nothing:
`cp -a --reflink=always vms/myphone vms/myphone.pristine`.

Never commit or publish anything from the bundle: it holds Apple firmware and your machine's identity.
`.gitignore` excludes `vms/`.

## 7. First boot

> **NVIDIA hosts: lock the GPU clocks first, after every host reboot** (section 10). Otherwise the
> GPU stays at idle clocks and every frame waits for it.
>
> ```sh
> sudo nvidia-smi --lock-gpu-clocks=2000,3105
> sudo nvidia-smi --lock-memory-clocks=10000,11501
> ```

```sh
GFX=1 WINDOW=1 ./run-vphone.sh vms/myphone
```

- `GFX=1` attaches the paravirtual GPU. `WINDOW=1` opens reims-vgpu's window (SDL3, X11 or Wayland),
  which shows the screen and takes mouse input as touches. Without `WINDOW=1` the VM is headless, and
  you can still use QMP screenshots and input.
- The terminal shows the guest's serial console (the verbose boot log). The QEMU monitor is
  multiplexed on it: Ctrl-A C switches to the monitor, Ctrl-A X quits.
- The VM gets vsock CID 3 (`CID=`); give every running VM its own CID.

Every switch of the launcher is in [PERFORMANCE.md](PERFORMANCE.md#launcher-switches). To drive
the VM from scripts, add a QMP socket:

```sh
GFX=1 WINDOW=1 ./run-vphone.sh vms/myphone -qmp unix:$PWD/runs/qmp.sock,server,nowait
```

On the test host the boot took about 1-2 minutes to vphoned and 3-5 minutes to an unlocked home
screen.

**Talk to vphoned** (in another terminal):

```sh
tools/vphoned.sh GET /v1/health                  # "status":"ok", plus setup_pending
```

**Skip Setup Assistant** if `/v1/health` reports `setup_pending: true` (a VM that never booted on the
Mac). The skip persists on the disk:

```sh
tools/vphoned.sh POST /v1/rpc '{"method":"setup.skip","params":{"force":true},"id":1}'
```

**Unlock:**

```sh
tools/vphoned.sh POST /v1/rpc '{"method":"screen.unlock","params":{},"id":1}'
```

Right after vphoned comes up, the unlock can fail with "Lock Screen did not dismiss in time". Retry
after 15 s (`tools/boot-test.sh` retries for you). A swipe up from the bottom edge also unlocks.

**Use it:** click to tap and drag to scroll. Swipe up from the bottom edge, or middle-click, to go Home.
Over QMP: `tools/qmp.py runs/qmp.sock tap X Y`, `swipe ...`, `home`, `shot out.png`
([TOOLS.md](TOOLS.md)).

**Stop it:** shut the guest down through vphoned, or quit QEMU with Ctrl-A X:

```sh
tools/vphoned.sh POST /v1/rpc '{"method":"system.shutdown","params":{"force":true},"id":1}'
```

## 8. Display scale

iOS draws the 1290x2796 screen at 3x by default. Under emulation every pixel costs, so a smaller
framebuffer with a lower UI scale gives the same 430x932-point layout with far fewer pixels:

| Scale | `gfx-res` | Pixels vs 3x |
|---|---|---|
| 3x (default) | 1290x2796 | 1 |
| 2x | 860x1864 | 1/2.25 |
| 1x | 448x932 (430x932 rounded up to a multiple of 32) | about 1/8.6 |

The scale comes from LLB, which fills `/chosen/display-scale` from the boot video settings.
`tools/display-scale.py` patches that one instruction in the firmware store inside `nvram.bin`. This
is the same patch vphone-cli applies for iPads (`IBootPatchDisplayScale`). Stop the VM first and keep
a backup of `nvram.bin`:

```sh
python3 tools/display-scale.py vms/myphone/nvram.bin        # show the current state
python3 tools/display-scale.py vms/myphone/nvram.bin 1      # 1x
echo 448x932 > vms/myphone/gfx-res                          # matching display size
python3 tools/display-scale.py vms/myphone/nvram.bin 0      # undo (back to 3x)
```

Measured on Settings scrolling with a quiet guest: 3x 36.6 frames/s average with about 50-60% vCPU load,
2x 37.9 frames/s with about 40%. The frame rate barely moved, but the CPU and GPU load dropped. The main
test VM now runs at 1x. Known issue: at 1x the boot shows a black screen instead of the Apple logo.

**At 1x, keep the width a multiple of 32** (rows a multiple of 128 bytes). NVIDIA's linear images
need 128-byte rows. At 430 pixels wide (1720-byte rows), iOS's full-screen surfaces were shadow-copied
in and out, about 10 screens per frame; at 448 the runtime aliases them instead. Copies fell from
about 246 million to 1.3 million texels per 30 s at idle.
Touch coordinates in `tools/qmp.py` are always given on the 1290x2796 grid, whatever the scale.

## 9. Recommended guest settings

Under TCG the guest's background work competes with the UI for the 8 vCPUs. On the test VM these
settings made the most difference.

**Disable unneeded services** through vphoned (`services.disable` needs `force`). On the test VM, with
iOS's defaults, all 8 vCPUs were about 80% busy at rest and SpringBoard drew no frames during swipes.
After disabling about 60 services, idle guest CPU fell from 92-104 to 32 CPU-seconds per 20 s.

```sh
for s in duetexpertd modelcatalogd analyticsd momentsd siriinferenced PerfPowerServices \
         icloudmailagent cloudd itunescloudd geoanalyticsd audioanalyticsd; do
  tools/vphoned.sh POST /v1/rpc \
    "{\"method\":\"services.disable\",\"params\":{\"label\":\"com.apple.$s\",\"force\":true},\"id\":1}"
done
```

- `services.enable` undoes it. launchd honours the override from the next boot, and
  `processes.kill` stops a running daemon now.
- **Check `logs.crashes` after disabling anything.** Disabling `com.apple.financed` alone made passd
  crash every ~22 s, and each crash ran ReportCrash. Disable financed and passd together, or neither.
- Keep `dasd`, `audiomxd` and `mobileassetd`: other services depend on them.
- About 43 more cloud, iCloud, Find My, Siri, Home and analysis services were disabled on the test VM
  (among them `homed`, `voicebankingd`, `mediaanalysisd`, `heard`, `symptomsd.distributed`).
  The full list is not published yet.
- vphoned also has a built-in profile, `services.profile.apply {"profile":"trimmed"}` (see vphone-cli's
  `Research/vphoned_http_api.md`). Untested with vphone-linux.

**In Settings:**

- Accessibility > Motion > **Reduce Motion** on.
- Accessibility > Display & Text Size > **Reduce Transparency** on.
- Display & Brightness > **Auto-Lock: Never**, so test runs don't lock.

## 10. Host GPU clocks

**Lock the clocks while the VM runs.** The settings do not survive a reboot: apply them again after
every host reboot.

The guest's GPU load comes in short bursts. NVIDIA's driver reads this as idle and keeps the GPU at its
lowest clocks: on the test host the RTX 4080 SUPER stayed in P8 (210 MHz graphics, 405 MHz memory).
Every guest frame then waits for slow GPU work, and each frame's window draw took about 3 ms. Locking the clocks fixed it. Guest GPU work fell from
420 µs to 140 µs (median) and 4.4 ms to 1.2 ms (90th percentile). The window's draw fell from
2960 µs to 388 µs (median).

```sh
sudo nvidia-smi --lock-gpu-clocks=2000,3105
sudo nvidia-smi --lock-memory-clocks=10000,11501
# undo:
sudo nvidia-smi --reset-gpu-clocks --reset-memory-clocks
```

These values are for the RTX 4080 SUPER. Use your GPU's supported range (`nvidia-smi -q -d SUPPORTED_CLOCKS`).
A milder option that needs no root is `nvidia-settings -a "[gpu:0]/GpuPowerMizerMode=1"`, which
helped partly (`=0` reverts).

## 11. Troubleshooting

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

**The guest is sluggish at idle.** Disable background services (section 9). Look for crash loops with
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
