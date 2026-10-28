# Many phones on one host

Many phones share one read-only base image (the golden). Each phone keeps its own data: app data,
caches, sign-ins and accounts survive its restarts, and no phone can see or change another's. This
page covers the on-disk layout, each phone's identity, the launch profiles, the tools, and an
estimate of how many phones fit on a DGX Spark.

**Status: untested on Spark.** The layout and the tools were checked on fake sparse images on
btrfs, without booting (see [Checks](#checks)). The phone counts are inferred from x86-64
measurements, not measured on a Spark.

## Why a reflink copy holds all of a phone's state

A phone keeps all its state in three files, and `run-vphone.sh` opens each one twice:

- `Disk.img`, as pflash and as the `root-pci` disk. The iOS data volume is inside it: app
  containers, the keychain and caches.
- `nvram.bin`, as pflash and as the `aux-pci` disk.
- `SEPStorage`, the SEP's storage. The keychain's keys are wrapped by the SEP.

A reflink copy is a copy-on-write file copy on btrfs or XFS. The copy shares the original's blocks
until one side writes to them. So a phone made from reflink copies of the golden's three files
holds every one of its own writes, and shares nothing once a block is written. qcow2 overlays would
not work here, because each file is opened twice.

## Layout

```text
vms/                        FLEET (tools/phone-up.sh). Must be one btrfs or reflink XFS filesystem
├── golden/<name>/          read-only, never booted as a phone
│   ├── Disk.img nvram.bin SEPStorage        mode 0444
│   ├── MANIFEST            sha256sum of the three state files, written once when the golden is captured
│   └── config.plist ecid udid-prediction.txt gfx-res OpenAVB.bin OpenSEP.bin
└── phones/<id>/            one phone. <id>: letters, digits, . _ -
    ├── Disk.img nvram.bin SEPStorage        reflink copies of the golden's, writable
    ├── config.plist ecid udid-prediction.txt gfx-res OpenAVB.bin OpenSEP.bin   copies of the golden's
    ├── serial mac          this phone's own identity (see Identity)
    ├── phone.toml          id, golden, golden_sha256, cid, profile
    └── snapshots/<name>/   optional: reflink copies of the three state files, taken while the phone is off
runs/<id>/                  run time: qmp.sock, census/
```

`phone.toml`:

```toml
id = "p01"
golden = "ios26-base"            # the folder under vms/golden/
golden_sha256 = "…"              # sha256 of that golden's MANIFEST
cid = 10                         # vsock CID: unique per phone, 3 or more
profile = "lite"                 # optional: the default profile for tools/phone-up.sh
```

Phones have no host ports yet: user networking (`-nic user`) opens none on the host. A `ports`
table gets added along with the first host port forward.

**Rules:**

- Only a phone's own QEMU writes its files.
- Nothing resets or replaces a phone's state files except an explicit `reset`, `restore` or
  `delete` that names that phone.
- The golden is never booted. To change the base, boot a writable copy of it and capture that copy
  as a new golden under a new name. Existing phones stay on the old golden: their reflinks keep its
  blocks alive.
- Capture the golden **before** any app install or sign-in. Every clone inherits whatever the
  golden holds, accounts and keychain items included.
- A snapshot can only be taken with the phone off. A running state cannot be saved: the GPU device
  blocks migration (`apple-gfx.c`), and the Apple devices have no vmstate.

## Identity

| Identity | Where it comes from | Per phone? |
|---|---|---|
| ECID | `ecid` (from `config.plist`'s `machineIdentifier`), passed as `-M vresearch101,ecid=` | No: clones keep the golden's (`rotate_ecid = false`) |
| UDID | `udid-prediction.txt`, derived from the ECID; the restore reads it | No: it follows the ECID. `phone-up.sh` refuses a phone whose two ECIDs differ |
| Serial | `serial`, passed as `-M vresearch101,serial=` (default `1234` for every VM) | Yes, always |
| MAC | `mac`, passed as `-nic user,mac=` (default `52:54:00:12:34:56` for every VM) | Yes, always |
| vsock CID | `phone.toml` `cid` | Yes, always |
| Keychain, accounts, app data | inside `Disk.img`, keys wrapped by `SEPStorage` | Yes: the phone's own reflinks |

`config/profiles.toml` `[identity]` sets the policy. Clones keep the golden's ECID until a clone
with a new ECID is shown to boot, because the restore personalised the golden's firmware for that
ECID. Testing that is on the Spark checklist. Two caveats:

- A MAC matters only once the guest uses the network.
- The serial reaches the machine's config region (`hw/vmapple/cfg.c`). Whether iOS reports that
  value, or the device-tree serial the firmware patches set, is untested.

### Making a phone by hand

The fleet manager will do this. Until it exists, make a phone by hand. The golden must already be
captured:

```sh
# Once, after the base VM is shut down and before any sign-in:
mkdir -p vms/golden && mv vms/myphone vms/golden/ios26-base
(cd vms/golden/ios26-base && sha256sum Disk.img nvram.bin SEPStorage > MANIFEST &&
 chmod 0444 Disk.img nvram.bin SEPStorage MANIFEST)

# Each phone:
G=vms/golden/ios26-base P=vms/phones/p01
mkdir -p "$P"
cp --reflink=always "$G"/{Disk.img,nvram.bin,SEPStorage} "$P"/
chmod u+w "$P"/{Disk.img,nvram.bin,SEPStorage}      # a reflink copy keeps the golden's 0444
for f in config.plist ecid udid-prediction.txt gfx-res OpenAVB.bin OpenSEP.bin; do
  [ -f "$G/$f" ] && cp "$G/$f" "$P/"
done
python3 -c 'import secrets; m = bytearray(secrets.token_bytes(6)); m[0] = m[0] & 0xfc | 2
print(":".join(f"{b:02x}" for b in m))' > "$P/mac"          # unicast, locally administered
python3 -c 'import secrets, string
print("".join(secrets.choice(string.ascii_uppercase + string.digits) for _ in range(12)))' > "$P/serial"
printf 'id = "p01"\ngolden = "ios26-base"\ngolden_sha256 = "%s"\ncid = 10\n' \
  "$(sha256sum < "$G/MANIFEST" | cut -c1-64)" > "$P/phone.toml"
tools/phone-isolation-check.sh "$G" "$P" vms/phones/p02    # with a second phone made the same way
```

`cp --reflink=always` fails, rather than making a full copy, when the filesystem cannot share
blocks.

## Profiles

`config/profiles.toml` holds the launch profiles. Each key is the `run-vphone.sh` variable of that
name. A variable already set in the caller's environment wins (`GFX=0 tools/phone-up.sh p01 lite`).

| Profile | SMP | MEM | TB_SIZE | Placement | For |
|---|---|---|---|---|---|
| `full` | 8 | 8G | default (2048 x86-64, 4096 aarch64) | fast cores (`run-vphone.sh` default) | one phone used like a real one |
| `lite` | 4 | 4G | 1024 | any core (`VPHONE_CPUS=""`) | many phones, one light app each |

The `lite` values are inferred and untested on Spark. Whether iOS runs well at 4 vCPUs and 4 GiB
is the first thing to measure. A phone's screen size comes from its own `gfx-res`, copied from the
golden.

## Tools

| Tool | Usage | What it does |
|---|---|---|
| `phone-up.sh` | `tools/phone-up.sh ID\|DIR [PROFILE] [qemu args]` | Boots `$FLEET/phones/ID` (`FLEET` default `vms`) in the foreground with its profile. QMP goes to `runs/ID/qmp.sock`, the census to `runs/ID/census`, the CID from `phone.toml`, and the QEMU name is `phone-ID`. Exits 2 if that phone's `Disk.img`, `nvram.bin` or `SEPStorage` is already open in any process, or another `phone-up.sh` holds it. Exits 1 for a golden, a folder without `phone.toml` or with read-only state files, a missing `serial` or `mac`, a serial, MAC or CID that another phone also has, or an `ecid` that disagrees with `udid-prediction.txt`. With `API_LISTEN=HOST:PORT` it also starts the phone's API proxy (below), which exits with QEMU. |
| `vphone-api-proxy` | `API_LISTEN=127.0.0.1:8765 tools/phone-up.sh ID`, or `vphone-api-proxy --listen HOST:PORT --cid CID [--vsock-dir DIR] [--verbose]` | The Linux side of vphone-cli's `--api-listen`: a token-checked HTTP/WebSocket gateway to one phone's vphoned (vsock port 1339; API in vphone-cli `Research/vphoned_http_api.md`). Build in `fleet/`: `cargo spark` (static aarch64 musl, linked with rustup's `rust-lld`; what `phone-up.sh` runs on aarch64), `cargo build --release` on other hosts. The token is `VPHONE_API_TOKEN` or a random one printed at start; send it as `Authorization: Bearer`, a `token=` query item or the subprotocol `vphone-token.<token>`. Refusals say what failed: 401 (no or wrong token, bad request head), 502 (the socket or vsock connect that failed, and why), 501 for `/v1/host/screen` (Mac only: use `screen.screenshot`). `API_VERBOSE=1` logs one line per connection. Reaches the guest through `$VSOCK_DIR/$CID.sock` when it exists, else AF_VSOCK, per connection. Tested on x86 against a fake vphoned over both datapaths (`vsock_loopback`); untested on Spark and against a real guest. |
| `phones-volume.sh` | `sudo tools/phones-volume.sh IMAGE SIZE [MOUNTPOINT]` | For ext4 hosts, which cannot reflink. Makes a sparse file holding an XFS filesystem with reflink (first use only) and mounts it through a direct-I/O loop device at `MOUNTPOINT` (default `vms`, which must be empty). Needs root. Adds nothing to fstab or systemd: run it again after a reboot. Not run in testing (needs root and xfsprogs). |
| `phone-isolation-check.sh` | `tools/phone-isolation-check.sh GOLDEN A B` | Checks on disk, without booting, that A and B are isolated reflinks of GOLDEN. For each state file it writes a marker block into A, then requires that B's and the golden's sha256 are unchanged, that A shares exactly one block fewer (`filefrag -v`), and that B still shares that block. It then writes A's original block back. It fails on a hard link, on a full copy, on a golden whose state files are not 0444, and while any of the three phones is running. Hashing reads every file whole: about four minutes with 64 GiB disks. |

`tools/boot-test.sh` leaves phones alone: it never stops a QEMU named `phone-*`.

## How many phones per DGX Spark

These are estimates from x86-64 measurements in these docs, not measured on a Spark.

**CPU is the limit.**

- An idle phone with about 60 iOS services disabled uses about 1.6 cores. With iOS's default
  services it uses about 5. Source: 32 and 92-104 CPU-seconds per 20 s ([ADVANCED.md](ADVANCED.md),
  "Background services").
- The Spark has 10 Cortex-X925 and 10 Cortex-A725 cores. Counting an A725 as about 0.55 of an X925
  gives about 15.5 fast-core equivalents. Keeping 2 for the host and GPU threads leaves about 13.5.
- That makes about **8 phones** idling with one light app each, or **4-6** if they are actively
  used.

**RAM would allow about 20.**

- Each phone needs about 6 GB: 4 GB of guest RAM (the iOS 26 floor, inferred), 1 GB of code cache
  (`TB_SIZE`) and about 1 GB for QEMU, reims-vgpu and Vulkan.
- About 120 GB is usable.

**Disk.**

- A finished VM is about 16 GB. A reflink phone adds only the blocks it has written: about 2-5 GB
  once it has been used for a while (inferred).
- A cold boot to unlock takes about 49-89 s.

**Untested on Spark:**

- Fewer vCPUs per phone (`SMP` 2-4) reduce the cost of TLB flushes. Whether iOS boots well at
  2 vCPUs is unknown.
- Running phones headless (`GFX=0`) would let KSM merge their memory (see below). Whether
  SpringBoard and an app's UI work without the GPU is unknown.

## Host facts that change with the phone count

- **Guest RAM lives in `/dev/shm`** when `GFX=1`. Each phone's guest RAM is a shared file mapping
  there, `MEM` bytes ([ARCHITECTURE.md](ARCHITECTURE.md)). `/dev/shm` must fit the sum of `MEM`
  over every running phone. On most distributions it defaults to half of RAM. Raise it with
  `mount -o remount,size=…G /dev/shm`.
- **KSM saves nothing for `GFX=1` phones.** Kernel same-page merging only merges private anonymous
  memory that has been marked mergeable. A shared `/dev/shm` mapping is neither. Only headless
  (`GFX=0`) phones, whose RAM is ordinary anonymous memory, could benefit. A 25-40% saving is
  plausible but inferred, and `run-vphone.sh` does not mark that memory mergeable yet.
- **`TB_SIZE` is resident memory per phone.** The TCG code cache fills under use, so count it with
  `MEM` for each phone.
- **`vm.max_map_count`** must be at least 1048576 ([SETUP.md](SETUP.md)). The limit applies to each
  process, and each phone is its own QEMU, so it does not grow with the phone count.

## Checks

These ran on fake 64 GiB sparse images on btrfs, with nothing booted:

- Two reflink phones pass `phone-isolation-check.sh`, run again and with A and B swapped, and
  each phone's own files hash the same before and after the checks.
- The check fails on a hard link to the golden, on a full copy (`cp --reflink=never`), on a
  writable golden, and when A and B are the same folder.
- `QEMU=/bin/echo tools/phone-up.sh A lite` prints A's own CID, MAC, serial, census folder and
  state paths, and the `lite` profile's `-smp 4 -m 4G` and `tb-size=1024`.
- A second `phone-up.sh A` exits 2, whether or not the first one went through `phone-up.sh`. On
  the golden it exits 1, and with a MAC another phone also has it exits 1.
