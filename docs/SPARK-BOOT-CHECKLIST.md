# DGX Spark boot checklist

The order in which to bring vphone-spark up on a DGX Spark (or another aarch64 Linux host), from a
bare host to many phones with their own data. Each step proves one thing the next one relies on, so
run them in order and stop at the first failure. Setup details are in [DGX-SPARK.md](DGX-SPARK.md);
how to diagnose, fix and report is in [AGENT-BRINGUP.md](AGENT-BRINGUP.md).

**Status: untested on Spark.** None of these steps has run on a Spark yet. The expected outputs come
from the x86-64 host this was tested on.

Keep everything each step prints: copy the outputs into `runs/spark-checklist/` (ignored by git) and
fill in the [results table](AGENT-BRINGUP.md#reporting-back) as you go.

Every command runs from the repository root. The fleet binary:

```sh
F=fleet/target/aarch64-unknown-linux-musl/release/vphone-fleet
```

## 1. Record the host

```sh
mkdir -p runs/spark-checklist
uname -a; getconf PAGESIZE; nproc; free -g; df -h . /dev/shm
nvidia-smi --query-gpu=name,driver_version --format=csv; vulkaninfo --summary | grep -E 'deviceName|driverVersion'
cat /sys/kernel/mm/transparent_hugepage/enabled /sys/module/zswap/parameters/enabled; mokutil --sb-state
```

Expected: `aarch64`, `4096`, 20 CPUs, the NVIDIA GB10 as the Vulkan device, driver 580.105.08 or
newer.

## 2. Check the host

```sh
tools/check-host.sh
```

Expected: `ok aarch64`, `ok cores: 10 x Cortex-A725, 10 x Cortex-X925` (the order may differ),
`ok 4 KiB host pages`, `ok Vulkan GPU: NVIDIA GB10…`, then `All checks passed.` Every `!` line names
its fix ([DGX-SPARK.md](DGX-SPARK.md#host-setup)).

## 3. Build

```sh
./setup/setup.sh -y 2>&1 | tee runs/spark-checklist/build.log   # all phases; ./setup/setup.sh build re-runs the build
ls linux-create/.build/out/out/Products/Debug-linux-aarch64/vphone-cli
file qemu/build/qemu-system-aarch64
cd fleet && cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test && \
  rustup target add aarch64-unknown-linux-musl && cargo spark && cd ..
```

Expected: every command succeeds; `file` shows `ELF 64-bit LSB … ARM aarch64`; `cargo test` ends
with `test result: ok` for each crate. Write down the real vphone-cli output folder if it is not
`Debug-linux-aarch64` (only inferred so far).

## 4. Get a VM

Either create it on the Spark ([CREATE-ON-LINUX.md](CREATE-ON-LINUX.md); needs the APFS module and
usbip from [DGX-SPARK.md](DGX-SPARK.md#host-setup) item 6):

```sh
./setup/create-vm.sh --device "iPhone17,3" --ios 26.6.2 --scale 1
```

or copy a finished VM made on another Linux PC. Then put it where `tools/boot-test.sh` looks, on
the reflink filesystem:

```sh
mkdir -p vms/base && cp -a --reflink=auto ~/vphone-setup-work/devices/<name> vms/base/vm
```

A VM made with vphone-cli on a Mac goes through `tools/import-vm.sh` instead, which writes the
`ecid` file the launcher and `vphone-fleet` need ([CREATE-ON-MAC.md](CREATE-ON-MAC.md)):
`tools/import-vm.sh me@my-mac.local:.vphone/machines/myphone base-import`, then
`mkdir -p vms/base && mv vms/base-import vms/base/vm`.

## 5. First boot

```sh
VMDIR=vms/base WINDOW=0 tools/boot-test.sh spark-boot1 3
```

Expected: `vphoned up t=…`, then `unlocked t=…`. On the x86-64 test PC vphoned answered after 49 s
to 2 minutes and the phone unlocked after 89 s to 5 minutes ([README.md](../README.md#status)). A first boot after a restore spends about 20 more minutes migrating data:
let it finish. Logs: `runs/spark-boot1/{serial,stderr,qemu}.log`. Then stop it:

```sh
for p in $(pgrep -x qemu-system-aar); do     # QEMU processes only, never a shell
  grep -qF "$PWD/runs/spark-boot1/qmp.sock" "/proc/$p/cmdline" && kill "$p" && tail --pid="$p" -f /dev/null
done
```

Do not install apps or sign in on this VM: it becomes the golden.

## 6. Capture the golden

```sh
$F golden add vms/base/vm --name base
(cd vms/golden/base && sha256sum -c MANIFEST)
```

Expected: `vms/golden/base/` with its three state files mode 0444, and `OK` for each.

## 7. Two phones, isolated

```sh
$F create A B --from base
tools/phone-isolation-check.sh vms/golden/base vms/phones/A vms/phones/B   # on disk, both stopped
$F up A B && $F list
```

Expected: the check ends with `PASS: … are isolated reflinks of …`; `up` prints `A healthy t=…s`
for each and `2 of 2 phones healthy`; `list` shows different CIDs and API ports, and
`cat vms/phones/{A,B}/mac` two different MACs. Then the booted checks, with `vphone-fleet serve`
running:

```sh
export VPHONE_FLEET_TOKEN=$(openssl rand -hex 16)
$F serve & SERVE=$!
T=$VPHONE_FLEET_TOKEN U=http://127.0.0.1:8760
```


1. **A write in A stays in A.** Write a marker into A, read it back from A, and look for it in B:

   ```sh
   curl -sS -H "Authorization: Bearer $T" -X PUT --data-binary 'marker-A' \
     "$U/phones/A/v1/files/content?path=/var/mobile/Documents/marker.txt"
   curl -sS -H "Authorization: Bearer $T" "$U/phones/A/v1/files/content?path=/var/mobile/Documents/marker.txt"   # marker-A
   curl -sS -H "Authorization: Bearer $T" "$U/phones/B/v1/files/content?path=/var/mobile/Documents/marker.txt"   # not found
   ```

2. **It survives a restart.** `$F stop A && $F up A`, then read the marker from A again: still
   `marker-A`.
3. **The golden is untouched.** `(cd vms/golden/base && sha256sum -c MANIFEST)` still prints `OK`
   three times.

Expected on an x86-64 host as on the Spark; none of it has run against a real guest yet.

## 8. A clone with a new ECID

Whether a clone with its own ECID still boots decides whether clones can have their own UDID
([DENSITY.md](DENSITY.md#identity)). On its own phone, never on A or B:

```sh
sed -i 's/^rotate_ecid = false/rotate_ecid = true/' config/profiles.toml
$F create E --from base
sed -i 's/^rotate_ecid = true/rotate_ecid = false/' config/profiles.toml
diff vms/golden/base/ecid vms/phones/E/ecid                   # must differ
$F up E 2>&1 | tee runs/spark-checklist/ecid-boot.log
curl -sS -H "Authorization: Bearer $T" -X POST "$U/phones/E/v1/rpc" \
  -d '{"method":"screen.unlock","params":{},"id":1}'          # "locked":false
```

Record one of: unlocks (rotate can be turned on), stops in iBoot or the kernel (keep
`runs/E/console.log`), or boots but vphoned never answers. Then `$F stop E; $F delete E --confirm E`.

## 9. Parking

With `serve` running, the [parking checklist](DENSITY.md#spark-checklist): wake latency, zswap
ratio, clock drift after a 10-minute park, no watchdog kills, a correct screenshot after a wake.

## 10. Density

Stop every phone and `serve` (it would park the bench phones), make phones only for the bench, and
sweep:

```sh
$F down && kill "$SERVE"
$F create b1 b2 b3 b4 --from base
tools/density-bench.sh runs/spark-checklist/density-1.csv b1                 # one phone, 54 settings
MEMS=4G SMPS=4 TB_SIZES=1024 GFXS="0 1" tools/density-bench.sh runs/spark-checklist/density-4.csv b1 b2 b3 b4
```

Each row is one phone at one setting: unlock time, CPU-seconds per 20 s idle, PSS, KSM sharing.
The full sweep boots 54 times, so expect a few hours. Phones per Spark at a setting: the smaller
of (spare cores x 20 / `cpu_s_per_20s`) and (usable RAM / `pss_mib`), then confirm it by running
that many at once.
