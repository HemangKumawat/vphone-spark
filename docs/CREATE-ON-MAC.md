# Create the VM on a Mac (optional)

You don't need a Mac: [SETUP.md](SETUP.md) creates the VM on Linux. This page is for people who
already use [vphone-cli](https://github.com/Lakr233/vphone-cli) on an Apple Silicon Mac, or prefer to
create the VM there. It replaces [SETUP.md](SETUP.md) step 4; do steps 1-3 on Linux as usual (only
`./build.sh` from step 3 is needed for this route), then continue with step 5.

1. [What you need on the Mac](#1-what-you-need-on-the-mac)
2. [Create the VM](#2-create-the-vm)
3. [Copy it to Linux](#3-copy-it-to-linux)
4. [What the import does](#4-what-the-import-does)

## 1. What you need on the Mac

- An Apple Silicon Mac (M1 or newer) running macOS 15 or newer. A macOS VM does not work.
- About 100 GB free disk space while creating the VM (firmware and the VM itself).
- An internet connection: the restore fetches signing tickets from Apple.
- vphone-cli's security settings (SIP's research-guest mode), set once from macOS Recovery. See
  [vphone-cli's requirements](https://github.com/Lakr233/vphone-cli#requirements).

## 2. Create the VM

Install vphone-cli and create a VM, following its
[create-and-run guide](https://github.com/Lakr233/vphone-cli/blob/main/Documents/Guides/create-and-run.md):

```sh
vphone-cli host preflight
vphone-cli vm create myphone \
  --iphone-source /path/to/iPhone17,3_Restore.ipsw \
  --cloudos-source /path/to/cloudOS.ipsw
```

The firmware pair tested with vphone-linux is iPhone17,3 iOS **26.6.2 (23G90)** with cloudOS **26.4
(23E5207q)**; [CREATE-ON-LINUX.md's firmware step](CREATE-ON-LINUX.md#2-download-and-patch-the-firmware) has the
download links. vphone-cli's
[compatibility list](https://github.com/Lakr233/vphone-cli/blob/main/Documents/Guides/compatibility.md)
has the other pairs.

The VM ends up in `~/.vphone/machines/myphone/` on the Mac. Make sure it is stopped
(`vphone-cli vm stop myphone`), and don't run the same copy on the Mac and on Linux. If you want a
Mac reference boot, boot a clone.

What was tested this way:

- vphone-cli **2.9.4**, with Apple's boot ROMs (newer vphone-cli versions create machines on the
  open-source OpenAVB/OpenSEP ROMs; `run-vphone.sh` boots whichever `config.plist` names).
- vphone-cli's custom firmware (CFW) installed, which provides vphoned in the guest. vphone-linux's tools
  use vphoned to unlock the guest, launch apps and change services.
- The VM copied to Linux **before its first boot on the Mac**. With vphone-cli's default `vm create`,
  the VM boots once on the Mac to check it works, then stops, and is copied after that; both should
  work, but only the first was tested.

vphone-cli 2.9 creates machines from templates by default, and a template clone "gets a new ECID,
UDID and MAC address on its first start". Check that a clone that never started on the Mac carries
its final identity in `config.plist` before you rely on it.

If `cfw install` refuses with "Less than 50 GiB of disk space" on an external drive or a disk image
that has the space, `patches/vphone-cli-cfw-freespace.patch` makes vphone-cli fall back to the
volume's plain available capacity. It is a local patch for vphone-cli, not part of vphone-cli.

## 3. Copy it to Linux

On Linux, in the `vphone-linux` directory:

```sh
tools/import-vm.sh me@my-mac.local:.vphone/machines/myphone myphone
```

This copies the VM from the Mac over ssh into `vms/myphone` (a USB drive path works too) and prepares
it. It also keeps an untouched copy, `vms/myphone.pristine`, to start over from. Then continue with
[SETUP.md step 5](SETUP.md#5-start-it).

## 4. What the import does

`tools/import-vm.sh` does these steps, in case you copy a VM by hand.

vphone-cli keeps machines under `~/.vphone/machines/<name>/` by default (or in the library given with
`-l`). The launcher needs these files from the bundle, in one directory:

| File | What it is |
|---|---|
| `Disk.img` | the 64 GB root disk (sparse) |
| `nvram.bin` | Virtualization.framework's auxiliary storage: a 16 KiB header, then the firmware store the boot ROM reads |
| `AVPBooter.vresearch1.bin` or `OpenAVB.bin` | the AP boot ROM (`config.plist`'s `romImages` names it) |
| `AVPSEPBooter.vresearch1.bin` or `OpenSEP.bin` | the SEP boot ROM |
| `SEPStorage` | the SEP's persistent storage (512 KiB) |
| `config.plist` | the machine's configuration: screen size, machine identifier, ROM names |

Copy the bundle **sparse**, for example with `rsync -aS`. A plain copy writes all 64 GB.

Then add the files vphone-linux reads that vphone-cli does not write:

- **`ecid`** (required): the machine's ECID in hex, from `config.plist`'s `machineIdentifier`:

  ```sh
  VM=vms/myphone
  python3 -c 'import plistlib,sys; d=plistlib.load(open(sys.argv[1],"rb")); print(hex(plistlib.loads(d["machineIdentifier"])["ECID"]))' \
    $VM/config.plist > $VM/ecid
  ```

- **`serial`** (optional): the serial number Virtualization.framework reports for this machine.
  The recovery-mode USB serial string shows it as `SRNM:[...]`.
- **`gfx-res`** (optional): the display size in pixels. `448x932` is the recommended size at 1x
  (see [ADVANCED.md](ADVANCED.md#display-scale)). The default is the bundle's `screenConfig`,
  1290x2796.

Keep a pristine copy before the first boot. On btrfs or XFS, reflink copies cost nothing:
`cp -a --reflink=always vms/myphone vms/myphone.pristine`.

Never commit or publish anything from the bundle: it holds Apple firmware and your machine's identity.
`.gitignore` excludes `vms/`.
