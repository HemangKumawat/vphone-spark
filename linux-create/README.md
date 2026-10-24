# linux-create

vphone-cli's firmware tooling (`fw prepare`, `fw patch`, `restore`, `cfw install`, `sign`, ...) built
for Linux, so the VM can be created with no Mac. The steps are in
[docs/CREATE-ON-LINUX.md](../docs/CREATE-ON-LINUX.md).

vphone-cli's own files are not copied or changed here. `sync-sources.sh` links them from a vphone-cli
checkout (`VPHONE_CLI`, default `../vphone-cli`; tested with tag `2.12.1`) into `Sources/`, minus the
files listed in `excludes/`, plus:

- `Shims/`: Linux stand-ins for Apple modules (Darwin, CryptoKit, Compression, MachO,
  AppleArchive, CommonCrypto, ...);
- `overlay/`: whole-file replacements and Linux-only files (the Linux `cfw install` path, the restore
  bridge, URLSession workarounds, `fw snapshot-flip`);
- `edits/`: one-line sed edits, one `path<TAB>expression` per line.
- `Vendor/Img4tool/`: libimg4-spm 0.1.1 (the version vphone-cli pins) adapted for Linux; see its README.

```sh
VPHONE_CLI=../vphone-cli ./sync-sources.sh
swift build --build-system swiftbuild --scratch-path .build/out
# -> .build/out/out/Products/Debug-linux-x86_64/vphone-cli
```

Needs Swift 6.4.1. Re-run `sync-sources.sh` after changing anything in `overlay/`, `edits/` or
`excludes/`, or after moving the vphone-cli checkout to another tag.
