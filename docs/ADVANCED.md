# Advanced setup

The details [SETUP.md](SETUP.md) leaves out.

## Building by hand

`./build.sh` runs these steps.

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
| `qemu` | [YahiaAngelo/qemu-vphone](https://github.com/YahiaAngelo/qemu-vphone) | [ChefKissInc/QEMUAppleSilicon](https://github.com/ChefKissInc/QEMUAppleSilicon) `master` @ `cc4302a9` | `vphone` | `3ed85774c2989addec3056a866991762b60c4b5c` |
| `reims-vgpu` | [YahiaAngelo/reims-vgpu](https://github.com/YahiaAngelo/reims-vgpu) | [steelbrain/reims-vgpu](https://github.com/steelbrain/reims-vgpu) `development` @ `56eed1eb` | `vphone` | `8ca9b8fb22c29f0e7ca66bdb8afa28f692361006` |
| `metal2vulkan` | [YahiaAngelo/metal2vulkan](https://github.com/YahiaAngelo/metal2vulkan) | [steelbrain/metal2vulkan](https://github.com/steelbrain/metal2vulkan) `development` @ `6b322fb5` | `vphone` | `fb0bd570ecfead540ce82b670a52c7245ccb616c` |

reims-vgpu's `Cargo.toml` has a `[patch]` section that takes metal2vulkan from `../metal2vulkan`, so the
`vphone` metal2vulkan is used only when both submodules sit side by side, as they do here.

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

## A VM made on a Mac

Creating the VM with vphone-cli on a Mac, copying it over, and the files the launcher needs from it
are in [CREATE-ON-MAC.md](CREATE-ON-MAC.md).

## Display scale

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
test VM now runs at 1x.

**The boot logo at 1x.** iOS draws the boot Apple logo from `ProgressUI.framework`'s
`apple-logo*@2x/@3x~iphone.png`, which have no 1x versions, so at 1x the screen stays black until the
lock screen. `tools/guest-1x-logo.py CID` writes 1x versions, halved from the guest's own @2x
images (nothing is copied in from outside). Its system volume is read-only, so the tool first runs
`/sbin/mount -u -w /` through a one-shot launchd job; the volume is read-only again after the next boot.

**At 1x, keep the width a multiple of 32** (rows a multiple of 128 bytes). NVIDIA's linear images
need 128-byte rows. At 430 pixels wide (1720-byte rows), iOS's full-screen surfaces were shadow-copied
in and out, about 10 screens per frame; at 448 the runtime aliases them instead. Copies fell from
about 246 million to 1.3 million texels per 30 s at idle.
Touch coordinates in `tools/qmp.py` are always given on the 1290x2796 grid, whatever the scale.

## Background services

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

## Host GPU clocks

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
