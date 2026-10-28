# Bringing vphone-spark up on a DGX Spark: a guide for an AI agent (or a person)

**In one paragraph.** This repository runs many virtual iPhones on one Linux aarch64 host, each with
its own persistent data, aimed at the NVIDIA DGX Spark. Everything was written and tested on an
x86-64 Linux host; **nothing has run on a Spark yet**. Your job, if you take it, is to prove it
there step by step, fix what breaks, and report what you measured. This page is context and
suggestions, not a rulebook: you decide how to work. The step-by-step commands are in
[SPARK-BOOT-CHECKLIST.md](SPARK-BOOT-CHECKLIST.md), the host setup in [DGX-SPARK.md](DGX-SPARK.md).

**Before anything else: no App Store.** These are Apple research VMs. App Store and Apple ID sign-in
fail, so WhatsApp, Instagram and other store apps cannot be installed; only self-built, sideloaded
apps run ([DGX-SPARK.md](DGX-SPARK.md#first-no-app-store-no-apple-id)). No host CPU changes that.

## What is proven and what is not

| Part | State | Evidence |
|---|---|---|
| iOS 26.6.2 / 27.0.1 / 27.1 boot to the home screen, touch, vphoned, sideloaded apps | proven on one x86-64 Linux PC (upstream) | [README.md](../README.md#upstream-vphone-linux) |
| aarch64 host glue: build paths, Vulkan ICD lookup, TB_SIZE 4096, core tiers, `VPHONE_SAFE_TCG` | written, untested on Spark | commit "Run the Linux glue on aarch64 hosts" |
| One golden, reflink phones, per-phone identity, `phone-up.sh` refusals | checked on fake images on btrfs, nothing booted | [DENSITY.md](DENSITY.md#checks) |
| `vphone-fleet` (lifecycle, parking, API, MCP) and `vphone-api-proxy` | `cargo test` against fake QEMU and fake vphoned on x86-64; untested against a real guest | [DENSITY.md](DENSITY.md#checks), [AUTOMATION.md](AUTOMATION.md) |
| Building on aarch64 | untested (CI builds and tests `fleet/` on an arm64 runner when the repository is public) | `.github/workflows/validate.yml` |
| Booting on GB10, Vulkan on GB10, multiple phones at once, a clone with a new ECID, parking on a real guest, the phones-per-Spark number | **untested** | the checklist exists to prove these |

## The bring-up ladder

Run [SPARK-BOOT-CHECKLIST.md](SPARK-BOOT-CHECKLIST.md) in order; each step relies on the last. What
usually breaks at each step on GB10/aarch64, and the first thing to try:

| Step | Most likely failure on a Spark | First fix to try | Keep |
|---|---|---|---|
| 1-2 host check | `host page size 65536`; Vulkan device `llvmpipe`; `/dev/vhost-vsock missing` | boot the 4 KiB-page kernel; driver 580.105.08+ and `libnvidia-gl-580`; `modprobe vhost_vsock` | `check-host.sh` output |
| 3 build | swiftly or Swift download for aarch64; the vphone-cli output folder is not `Debug-linux-aarch64`; libarchive shim path | read the exact error; fix the path in the script that assumed it (`grep -rn Debug-linux`) | `build.log`, `file` of each binary |
| 4 create the VM | APFS module: `Key was rejected by service` (Secure Boot); usbip attach fails | sign `apfs.ko` and enrol the MOK ([DGX-SPARK.md](DGX-SPARK.md#host-setup) item 6); or make the VM on another host and copy it | the failing step's log |
| 5 first boot | no `vphoned up` (guest hangs or crashes early) | `VPHONE_SAFE_TCG=1`; then `TB_SIZE=2048`; `GFX=0` to separate CPU from GPU; `VK_DRIVER_FILES=` the right ICD | `runs/<run>/{serial,stderr,qemu}.log` |
| 5 first boot | black screen or GPU fault | the ICD path; `GFX_ISOLATION=off` as a diagnostic (points at the page size) | `stderr.log`, `runs/census/` |
| 6-7 golden, clones | `create` refuses: no reflink | btrfs, or `tools/phones-volume.sh` (XFS in a file) | the refusal text |
| 7 two phones | the second phone's vphoned never answers | duplicate CID (cannot happen through `create`); `VSOCK_DIR` mode (`check-host.sh`) | `vphone-fleet doctor --logs` |
| 8 new ECID | the clone stops in iBoot or never unlocks | expected possible outcome: record it, keep `rotate_ecid = false` | `runs/E/console.log` |
| 9 parking | slow wakes, clock jumps, apps killed after a wake | turn the switch off ([below](#if-something-is-unstable)) and measure again | wake times, `memory.stat`, `logs.crashes` |
| 10 density | phones fail to unlock at small `MEM` or `SMP` | that is a result: record the floor | the CSV and its `runs/bench/` logs |

## Contributing fixes

Suggested flow, for anyone with write access:

- Work on a branch (for example `spark-bringup`), never on `main` directly, and open one pull request
  per logical fix.
- One fix per commit. Put the failing output and the passing output in the commit message, and say
  what the fix was tested on ("on a DGX Spark, DGX OS 7.x, driver 580.x").
- Never rewrite published history (no force-push to a shared branch).
- Never commit Apple firmware, IPSWs, ROMs, VM images, keys or secrets. CI's `no-apple-content` job
  rejects the obvious ones; the rest is on you.
- Before each push, run what CI runs:
  ```sh
  find . -name '*.sh' -not -path './qemu/*' -not -path './reims-vgpu/*' -not -path './metal2vulkan/*' -exec bash -n {} \;
  shellcheck -x tools/<the scripts you changed>.sh setup/*.sh
  (cd fleet && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test)
  ```
- Review your own diff before pushing, as a reviewer would: does it change behaviour on x86-64 hosts
  (it must not, unless that is the fix); does every new failure path say what failed; does it leave
  temp files or processes behind; are the docs that describe the changed behaviour updated in the
  same commit; is "untested on Spark" removed only where you tested it.
- Use it for real work only when the branch is green and the checklist passes on the Spark.

## What we found, with sources

- **No aarch64 port existed** (checked 2026-10-28): vphone-cli needs Virtualization.framework on an
  Apple silicon Mac; vphone-linux supports only x86-64 hosts; its forks and issues have no ARM work.
- **What pinned it to x86-64 was host glue, not code:** a hard-coded `Debug-linux-x86_64` path, a
  Fedora libarchive symlink, an arch-less swiftly URL, a Fedora Vulkan ICD name, the x86 TCG code
  cache cap (`qemu/tcg/i386/tcg-target.h:32`: 2 GiB; aarch64 has none, `tcg/aarch64/tcg-target.h:17`),
  and an i7-specific core layout. All changed in the aarch64 commit, untested on Spark. The Rust
  crates cross-build to static aarch64 binaries; whether QEMU (C) and vphone-cli (Swift) build on
  aarch64 is untested (checklist step 3).
- **KVM alone cannot run this guest unmodified:** the machine runs "under TCG on the apple-gxf CPU (GXF/SPRR for SPTM
  and TXM)" ([vresearch101.c:20](https://github.com/YahiaAngelo/qemu-vphone/blob/4da65a96e039413faff00ed52e3d956bf931bfd9/hw/vmapple/vresearch101.c#L20)).
  GENTER/GEXIT are decoded by the emulator
  ([translate-a64.c:10632](https://github.com/YahiaAngelo/qemu-vphone/blob/4da65a96e039413faff00ed52e3d956bf931bfd9/target/arm/tcg/translate-a64.c#L10632)),
  SPRR and Apple's system registers are emulated in `hw/arm/apple-silicon/a13_gxf.c` and `a13.c`.
  A Cortex-X925 has neither, and KVM does not provide them. Whether a VMM could trap and emulate
  them is the [open question](#open-question-hardware-virtualization).
- **Speed under TCG:** expect about the x86-64 figures (cold app launch about 20 s, warm 4-8 s,
  scrolling about 60 frames/s); inferred from the same single-thread class.
- **Density arithmetic** (inferred): an idle phone costs about 1.6 cores with background services
  trimmed, about 5 with iOS's defaults ([ADVANCED.md](ADVANCED.md)); about 13.5 fast-core
  equivalents are usable, so about 8 idle phones or 4-6 active ones. RAM (about 6 GB each) would
  allow about 20 ([DENSITY.md](DENSITY.md#how-many-phones-per-dgx-spark)).
- **KSM and the GPU:** with `GFX=1` guest RAM is a shared `/dev/shm` file, which KSM cannot merge;
  only `GFX=0` phones could share pages ([DENSITY.md](DENSITY.md#host-facts-that-change-with-the-phone-count)).
- **Reflink copies, not qcow2:** each state file is opened twice (pflash and a PCI disk), which an
  overlay cannot serve; a reflink copy holds every write in the phone's own file
  ([DENSITY.md](DENSITY.md#why-a-reflink-copy-holds-all-of-a-phones-state)).
- **Identity:** serial, MAC, CID and API port are per phone; ECID and UDID are shared until a clone
  with a new ECID is shown to boot ([DENSITY.md](DENSITY.md#identity)).
- **Snapshots only while stopped:** the GPU device blocks migration and the Apple devices have no
  saved state, so a running phone cannot be saved ([DENSITY.md](DENSITY.md#layout)).
- **App Store:** vphone-cli [#393](https://github.com/Lakr233/vphone-cli/issues/393),
  [#326](https://github.com/Lakr233/vphone-cli/issues/326),
  [#282](https://github.com/Lakr233/vphone-cli/issues/282),
  [#212](https://github.com/Lakr233/vphone-cli/issues/212).
- **Spark host facts (community reports):** Vulkan on GB10 needs driver 580.105.08+ and
  `libnvidia-gl-580` ([gist](https://gist.github.com/solatticus/14313d9629c4896abfdf57aaf421a07a));
  the 64 KiB-page kernel option is for CUDA memory headroom
  ([NVIDIA forum](https://forums.developer.nvidia.com/t/introducing-dgx-spark-memory-saver-more-memory-headroom-for-cuda-workloads/384884)).

## How we suggest working on hardware you own

These habits got this far without a Spark; they are offered, not imposed.

- **Ground truth before claims.** Run the command, read the line. A claim nobody ran is a guess.
- **Two sources for anything load-bearing,** from different tools or layers, plus one observation
  that would be impossible if the claim were false. Reading the same file twice is one source.
- **Look up published specs** (driver versions, part numbers, page sizes) instead of inferring them.
- **Never report a derived number whose inputs you did not measure.** An empty cell beats a made-up
  one.
- **Consume-side proof.** A fix is done when the guest or the reader shows the changed state (the
  phone unlocks, the API answers), not when the writer says "ok".
- **One variable at a time,** with before and after output kept.
- **A stop rule:** after about three failures on the same error, research the exact error text;
  after about eight, stop and ask a human.
- **No silent failures.** Every caught error says what failed and why, where someone will see it.
- **Adopt before building.** Look for a maintained project that already does it.
- **The smallest change that works.** No speculative abstractions; delete before adding.
- **Timeouts from a reason,** not a number picked on the spot: the time the thing is known to take
  (a boot to unlock is about 5 minutes), with a margin you can explain.
- **Upstream reports carry artifacts:** logs, commands, commit ids. Never invent an id, a value or a
  result.

## Fitting more phones on one Spark

Light VMs and containers do not help here. Firecracker, cloud-hypervisor, Kata and friends need
KVM, which iOS cannot use on a non-Apple CPU; containers share the host kernel, and iOS brings its
own; memory ballooning needs a driver in the guest, which iOS does not have
([DENSITY.md](DENSITY.md#why-not-microvms-containers-or-ballooning)). What is already built is
parking: `vphone-fleet serve` pauses idle phones with QMP `stop`/`cont` (a parked phone uses no CPU),
can reclaim a parked phone's memory into zswap through `memory.reclaim`, and gives the phone in use
the fast cores and a higher CPU weight. The settings are `[fleet]` in `config/profiles.toml`.
Measure first: wake latency, the zswap compression ratio, and the guest clock after a park
([DENSITY.md](DENSITY.md#spark-checklist)). The ranked next levers (NVMe writeback for long-parked
phones, 3 GB guest RAM, 1x display, KSM for headless phones) are in
[DENSITY.md](DENSITY.md#next-levers-not-built-yet).

## If something is unstable

Every `vphone-fleet` behaviour and every launcher speed trick has a switch
([table](DENSITY.md#feature-switches)). Turn the suspect one off first, with the environment
(`VPHONE_FEATURE_PARKING=0`, `VPHONE_SAFE_TCG=1`) or live with `PUT /features/<name> {"on": false}`
on a running `serve`. Confirm the symptom goes away, then tune or fix that feature on the bring-up
branch.

## Open question: hardware virtualization

When there is time to experiment: could the Mac's approach make the phones faster here? On a Mac,
vphone-cli runs iOS with hardware virtualization (Virtualization.framework on Apple silicon). Here
iOS runs under QEMU's TCG emulation, because KVM on the Spark's Arm cores lacks the Apple CPU
extensions iOS uses (GXF, SPRR, Apple system registers; see [What we found](#what-we-found-with-sources)).
TCG is why CPU is the density limit. The question is whether hardware virtualization on the Spark is
a simple move, a big project or impossible, and if possible, what the path is. Leads, none proven:

1. KVM plus trapping and emulating the Apple-specific registers and instructions in the VMM.
   Commercial iOS-on-Arm-server virtualization is publicly reported to work this way; find primary
   sources.
2. Which iOS code paths actually touch GXF and SPRR, and whether the PCC research VM configuration
   needs them.
3. Whether the `vresearch101` machine can run with `-accel kvm` on aarch64 for the parts that do not
   need Apple extensions.
4. A cheap first experiment: boot with `-accel kvm` on the Spark (`run-vphone.sh` sets `-accel tcg`
   today) and record exactly where and why it fails: the first trapped instruction or register.

Time-box each experiment, change one thing at a time, keep failing and passing output, and never
risk the working TCG setup (a separate branch and a separate phone copy). Report a short note with
sources: simple, big project or blocked, plus the first concrete step. Build it only once the
evidence says the path is real.

## Reporting back

Fill one row per checklist step, in the pull request or the bring-up issue:

| Step | Result (pass / fail / skipped) | Key numbers | Evidence (log, CSV, commit) | Fix made (commit) |
|---|---|---|---|---|
| 1 host | | page size, driver, Vulkan device | | |
| 2 check-host | | | | |
| 3 build | | vphone-cli output folder | | |
| 4 VM | created here / copied | | | |
| 5 first boot | | `vphoned up t=`, `unlocked t=` | | |
| 6 golden | | | | |
| 7 isolation | | marker in A only; survives restart; golden `OK` | | |
| 8 new ECID | boots / iBoot / kernel / no vphoned | | | |
| 9 parking | | wake s, zswap ratio, clock drift | | |
| 10 density | | CPU-s per 20 s, PSS per phone, phones per Spark | | |
