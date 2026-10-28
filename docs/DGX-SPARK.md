# Running on an NVIDIA DGX Spark

How to set up a DGX Spark (GB10: 10 Cortex-X925 and 10 Cortex-A725 cores, 128 GB unified memory,
Linux aarch64) to run many virtual iPhones, each with its own data.

**Status: untested on Spark.** Nothing here has been built or booted on a DGX Spark yet. The
requirements below come from this repository's source, NVIDIA's documentation, community reports
and Ubuntu's standard procedures; where one has a source it is linked, and the rest is inferred. [SPARK-BOOT-CHECKLIST.md](SPARK-BOOT-CHECKLIST.md) is the order in which
to prove them, and [AGENT-BRINGUP.md](AGENT-BRINGUP.md) is the guide for whoever (person or AI agent)
does the bring-up.

## First: no App Store, no Apple ID

These are research VMs (Apple's `vresearch101` platform, the one Private Cloud Compute research VMs
use). App Store and Apple ID sign-in fail (vphone-cli
[#393](https://github.com/Lakr233/vphone-cli/issues/393),
[#326](https://github.com/Lakr233/vphone-cli/issues/326),
[#282](https://github.com/Lakr233/vphone-cli/issues/282)), and store apps such as Instagram and
TikTok crash at launch ([#212](https://github.com/Lakr233/vphone-cli/issues/212)). WhatsApp,
Instagram and other App Store apps cannot be installed. Only apps you build yourself (for example
with [xtool](https://github.com/xtool-org/xtool)) and sideload with `tools/install-ipa.py` run. This
does not depend on the host CPU: a Spark changes nothing here.

## Credits

This is a port of [YahiaAngelo/vphone-linux](https://github.com/YahiaAngelo/vphone-linux) (MIT,
based on commit `3678abe`), which runs [Lakr233/vphone-cli](https://github.com/Lakr233/vphone-cli)'s
virtual iPhone (MIT) on Linux. The emulator is
[YahiaAngelo/qemu-vphone](https://github.com/YahiaAngelo/qemu-vphone), built on
[ChefKissInc/Inferno](https://github.com/ChefKissInc/Inferno) (formerly QEMUAppleSilicon). The GPU
path is steelbrain's [reims-vgpu](https://github.com/steelbrain/reims-vgpu) and
[metal2vulkan](https://github.com/steelbrain/metal2vulkan) (GPL-2.0-or-later or LGPL-3.0-or-later). The VM platform is the one of
Apple's [Private Cloud Compute research environment](https://github.com/apple/security-pcc). Full
list: [README.md](../README.md#credits); licences: [LICENSING.md](LICENSING.md).

## What changes on a Spark, and what does not

- **Emulation only (TCG).** iOS needs Apple CPU features that no other Arm core has: the GXF
  guarded levels and SPRR permissions that SPTM and TXM use, and Apple's own system registers. The
  emulator implements them in software (`qemu/hw/vmapple/vresearch101.c`, `qemu/target/arm/`). KVM
  runs the guest on the host CPU and cannot add instructions the hardware lacks, so the Spark's
  Arm cores do not make iOS run natively. Whether a VMM could trap and emulate them under KVM is an
  open question ([AGENT-BRINGUP.md](AGENT-BRINGUP.md#open-question-hardware-virtualization)).
- **Speed: expect about the x86-64 numbers** (inferred, untested on Spark): a cold app launch about
  20 s, a warm one 4-8 s, unlock 1.5-5 minutes after start. The X925 is in the same single-thread
  class as the tested i7-14700K.
- **A bigger code cache.** QEMU's x86-64 code generator caps `TB_SIZE` at 2 GiB; the aarch64 one
  does not, so `run-vphone.sh` defaults to 4096 MiB on aarch64.
- **Core tiers.** `run-vphone.sh` and `tools/pin-threads.sh` find the fast cores from
  `cpu_capacity` at run time; `tools/check-host.sh` names X925 and A725 cores by their MIDR part
  numbers (`0xd85`, `0xd87`).
- **The fork's TCG speed tricks only ever ran on x86-64.** If a guest crashes, hangs or draws
  garbage on the Spark, start it with `VPHONE_SAFE_TCG=1` first. That turns off cross-page links,
  return-address prefetch and inline jump-cache lookup ([PERFORMANCE.md](PERFORMANCE.md#launcher-switches)).

## Host setup

DGX OS is based on Ubuntu 24.04 ([NVIDIA release notes](https://docs.nvidia.com/dgx/dgx-spark/release-notes.html)).
As root, once:

1. **A 4 KiB-page kernel.** `getconf PAGESIZE` must print `4096`. reims-vgpu's GPU task processes
   map guest RAM in whole 16 KiB guest pages, which a 64 KiB-page kernel cannot do. Do not boot the
   64 KiB "memory saver" kernel NVIDIA offers for CUDA workloads
   ([NVIDIA forum](https://forums.developer.nvidia.com/t/introducing-dgx-spark-memory-saver-more-memory-headroom-for-cuda-workloads/384884)).
   As a diagnostic only, `GFX_ISOLATION=off` runs the GPU work inside QEMU.
2. **Vulkan on the GB10 GPU.** Community reports need NVIDIA driver 580.105.08 or newer and the
   `libnvidia-gl-580` package for Vulkan
   ([gist](https://gist.github.com/solatticus/14313d9629c4896abfdf57aaf421a07a)). Check with
   `vulkaninfo --summary`: the device must be the NVIDIA GPU, not `llvmpipe`. `run-vphone.sh` looks
   for the ICD at `/usr/share/vulkan/icd.d/nvidia_icd.aarch64.json`, then `nvidia_icd.json`, then
   `/etc/vulkan/icd.d/nvidia_icd.json`; set `VK_DRIVER_FILES` if yours is elsewhere.
3. **vsock and memory maps** ([SETUP.md](SETUP.md#2-install-the-dependencies)):
   ```sh
   echo vhost_vsock | sudo tee /etc/modules-load.d/vhost-vsock.conf
   sudo modprobe vhost_vsock && sudo chmod 666 /dev/vhost-vsock
   echo 'vm.max_map_count = 1048576' | sudo tee /etc/sysctl.d/90-vphone.conf && sudo sysctl --system
   ```
4. **`/dev/shm` big enough** for every running phone's guest RAM (`MEM` each, with `GFX=1`):
   `sudo mount -o remount,size=100G /dev/shm` ([DENSITY.md](DENSITY.md#host-facts-that-change-with-the-phone-count)).
5. **A reflink filesystem for the phones.** `vms/` must be on btrfs or XFS with reflink. On ext4
   (inferred to be the DGX OS default; check with `df -T .`), `sudo tools/phones-volume.sh /data/vphones.img 1T vms` makes an XFS volume
   in a file ([DENSITY.md](DENSITY.md#tools)).
6. **Only for creating the VM on the Spark** (skip it when you copy a VM made on another Linux PC or
   a Mac: the bundle does not depend on the host CPU):
   - `vhci-hcd` and `usbip` for the restore over USB/IP (`sudo modprobe vhci-hcd`, package
     `linux-tools-generic` or `usbip`; [CREATE-ON-LINUX.md](CREATE-ON-LINUX.md#3-restore-over-usbip)).
   - The APFS driver built by `setup/setup.sh` from linux-apfs-rw plus
     `patches/linux-apfs-rw-ios27.patch`. With Secure Boot on (`mokutil --sb-state`), loading an
     unsigned module fails with `Key was rejected by service`. Sign it with the machine owner key
     (MOK) that Ubuntu's DKMS uses, and enrol that key once (Ubuntu's Secure Boot procedure, untested
     on Spark):
     ```sh
     ls /var/lib/shim-signed/mok/MOK.der || sudo update-secureboot-policy --new-key
     sudo mokutil --import /var/lib/shim-signed/mok/MOK.der   # set a one-time password, reboot,
                                                             # choose "Enroll MOK" in the blue screen
     sudo /usr/src/linux-headers-$(uname -r)/scripts/sign-file sha256 \
       /var/lib/shim-signed/mok/MOK.priv /var/lib/shim-signed/mok/MOK.der .build/linux-apfs-rw/apfs.ko
     ```
     Sign again after every kernel update (`setup/setup.sh` rebuilds the module for the new kernel).

`tools/check-host.sh` checks items 1-4 and the cgroup and zswap setup for parking. Run it after
every change.

## Building

```sh
./setup/setup.sh                       # dependencies, sources, QEMU with reims-vgpu, vphone-cli tooling
cd fleet && rustup target add aarch64-unknown-linux-musl && cargo spark   # vphone-fleet, vphone-api-proxy
```

Untested on Spark: the vphone-cli tooling should land in
`linux-create/.build/out/out/Products/Debug-linux-aarch64/` (inferred from the x86-64 path).
`cargo spark` makes static binaries in `fleet/target/aarch64-unknown-linux-musl/release/`.

## Many phones, each with its own data

[DENSITY.md](DENSITY.md) is the full design. In short:

- **Each phone's data lives only in `vms/phones/<id>/`.** Its `Disk.img` (app data, keychain,
  accounts, caches), `nvram.bin` and `SEPStorage` (the keys that protect the keychain) are reflink
  copies of the golden's, so every write stays in that phone's files and no phone sees another's.
- **Only `reset`, `restore` and `delete` destroy a phone's data,** and each names the phone; `reset`
  and `delete` need `--confirm <id>` as well. Nothing else writes a phone's state files but that
  phone's own QEMU.
- **Capture the golden before any app install or sign-in.** Every clone inherits what the golden
  holds, accounts and keychain items included.

| Identity | Per phone? | Status |
|---|---|---|
| Serial, MAC, vsock CID, API port | Yes, always (`vphone-fleet create`) | `tools/phone-up.sh` refuses a duplicate serial, MAC or CID; `create` never reuses an API port |
| Keychain, accounts, app data | Yes: the phone's own reflinks | checked on disk; untested booted on Spark |
| ECID and UDID | No: clones keep the golden's (`rotate_ecid = false`) | **Untested.** Whether a clone with a new ECID still boots is checklist step 8 |

The restore personalised the golden's firmware for its ECID, so a clone with a new ECID may not
boot. Until the checklist shows that it does, all clones share the golden's ECID and UDID.

## How many phones per Spark

An estimate from x86-64 measurements, not measured on a Spark
([DENSITY.md](DENSITY.md#how-many-phones-per-dgx-spark)): CPU is the limit. About 8 phones idling
with one light app each, or 4-6 in active use; RAM alone would allow about 20. Parking idle phones
(`vphone-fleet serve`) and zswap might raise that to 20-30
([DENSITY.md](DENSITY.md#how-many-phones-parking-buys)). `tools/density-bench.sh` measures the real
cost per phone; the checklist's last step runs it.
