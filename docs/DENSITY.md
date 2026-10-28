# Many phones on one host

Many phones share one read-only base image (the golden). Each phone keeps its own data: app data,
caches, sign-ins and accounts survive its restarts, and no phone can see or change another's. This
page covers the on-disk layout, each phone's identity, the launch profiles, the tools, the fleet
manager `vphone-fleet`, parking idle phones, and an estimate of how many phones fit on a DGX Spark.

**Status: untested on Spark.** The layout, the tools and `vphone-fleet` were checked on fake sparse
images on btrfs with a fake QEMU, without booting (see [Checks](#checks)). The phone counts are
inferred from x86-64 measurements, not measured on a Spark.

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
    ├── phone.toml          id, golden, golden_sha256, cid, api_port, profile
    └── snapshots/<name>/   optional, taken while the phone is off: reflink copies of the three state
                            files, PatchReceipt.plist and restore-info.json when present, MANIFEST,
                            snapshot.toml (the phone's id, ECID, serial and MAC at the time)
runs/<id>/                  run time: qmp.sock, census/; from vphone-fleet also pid, console.log,
                            api-token (0600), scope (its systemd scope), activity (touched per API call)
runs/fleet-token            the control API's token (0600); runs/fleet.lock: one vphone-fleet serve
```

`phone.toml`:

```toml
id = "p01"
golden = "ios26-base"            # the folder under vms/golden/
golden_sha256 = "…"              # sha256 of that golden's MANIFEST
cid = 10                         # vsock CID: unique per phone, 3 or more
api_port = 8765                  # host port of the phone's API (vphone-api-proxy, 127.0.0.1)
profile = "lite"                 # optional: the default profile for tools/phone-up.sh
```

The API port is the phone's only host port: user networking (`-nic user`) opens none.

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

`config/profiles.toml` `[identity]` sets the policy. With `rotate_ecid = true`, `vphone-fleet
create` gives each clone a new ECID: `ecid`, `udid-prediction.txt` and `config.plist`'s
`machineIdentifier`, as vphone-cli does for a clone on a Mac. Clones keep the golden's ECID until a clone
with a new ECID is shown to boot, because the restore personalised the golden's firmware for that
ECID. Testing that is on the Spark checklist. Two caveats:

- A MAC matters only once the guest uses the network.
- The serial reaches the machine's config region (`hw/vmapple/cfg.c`). Whether iOS reports that
  value, or the device-tree serial the firmware patches set, is untested.

### Making a phone by hand

`vphone-fleet golden add` and `vphone-fleet create` do this ([below](#vphone-fleet)). Without Rust,
the same steps by hand:

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

| `vphone-fleet` | `vphone-fleet COMMAND` ([below](#vphone-fleet)) | Makes, runs, parks and snapshots the phones of one host: golden add, create, start/stop/up/down, cold snapshot/restore, reset, delete, and `serve`: the control API plus a 1 s loop for health, auto-restart, parking and CPU priority. Built in `fleet/` with `vphone-api-proxy`. |

`tools/boot-test.sh` leaves phones alone: it never stops a QEMU named `phone-*`.

## vphone-fleet

One static binary (`fleet/vphone-fleet`, built with `vphone-api-proxy`: `cargo spark` in `fleet/` on
aarch64, `cargo build --release` elsewhere) that owns each phone's lifecycle. It changes a phone's
state files only through a command that names the phone, and only while it is stopped: it takes the
same locks `phone-up.sh` holds while QEMU runs. Phones run detached, each in its own process group
and systemd scope. A `vphone-fleet` that restarts adopts every running phone from `runs/<id>/pid`, so
the phones never depend on it staying up.

```sh
vphone-fleet golden add vms/myphone --name ios26-base   # stopped VM -> vms/golden/ios26-base, 0444, MANIFEST
vphone-fleet create p01 p02 p03 --from ios26-base        # reflinks; own serial, MAC, CID, API port
vphone-fleet up                                          # start all, wait for each /v1/health
vphone-fleet list                                        # one JSON line per phone
vphone-fleet snapshot p01 clean                          # cold: p01 must be stopped
vphone-fleet restore p01 clean
vphone-fleet reset p01 --confirm p01                     # fresh state from its golden, keeps identity
vphone-fleet delete p01 --confirm p01
vphone-fleet serve                                       # control API on 127.0.0.1:8760, and the loop
```

`create` refuses a filesystem that cannot reflink: it never makes a full copy. `restore`
checks the snapshot's MANIFEST and refuses a snapshot taken of another phone or under another
ECID: the SEP's anti-replay state pairs with the disk. `reset` refuses when the golden's MANIFEST
no longer matches `golden_sha256`. Restoring a cold snapshot costs one boot (about 49-89 s); a
running state cannot be saved ([Rules](#layout)).

The control API takes the same token check as a phone's API (`Authorization: Bearer`, from
`VPHONE_FLEET_TOKEN` or a new one in `runs/fleet-token`). It answers JSON:

| Request | Does |
|---|---|
| `GET /phones`, `GET /phones/ID` | each phone's state (`active`, `idle`, `parked`, `stopped`), pid, health, seconds since last use, CID, API port; one phone also with its API token |
| `POST /phones?ids=A,B&from=GOLDEN[&profile=P]` | create |
| `POST /phones/start?ids=A,B`, `POST /phones/stop` | start or stop (default: all) |
| `POST /phones/ID/start`, `stop`, `park`, `wake` | one phone; `wake` answers after `/v1/health` |
| `POST /phones/ID/snapshots?name=N`, `restore?name=N`, `reset?confirm=ID` | cold snapshot, restore, reset |
| `DELETE /phones/ID?confirm=ID` | delete |
| `POST /goldens?dir=DIR[&name=NAME]` | golden add |
| `GET /features`, `PUT /features/NAME {"on": false}` | [feature switches](#feature-switches) |
| `ANY /phones/ID/v1/...` | that phone's vphoned API, token stripped, woken first if parked ([AUTOMATION.md](AUTOMATION.md)) |
| `POST /batch`, `GET /events` | one call on many phones; one WebSocket of every phone's and the fleet's events |
| `GET /phones/ID/screen`, `POST /phones/ID/input` | a PNG through QMP; tap, swipe, keys and text through QMP |
| `GET /diag`, `GET /phones/ID/diag`, `GET /phones/ID/logs?tail=N` | every check with a hint per failure (`vphone-fleet doctor`), and log tails |

Examples in curl and Python, the MCP server (`vphone-fleet mcp`) and the diagnosis flow:
[AUTOMATION.md](AUTOMATION.md). Contract: [openapi.yaml](openapi.yaml).

### Mac vphone-cli templates vs vphone-fleet

vphone-cli on a Mac makes machines from templates and keeps snapshots per machine
(`VPhoneMachineTemplates.swift`, `VPhoneMachineSnapshots.swift` in vphone-cli). `vphone-fleet` does
the same on Linux, then adds what many phones on one host need.

| Feature | Mac vphone-cli | Here | Evidence |
|---|---|---|---|
| Clone storage | `clonefile(2)` of disk, SEP storage and NVRAM; refused off APFS | `FICLONE` reflinks of the same three; refused on a filesystem that cannot reflink (`tools/phones-volume.sh` makes one on ext4) | `phone-isolation-check.sh` passes on two clones ([Checks](#checks)) |
| Clone identity | new ECID, MAC and UDID on first start | new serial, MAC, vsock CID and API port always; new ECID and UDID with `rotate_ecid` (off until a re-ECID'd clone is shown to boot) | identity checks in [Checks](#checks) |
| Clone provenance | `TemplateSource.plist` (template id, build) | `phone.toml` `golden` and `golden_sha256` (of the golden's MANIFEST); `reset` refuses a changed golden | `phone/snapshot.rs` `reset` |
| Base integrity | frozen template | MANIFEST (sha256 of the state files), mode 0444; `phone-up.sh` refuses to boot a golden | `golden add` check |
| Snapshot contents | disk, SEP storage, NVRAM as one unit, plus `PatchReceipt.plist` and `restore-info.json`; not `config.plist` or `udid-prediction.txt` | the same set; identity is not carried | `SNAPSHOT_EXTRAS` in `phone.rs` |
| Revert removes files written after the snapshot | yes (`absentFiles`) | yes | acceptance check |
| Only while stopped | `requireStopped` | the three files' locks as `phone-up.sh` holds them, and no process has them open | `require_stopped` |
| Snapshot integrity | file lists checked | sha256 MANIFEST verified before a restore | acceptance check |
| Restore into another machine | snapshots live in their machine | refused: the snapshot's id and ECID must be the phone's | `restore B s1` refused |
| Confirm before destroying | — | `reset` and `delete` need the id repeated | acceptance check |
| Template trimming | offline trim of the System volume (`VPhoneMachineTemplateTrimmer.swift`) | not done: it needs macOS APFS tools | — |
| Clock after a pause | time sent after connect, every minute, and on Mac wake | time sent after each wake from parking (`clock_resync`) | acceptance check |
| Many phones at once | one machine per window or command | one `serve`: start/stop many, health checks, auto-restart, parking, CPU weight and pinning, one control API | [Checks](#checks) |
| Idle cost | a running machine keeps its CPU and RAM | a parked phone uses no CPU; its memory can go to zswap | [Parking and memory](#parking-and-memory) |
| API per phone | `--api-listen` | `vphone-api-proxy` on the phone's `api_port`; a call wakes a parked phone | acceptance check |

## Parking and memory

CPU limits a host to about 8 running phones ([below](#how-many-phones-per-dgx-spark)). Most phones
are idle most of the time, so `vphone-fleet serve` pauses the idle ones and wakes them on use.

### Why not microVMs, containers or ballooning

| Approach | Why it does not fit |
|---|---|
| Firecracker, cloud-hypervisor, libkrun, Kata | They need KVM. KVM runs the guest on the host CPU, and iOS needs Apple CPU features (GXF, SPRR, Apple system registers) a non-Apple CPU does not have. Only QEMU's TCG emulates them today; trap-and-emulate under KVM is an [open question](AGENT-BRINGUP.md#open-question-hardware-virtualization). |
| gVisor | A user-space Linux kernel for Linux processes. iOS is not a Linux process. |
| LXC and other containers | They share the host's Linux kernel. iOS brings its own kernel (XNU). |
| Memory balloon, virtio-mem, free page reporting | They need a driver in the guest. iOS has none. |
| CRIU checkpoints | Inferred: it cannot checkpoint a QEMU holding GPU (Vulkan) memory and vsock connections, and QEMU's own migration is blocked by the GPU device. |

### What transfers

Park-when-idle does. Hosted agent platforms pause idle sandboxes and resume them on the next call
(DigitalOcean's managed agents auto-pause; Google's agent substrate snapshots and parks). Here that is
QMP `stop`/`cont` plus `memory.reclaim` and zswap:

| State | When | What the phone gets |
|---|---|---|
| ACTIVE | used within `idle_secs` (60) | `cpu.weight` 1000, the fast cores |
| IDLE | no use for `idle_secs` | `cpu.weight` 100, every core |
| PARKED | no use for `park_secs` (300), or the least recently used past `max_awake` (8) | QMP `stop`: no CPU at all; with `memory_reclaim`, `reclaim_on_park` (3G) written to its `memory.reclaim` |

A use is a connection to the phone's API port (`vphone-api-proxy --activity` touches
`runs/<id>/activity`), or a start or wake through the control API. Health checks go straight to the
vsock socket and do not count. A wake is QMP `cont`, then a wait for `/v1/health`, then
`PUT /v1/clock` with the host's time (`clock_resync`): the guest clock stops while the VM is
paused. A call to a parked phone's port wakes it within the loop's 1 s tick.

What it does not do, on purpose:

- **No `cpu.max`.** Throttling starves some guest threads while others run, and iOS's FrontBoard
  watchdog kills apps that miss its deadlines (about 10 s). A paused VM stops all of its time at
  once, so nothing inside sees a missed deadline.
- **No cgroup freezer.** It freezes QEMU's threads wherever they are; QMP `stop` is QEMU's own
  clean pause.
- **No DAMON, no KSM changes.**

The weights and the reclaim need each phone in its own cgroup: `phone-up.sh` starts QEMU in a
transient systemd scope `vphone-<id>-<pid>.scope` in `vphones.slice` (`systemd-run --scope`,
`--user` when not root; nothing is written to systemd's unit folders). Pinning uses CPU affinity on
every thread, because a user's systemd gets no `cpuset` controller.

### Host setup for memory reclaim

`memory_reclaim` is off by default. It only helps with zswap and a swap device; without them the
kernel has nowhere to put a parked phone's pages. Once, as root (DGX OS / Ubuntu):

```sh
# zswap: a compressed cache in RAM in front of swap
echo zstd | sudo tee /sys/module/zswap/parameters/compressor
echo Y    | sudo tee /sys/module/zswap/parameters/shrinker_enabled
echo 1    | sudo tee /sys/module/zswap/parameters/enabled
# a 64 GiB swap file on the NVMe (on btrfs: sudo btrfs filesystem mkswapfile --size 64g /swapfile)
sudo fallocate -l 64G /swapfile && sudo chmod 600 /swapfile && sudo mkswap /swapfile && sudo swapon /swapfile
# keep both after a reboot
echo '/swapfile none swap sw 0 0' | sudo tee -a /etc/fstab
#   and add to GRUB_CMDLINE_LINUX in /etc/default/grub, then sudo update-grub:
#   zswap.enabled=1 zswap.compressor=zstd zswap.shrinker_enabled=1
tools/check-host.sh        # should say "zswap on, swap present"
```

Then set `memory_reclaim = true` in `config/profiles.toml` `[features]`. With `GFX=1` the guest's
RAM is a file in `/dev/shm`; the kernel can swap those pages too (inferred, untested on Spark).

### How many phones parking buys

Inferred, unsure, untested on Spark:

| | Phones | Limit |
|---|---|---|
| All running | about 8 | CPU (about 13.5 fast-core equivalents) |
| 8 running, the rest parked | about 20 | RAM: about 120 GB at about 6 GB per phone; a parked phone needs no CPU |
| The same with zswap at about 2.5x | about 25-30 | 3 GB reclaimed per parked phone leaves about 4.2 GB (25 phones); 4.5 GB reclaimed leaves about 3.3 GB (30) |

### Spark checklist

1. Wake-to-health latency with 20 phones: park each, then time `POST /phones/<id>/wake` and a first
   call through its API port.
2. Compression: `zswapped` (original bytes) against `zswap` (compressed bytes) in each scope's
   `memory.stat`
   (`/sys/fs/cgroup/user.slice/user-$(id -u).slice/user@$(id -u).service/vphones.slice/vphone-*.scope/memory.stat`).
3. Guest date drift after a 10-minute park, with `clock_resync` on and off.
4. vphoned `logs.crashes` after waking: no FrontBoard watchdog kills.
5. A screenshot after waking (`screen.screenshot`) shows the app, not a black or stale screen.

### Next levers, not built yet

Ranked by expected gain (all inferred):

| Lever | Rule | Expected gain | How to measure |
|---|---|---|---|
| NVMe writeback for long-parked phones | `memory.zswap.writeback` 1 only for phones parked over an hour, 0 for the rest | about 40+ phones; their wake is slower (pages come back from NVMe) | wake latency, and `zswpwb` in `memory.stat` |
| Guest RAM 3G in `lite` | only after measuring that a light app never needs more | about a quarter more phones by RAM | jetsam events in vphoned's logs at 3G against 4G |
| 1x display scale in `lite` | [PERFORMANCE.md](PERFORMANCE.md), display scale | less GPU and copy work per frame | CPU per idle phone at 1x and 3x |
| KSM | only for `GFX=0` phones ([below](#host-facts-that-change-with-the-phone-count)) | 25-40% of their RAM | `/sys/kernel/mm/ksm/pages_sharing` |

## Feature switches

Each `vphone-fleet` behaviour can be turned off alone. The value comes from, lowest first: the
default, `config/profiles.toml` `[features]`, the environment (`VPHONE_FEATURE_PARKING=0`), and
`PUT /features/<name>` on a running `serve` (live, not written to the file). `vphone-fleet features`
and `GET /features` print each value and where it came from. Turning a switch off undoes it: parked
phones are resumed, weights go back to 100, affinity back to every core. The token check is never a
switch.

| Switch | Default | What it does | Turn it off when |
|---|---|---|---|
| `parking` | on | parks idle phones, wakes them on use | the first call after a quiet spell is too slow, or an app loses its network sessions while paused |
| `memory_reclaim` | off | reclaims a parked phone's memory into zswap | wakes get slow, or the host swaps hard; it needs [host setup](#host-setup-for-memory-reclaim) first |
| `cpu_priority` | on | `cpu.weight` 1000 for the phone in use, 100 for the rest | the background phones' apps time out |
| `core_pinning` | on | the phone in use on the fast cores, the rest on every core | the phone in use is slower than unpinned |
| `clock_resync` | on | sets the guest clock after a wake | `PUT /v1/clock` errors in the log, or the guest time jumps wrongly |
| `auto_restart` | on | restarts a phone whose QEMU died or whose vphoned stopped answering | a phone restarts in a loop: turn it off to read its `runs/<id>/console.log` |
| `batch` | on | `POST /batch`: one vphoned call on many phones, `max_awake` at a time | a script floods the phones, or waking many parked phones at once is too heavy |
| `events` | on | `GET /events` and the watchers that hold one `/v1/events` connection per healthy phone | nobody listens and the per-phone connections cost too much |
| `qmp_input` | on | `POST /phones/ID/input`: tap, swipe, keys and text through QMP | input lands in the wrong place, or only vphoned should touch the phones |
| `mcp` | on | `vphone-fleet mcp`: the fleet as MCP tools for AI agents | no agent should drive the phones |

A switched-off route answers 503 `{"error":"feature <name> is off","enable":"PUT /features/<name>"}`.

The launcher's own switches are environment variables ([PERFORMANCE.md](PERFORMANCE.md)); a
profile can set them too:

| Switch | Default | What it does | Turn it off when |
|---|---|---|---|
| `VPHONE_SAFE_TCG` | `0` | `1` turns off the three TCG speed tricks below | first thing to try when a guest misbehaves on an aarch64 host |
| `VPHONE_XPAGE_LINKS` | `1` | checked cross-page direct-branch links | crashes or hangs that `VPHONE_SAFE_TCG=1` cures: find which of the three |
| `VPHONE_RET_PREFETCH` | `2` | return-address jump cache prefetch at calls | as above |
| `VPHONE_INLINE_LOOKUP` | `2` | jump cache lookup at `BR`/`BLR`/`RET` without the generic helper | as above |
| `TB_SIZE` | 2048 MiB x86-64, 4096 aarch64; 1024 in `lite` | the TCG code cache, resident memory per phone | lower to fit more phones; raise when the UI stalls every few minutes (code cache flushes) |
| `GFX` | `1` in both profiles | the GPU device and screen | `GFX=0` for a phone nobody looks at (untested: whether apps' UI works without it) |

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
  plausible but inferred. QEMU already marks that RAM mergeable (`-machine mem-merge`, on by
  default: `qemu/qemu-options.hx`, `memory_try_enable_merging` in `qemu/system/physmem.c`), so the
  host only has to turn KSM on (`echo 1 | sudo tee /sys/kernel/mm/ksm/run`).
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

`vphone-fleet` ran on x86-64 (btrfs, systemd 262) against fake 64 MiB sparse images and a fake
QEMU that answers QMP and plays vphoned on the vsock socket, with nothing booted:

- `golden add`, then `create A B C`: `phone-isolation-check.sh` passes on the golden, A and B. A and B
  differ in serial, MAC and CID, and A's `config.plist` carries A's MAC.
- `snapshot A s1`, a marker block written into A, `restore A s1`: A hashes as before, its
  `restore-info.json` is back and a `PatchReceipt.plist` written after s1 is gone; B and the golden
  are unchanged. `restore B s1` and `delete A` without `--confirm A` (or with `--confirm B`) are refused.
- `up` reports `3 of 3 phones healthy`; each QEMU runs in its own scope in `vphones.slice`.
- Under `serve`: a request without the token gets 401; a second `serve` is refused; `park` stops B
  (QMP `stop`) and writes its `memory.reclaim`; one request through B's API port wakes it (QMP
  `cont`, answer 200) and sets its clock; turning `parking` off resumes a parked phone; a phone whose
  QEMU is killed is restarted by the loop. Through stop, start, park, wake and restart, A's, B's and
  C's state files hash the same.
- `cargo test` covers the rest without QEMU: the parking state machine on an injected clock against
  a fake QMP socket and a folder standing in for the cgroup (ACTIVE to IDLE to PARKED and back,
  `max_awake` parking the least recently used, waking on a call to the phone's port, turning
  `parking` and `cpu_priority` off), a phone stopped or started by another process, the feature
  switches' override order, and CPU pinning of a real process.
