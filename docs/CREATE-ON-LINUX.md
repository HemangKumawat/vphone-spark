# Create the VM on Linux

The whole VM is made on Linux, with no Mac and no file from one: vphone-cli's firmware tooling built
for Linux (`linux-create/`) downloads and patches the firmware, restores it over USB/IP, and installs
the custom firmware (CFW) through an SSH ramdisk. The boot ROMs are **OpenAVB** and **OpenSEP**, the
open-source replacements for Apple's `AVPBooter` and `AVPSEPBooter` that vphone-cli ships.

> **Status (2026-10-24):** done end to end on the test host. A blank bundle was restored through
> OpenAVB's DFU mode, given the CFW, and booted to the home screen: vphoned answered 49 s after start,
> the lock screen unlocked at 89 s, and the first boot's data migration took about 20 minutes more.
> Every input is made by a script in `tools/` or downloaded from its source; nothing comes from a
> Mac. Several steps need root, so the route is still a series of commands rather than one.

**Before you start:** do [SETUP.md](SETUP.md) steps 2 and 3 first: install its packages, clone
this repository, and run `./build.sh`. That fetches the submodules and builds QEMU and the GPU
library, which every step below uses (the restores run in QEMU too). Then come back here.

Something not working? The Linux-creation entries in [TROUBLESHOOTING.md](TROUBLESHOOTING.md#creating-the-vm-on-linux)
cover what went wrong on the test PC.

The steps:

1. [Extra dependencies](#1-extra-dependencies)
2. [Build vphone-cli for Linux and usbmuxd](#2-build-vphone-cli-for-linux-and-usbmuxd)
3. [Make an empty VM bundle](#3-make-an-empty-vm-bundle)
4. [Download and patch the firmware](#4-download-and-patch-the-firmware)
5. [Restore over USB/IP](#5-restore-over-usbip)
6. [Boot the SSH ramdisk](#6-boot-the-ssh-ramdisk)
7. [Install the CFW](#7-install-the-cfw)
8. [Finish and first boot](#8-finish-and-first-boot)

Everything below uses `vms/` as the library and `myphone` as the VM. `VC` is the Linux vphone-cli:

```sh
VC=$PWD/linux-create/.build/out/out/Products/Debug-linux-x86_64/vphone-cli
```

## 1. Extra dependencies

On top of the packages in [SETUP.md](SETUP.md#2-on-linux-install-the-dependencies) (Fedora names):

```sh
sudo dnf install usbip fuse-sshfs fuse3 7zip kernel-devel unzip curl zstd \
  libusb1-devel libplist-devel libimobiledevice-glue-devel openssl-devel libcurl-devel \
  libimobiledevice-devel libusbmuxd-devel libtatsu-devel libarchive-devel
```

- **Swift 6.4.1** for Linux (tested with `swift-6.4.1-RELEASE`, installed with
  [swiftly](https://www.swift.org/install/linux/)).
- **rcodesign** (apple-codesign 0.29.0) for the bundle seals the CFW makes:
  `cargo install apple-codesign --locked`. The installer finds it on `PATH`, in `~/.cargo/bin` of
  the user who ran sudo, or at `$VPHONE_RCODESIGN`.
- **linux-apfs-rw** ([github.com/linux-apfs/linux-apfs-rw](https://github.com/linux-apfs/linux-apfs-rw)),
  built against the running kernel into `.build/linux-apfs-rw/`. The CFW install mounts the cryptex
  images read-only with it (it tries `modprobe apfs`, then `$VPHONE_APFS_KO`, then
  `.build/linux-apfs-rw/apfs.ko`).
- **Root** for: the usbmuxd swap, USB/IP attach, building the SSH ramdisk, and `cfw install`.
- **Disk space: about 45 GB free** on a reflink-capable file system (btrfs, XFS), not tmpfs. The
  restore tree takes about 12 GB, the restored disk about 9 GB and 15-16 GB after the CFW, and the
  CFW install needs about 8 GB of scratch space in `/var/tmp`. **A restore failed at 12%
  (`format_media`) with 17 GB free and passed with 27 GB.**
- **Network.** The IPSWs come from Apple's CDN (about 1-3 MiB/s on the test host; the 11.3 GB
  iPhone IPSW took about an hour).

Checked in a clean Fedora 44 container (2026-10-24): every package above and in SETUP.md installs,
every library `linux-create` links against is then present, and `tools/build-usbmuxd-32k.sh`
builds. The Swift and Rust builds themselves were checked on the test PC only.

## 2. Build vphone-cli for Linux and usbmuxd

**vphone-cli.** `linux-create/` builds vphone-cli **2.12.1**'s firmware tooling (`fw`, `restore`,
`cfw`, `sign`, ...) for Linux without changing vphone-cli's files: `sync-sources.sh` links them from
a vphone-cli checkout and adds Linux stand-ins (`Shims/`), replacement files (`overlay/`) and
one-line edits (`edits/`).

```sh
git clone https://github.com/Lakr233/vphone-cli.git && git -C vphone-cli checkout 2.12.1
cd linux-create
VPHONE_CLI=../vphone-cli ./sync-sources.sh
swift build --build-system swiftbuild --scratch-path .build/out
cd ..
```

**The vphone-cli release bundle.** Download vphone-cli's **2.12.1** release zip (the macOS app; it is
only unzipped here, nothing in it runs on the host). It provides:

- `OpenAVB/OpenAVB.bin` and `OpenSEP/OpenSEP.bin`, the boot ROMs;
- `guest-resources/`, the prebuilt arm64e guest binaries the CFW installs (vphoned, the launchd hook,
  the sound driver and others).

```sh
mkdir -p .build/vphone-release-2.12.1/app
unzip -q VPhone-2.12.1.zip -d .build/vphone-release-2.12.1/app
R=.build/vphone-release-2.12.1/app/VPhone.bundle/Contents/Resources
ln -sfn $R/guest-resources guest-resources
mkdir -p OpenAVB OpenSEP
cp $R/OpenAVB/OpenAVB.bin OpenAVB/ && cp $R/OpenSEP/OpenSEP.bin OpenSEP/
```

The release and the vphone-cli checkout must be the same version.

**QEMU** is the one `./build.sh` builds ([SETUP.md](SETUP.md#3-build)); check that it exists:

```sh
ls qemu/build/qemu-system-aarch64     # missing? run ./build.sh first
```

Its `vphone` branch also has the virtual USB device
controllers a restore needs (`hw/vmapple/vusb.c`, which OpenAVB, iBoot and AVPBooter drive in DFU
and recovery mode, and `hw/vmapple/vusb-pci.c`, the RestoreOS's virtio USB controller) and the
auxiliary virtio-blk drive.

**The 32 KiB usbmuxd.** The RestoreOS's `AppleUSBDeviceMux` reads 32 KiB buffers and drops a USB
transfer that does not hold exactly one mux packet. Stock usbmuxd sends 48 KiB packets, which reset
the guest mid-restore. Build a copy with a 32 KiB MTU once:

```sh
tools/build-usbmuxd-32k.sh        # -> .build/usbmuxd-32k/usbmuxd
```

## 3. Make an empty VM bundle

```sh
tools/new-vm.sh vms/myphone          # a random ECID; or: tools/new-vm.sh vms/myphone <ECID in hex>
```

It writes what vphone-cli's `vm new` would, with the ROMs from step 2:

| File | Contents |
|---|---|
| `OpenAVB.bin`, `OpenSEP.bin` | Copies of the ROMs. |
| `config.plist` | vphone-cli's machine configuration: `vresearch101`, 8 CPUs, 8 GiB (`CPUS=`, `MEM_GIB=`), the 1290x2796 screen, `romImages` naming the two ROMs (`run-vphone.sh` boots those), NAT with a random MAC, and a `machineIdentifier` holding the ECID (Virtualization.framework's format: a binary plist `{ECID: …}`). |
| `ecid`, `udid-prediction.txt` | The ECID, for `run-vphone.sh` and for `restore` (so it needs no `--ecid`). |
| `Disk.img` | Empty, sparse, 64 GiB. |
| `nvram.bin` | Zeros, 32 MiB + 16 KiB. |
| `SEPStorage` | Zeros, 512 KiB. |

The name must not start with a dot (vphone-cli refuses it). Apple's signing server accepted a
random ECID from this script in testing (`restore --get-shsh`).

With an empty `nvram.bin`, OpenAVB finds no firmware to boot and drops into DFU mode
(`OpenBootROM: virtual firmware source not found`), which is where a restore starts.

Use fresh zeroed `SEPStorage` for each new restore. A VM whose SEP storage had been through many
restores panicked in the SEP key store at its first boot (`SEP Panic: :sks`).

## 4. Download and patch the firmware

The VM is made from two of Apple's firmware files (IPSWs), the pair vphone-cli recommends and that
was tested end to end:

| File | Size | Download |
|---|---|---|
| iPhone 16 (iPhone17,3) iOS **26.6.2 (23G90)** | 11.3 GB | [iPhone17,3_26.6.2_23G90_Restore.ipsw](https://updates.cdn-apple.com/2026SummerFCS/29d685ce-f70d-45a0-9823-b1cd115f3927/iPhone17,3_26.6.2_23G90_Restore.ipsw) |
| cloudOS **26.4 beta (23E5207q)** | 1.2 GB | [cloudOS 26.4 23E5207q](https://updates.cdn-apple.com/private-cloud-compute/c0ecdb4b310cf5239ab2b248dd3098eec297dc5aa3bbe6ada27273262b0b8b64) (the link has no file name; save it as below) |

Both come straight from Apple's update servers; vphone-cli lists the same links (`$VC fw urls
--device iPhone17,3`, and its firmware catalog for the cloudOS). The cloudOS 26.4 beta is the newest
cloudOS that still has the `vphone600ap` firmware the VM needs, so later cloudOS versions don't
work. Other iOS versions may work with it but were not tested here (`$VC fw catalog`).

**4.1 Download them** into an `ipsws/` folder in this checkout (git ignores it). `-C -` resumes a
broken download; the iPhone file took about an hour on the test PC:

```sh
mkdir -p ipsws
curl -L -C - -o ipsws/iPhone17,3_26.6.2_23G90_Restore.ipsw \
  'https://updates.cdn-apple.com/2026SummerFCS/29d685ce-f70d-45a0-9823-b1cd115f3927/iPhone17,3_26.6.2_23G90_Restore.ipsw'
curl -L -C - -o ipsws/cloudOS-26.4-23E5207q.ipsw \
  'https://updates.cdn-apple.com/private-cloud-compute/c0ecdb4b310cf5239ab2b248dd3098eec297dc5aa3bbe6ada27273262b0b8b64'
ls -l ipsws/      # 11254382122 and 1199454323 bytes
```

If you already have them elsewhere (another disk is fine), use those paths below instead.

**4.2 Start the USB helpers** (root, in two terminals, and leave them running through step 7; both
the GPU driver below and the restore in step 5 need them):

```sh
sudo tools/usbmuxd-32k.sh          # stops and runtime-masks the system usbmuxd, runs the 32 KiB one
sudo tools/usbip-autoattach.sh     # attaches the VM's USB device at each stage (DFU, iBEC, RestoreOS)
```

**4.3 Get the GPU driver.** It ships in the cloudOS IPSW's AEA-encrypted cryptex, and Apple's key
server no longer gives out the key for 23E5207q, so take it from a stock cloudOS restore instead, as
vphone-cli does on a Mac:

```sh
tools/gpu-driver-from-cloudos.sh ipsws/cloudOS-26.4-23E5207q.ipsw drivers/
# -> drivers/AppleParavirtGPUMetalIOGPUFamily.bundle
```

This restores the stock cloudOS into a temporary VM (about 20 minutes), reads the driver from its
System volume with 7-Zip (no root, no mount), and deletes the VM. It needs about 5 GB free while it
runs. On the test PC it gave the same driver as vphone-cli on a Mac (SHA-1 `b5d95b38…`). You need it
once; keep `drivers/` for later VMs.

**4.4 Merge and patch the firmware** into the VM from step 3:

```sh
$VC fw prepare -l vms myphone \
  -i ipsws/iPhone17,3_26.6.2_23G90_Restore.ipsw -c ipsws/cloudOS-26.4-23E5207q.ipsw \
  --gpu-driver-bundle drivers/AppleParavirtGPUMetalIOGPUFamily.bundle
$VC fw patch -l vms myphone
```

`fw prepare` merges both IPSWs into a restore tree inside the VM (`vms/myphone/*_Restore`, about
12 GB); `fw patch` patches its boot chain. After that the IPSWs are only needed to make another VM,
so you can move or delete them. (`-i` and `-c` also take the URLs directly; vphone-cli then
downloads into `~/.vphone/ipsws`. Step 4.3 needs the cloudOS file on disk, so the explicit download
is simpler.)

- **Run `fw patch` every time after `fw prepare`.** `fw prepare` writes stock firmware; on the test
  host a re-prepared tree went unpatched and nothing booted past iBoot. `restore` later prints
  "recorded N boot-chain patches", but that number comes from the patch plan, not from the files.
  To check a tree: `FirmwareOriginals/` exists in the bundle, and the research kernel differs from the
  IPSW's (patched, it is about 43 MB instead of 14 MB).
- Don't set `VPHONE_NO_LZFSE`: OpenAVB only accepts an LZFSE-compressed iBSS, which `fw patch`
  writes by default.

## 5. Restore over USB/IP

The restore runs vphone-cli's idevicerestore against the guest, which QEMU presents as a USB device:

```text
guest (OpenAVB DFU → iBSS/iBEC → RestoreOS)
  └─ QEMU vusb / vusb-pci ── chardev ──▶ tools/vusb-usbip.py (USB/IP server, 127.0.0.1:3240)
       └─ usbip attach (vhci_hcd) ──▶ usbmuxd (32 KiB) ──▶ vphone-cli restore
```

1. Keep the two USB helpers from step 4.2 running.

2. Start the VM in DFU (add `GFX=1 WINDOW=1` to watch it in a window):

   ```sh
   tools/dfu-up.sh vms/myphone runs/myphone-dfu
   $VC recovery-probe                 # [+] iPhone99,11 in DFU, ECID 0x...
   ```

3. Restore (this **erases** `Disk.img`; about 25 minutes on the test host):

   ```sh
   VPHONE_RESTORE_TIMEOUT_SCALE=10 $VC restore -l vms myphone -v
   ```

   A good run logs `Describing iBSS to OpenAVB`, ends with `Status: Restore Finished`, and leaves a
   firmware store in `nvram.bin` (`HUFA` at 0x4000, iLLB, `auto-boot=true`). Stop the VM afterwards.
   `VPHONE_RESTORE_TIMEOUT_SCALE` stretches idevicerestore's device waits for emulation.

On the test host one restore attempt failed early (the NVRAM recorded
`restore-outcome=initial_engine_failure_no_return`) and the next one, unchanged, passed. If a
restore hangs at "Waiting for device to enter restore mode", stop it, zero `nvram.bin` again and
retry.

## 6. Boot the SSH ramdisk

Linux cannot write the guest's System volume: it is sealed, and linux-apfs-rw refuses read-write
mounts of sealed volumes. So the guest's own kernel mounts its volumes from an SSH ramdisk (dropbear,
`root`/`alpine`), and the CFW install reaches them over sshfs.

**Build the SSH ramdisk image** once per firmware, while the restore tree is still in full mode
(right after step 5):

```sh
tools/ssh-ramdisk-stage.sh                              # -> runs/sshramdisk/ssh-root
bash tools/ssh-ramdisk-rebuild-signed.sh vms myphone    # -> runs/sshramdisk/out/094-39914-031.dmg (asks for sudo once)
```

- `ssh-ramdisk-stage.sh` downloads the SSH tools (dropbear, bash, coreutils, sftp-server for iOS)
  from SSHRD_Script's [sshtars](https://github.com/verygenericname/sshtars) at a pinned commit,
  checks their SHA-256, and unpacks them. This repository doesn't redistribute them. They are not
  re-signed: the trust cache built in the next step admits them as they ship.
- `ssh-ramdisk-rebuild-signed.sh` saves the tree's stock restore ramdisk to `runs/sshramdisk/` the
  first time, mounts a copy of its APFS volume (root, linux-apfs-rw), adds the tools, and repacks it.
- The tested ramdisk also carried a PRNG seeder (`prng_seedctl`), which exited without effect; the
  script leaves it out.

**Switch the restore tree to the SSH ramdisk:**

```sh
tools/restore-tree-mode.sh vms myphone ssh      # `full` undoes it
```

This swaps in the SSH ramdisk, builds a trust cache that admits its tools, forces TXM's selector-24
check to pass (`fw patch --restore-txm-precheck`), makes the patched research kernel the restore
kernel, and updates the manifest. Keep the tree in `full` mode for a full restore.

**Boot it.** With `auto-boot=false`, OpenAVB loads the restored iLLB and iBoot stops at its recovery
prompt (`05ac:1281`); `restore --boot-ramdisk` boots the ramdisk from there:

```sh
python3 tools/nvram-autoboot.py vms/myphone/nvram.bin --auto-boot false
tools/dfu-up.sh vms/myphone runs/myphone-sshrd
VPHONE_RESTORE_TIMEOUT_SCALE=10 VPHONE_RESTORE_BOOT_ARGS_EXTRA="serial=3" \
  $VC restore -l vms myphone --boot-ramdisk -v
tools/ssh-ramdisk.sh 'id'                                  # uid=0(root)
tools/ssh-ramdisk.sh '/bin/bash /usr/bin/mount_filesystems; /sbin/mount'
```

The ramdisk shows up as `05ac:12a8` after a few minutes. The command can keep waiting for lockdownd
after that; once `ssh-ramdisk.sh` works, it can be stopped.

`mount_filesystems` mounts by role. On the test VM: System `disk1s1` at `/mnt1`, Data at `/mnt2`,
Preboot `disk1s5` at `/mnt6`, and xART `disk1s3` at `/mnt7` (it holds the gigalocker, a 6 MB
`<uuid>.gl`). Check with `/sbin/mount` and `ls`: the device numbers differ between restores.

## 7. Install the CFW

With the ramdisk up and its volumes mounted:

```sh
sudo $VC cfw install -l vms myphone --work-parent /var/tmp --keep-artifacts
```

- The installer mounts the guest's System, xART and Preboot on the host with sshfs, through
  `tools/sshfs-remote-sftp.sh` (the ramdisk's dropbear has no SFTP subsystem, so the wrapper runs
  `/usr/libexec/sftp-server` as a command). The guest paths default to `/mnt1`, `/mnt7` and `/mnt6`;
  if `mount_filesystems` put them elsewhere, pass
  `sudo VPHONE_GUEST_SYSTEM=… VPHONE_GUEST_XART=… VPHONE_GUEST_PREBOOT=… $VC cfw install …`.
- It decrypts the OS and App cryptexes in process, mounts them read-only with linux-apfs-rw, and
  copies them into System. That took about 16 minutes on the test host. The App cryptex is many
  small files, and the host disk barely grows for minutes at a time while it copies; it is not stuck.
- It renames the xART gigalocker to `AA.gl`, installs vphoned and the launchd hook, and ends with
  `[+] Linux guest install complete` and a new `PatchReceipt.plist`.
- Without `--keep-artifacts` the restore tree is deleted at the end (12 GB back; a new restore then
  needs `fw prepare` again).

If an install fails halfway, look for leftover root sshfs mounts (`findmnt -l | grep vphone-cfw`)
and unmount them with `fusermount3 -u` before retrying.

## 8. Finish and first boot

1. Unmount the guest volumes from the ramdisk so nothing is lost, then stop the VM:

   ```sh
   tools/ssh-ramdisk.sh 'sync; /sbin/umount /mnt2; /sbin/umount /mnt6; /sbin/umount /mnt1; sync'
   ```

   xART at `/mnt7` stays busy; `sync` is enough for it.
2. Make the kernel boot the installed system, not the restore's snapshot, and turn auto-boot back on
   (the VM must be stopped):

   ```sh
   $VC fw snapshot-flip -l vms myphone
   python3 tools/nvram-autoboot.py vms/myphone/nvram.bin --auto-boot true
   ```

   Without the snapshot flip, launchd panics with `Library not loaded: /usr/lib/libSystem.B.dylib`.
3. If you kept the restore tree: `tools/restore-tree-mode.sh vms myphone full`.
4. Give the system usbmuxd back: `sudo kill $(cat /run/usbmuxd-32k.pid)`, then
   `sudo systemctl unmask --runtime usbmuxd.service`.
5. Keep a pristine copy (`cp -a --reflink=always vms/myphone vms/myphone.pristine`), then boot as in
   [SETUP.md](SETUP.md#5-start-it) (`GFX=1 WINDOW=1 ./run-vphone.sh vms/myphone`).

**The first boot** shows the Apple logo with a progress bar while iOS migrates its data. Under
emulation this took about 20 minutes on the test host; vphoned answers well before it ends (it
reports `setup_pending: true`). When the progress bar is gone, skip Setup Assistant and unlock:

```sh
tools/vphoned.sh POST /v1/rpc '{"method":"setup.skip","params":{"force":true},"id":1}'
tools/vphoned.sh POST /v1/rpc '{"method":"screen.unlock","params":{},"id":1}'
```

Then, on a new VM:

- **Set Auto-Lock to Never** (Settings › Display & Brightness › Auto-Lock). A new VM locks and turns
  the screen off after 60 s, and iOS then drops every touch (backboardd logs
  `ignoring touch event ((backlight dim/off))`): it looks as if touch is broken. `screen.unlock`
  wakes it. vphoned's `display.auto_lock.set` did not change the effective value on the test VM.
- **Lower the display scale.** A new VM draws at iOS's default 3x (1290x2796). Shut it down, then
  patch LLB to 1x and set the matching display size ([ADVANCED.md](ADVANCED.md#display-scale)):

  ```sh
  tools/vphoned.sh POST /v1/rpc '{"method":"system.shutdown","params":{"force":true},"id":1}'
  python3 tools/display-scale.py vms/myphone/nvram.bin 1
  echo 448x932 > vms/myphone/gfx-res
  ```

  Then, with the VM running again, `python3 tools/guest-1x-logo.py 3` restores the boot Apple logo.

  On the test host the Linux-made VM then booted at 1x to the home screen (vphoned at 68 s,
  unlocked at 153 s).
