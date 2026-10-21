# Contributing

Thanks for helping. vphone-linux spans four repositories, so first find where a change belongs:

| Change | Repository |
|---|---|
| Launcher, tools, docs | this repository |
| Machine model, SEP, USB, vsock, TCG performance | [qemu-vphone](https://github.com/YahiaAngelo/qemu-vphone) (`qemu/`, branch `vphone`) |
| GPU protocol, resources, IOSurfaces, window | [reims-vgpu fork](https://github.com/YahiaAngelo/reims-vgpu) (`reims-vgpu/`, branch `vphone`) |
| Metal → Vulkan translation | [metal2vulkan fork](https://github.com/YahiaAngelo/metal2vulkan) (`metal2vulkan/`, branch `vphone`) |
| VM creation, firmware patches, vphoned | upstream [vphone-cli](https://github.com/Lakr233/vphone-cli) |

Fixes that are not specific to iOS guests are better sent to the upstream projects (QEMU or ChefKissInc/QEMUAppleSilicon,
reims-vgpu, metal2vulkan). The forks then pick them up.

## Ground rules

- **Never commit Apple material**: no firmware, IPSWs, ROMs, kernelcaches, dyld shared caches, VM
  images, or files copied out of a guest. No device identifiers either (ECID, serial number, UDID).
  `.gitignore` covers `vms/`, `dsc/` and `runs*/`, but check your diff anyway.
- **Test on a copy of your VM.** Make reflink copies (`cp -a --reflink=always`) and never run two
  VMs on one `Disk.img`.
- **Performance claims need numbers.** Give the workload, the tool (`tools/perf-apps.sh`,
  `tools/tap-latency.py`, `tools/scroll-bench.sh`, ...), before and after, and the host.
- **New tuning switches** default to upstream behaviour in code. Turn them on in `run-vphone.sh`, and
  document them in `docs/PERFORMANCE.md`.
- **Diagnostics** stay off unless an environment variable turns them on, and must not write
  unbounded files to `/tmp`.
- Shell scripts pass `bash -n`; Python scripts run on the system `python3`.
- Contributions to this repository are accepted under its MIT license ([LICENSE](LICENSE)); changes to
  a fork fall under that fork's license ([docs/LICENSING.md](docs/LICENSING.md)).

## Bug reports

Include:

- the commits of the superproject and of each submodule (`git submodule status`);
- host distro, CPU, GPU and driver;
- iOS version and vphone-cli version;
- the launcher's environment;
- the tail of the serial log and of QEMU's stderr.

Remove your ECID, serial and UDID before posting logs.
