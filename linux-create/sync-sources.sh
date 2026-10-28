#!/bin/bash
# Rebuild Sources/<Target>/ as per-file symlinks into the vphone-cli checkout, minus the files
# listed in excludes/<Target>.txt, plus the Linux replacements in overlay/<Target>/.
# Run after updating vphone-cli. vphone-cli's own files are never modified.
set -euo pipefail
cd "$(dirname "$0")"
V=${VPHONE_CLI:-../vphone-cli}
# ArchiveKit links Shims/libarchive/lib/libarchive.so: point it at this host's runtime library
# (no -dev package needed). gcc searches its own ABI's dirs: /usr/lib64 on Fedora, /usr/lib on
# Arch, /usr/lib/aarch64-linux-gnu on Ubuntu arm64.
lib=$(gcc -print-file-name=libarchive.so.13)
[ "${lib#/}" != "$lib" ] || { echo "libarchive.so.13 not found; install libarchive" >&2; exit 1; }
mkdir -p Shims/libarchive/lib && ln -sfn "$(realpath -s "$lib")" Shims/libarchive/lib/libarchive.so
declare -A SRC=(
  [VPhonePatchKit]=VPhoneExecutable/VPhoneCommand/VPhonePatchKit
  [FirmwarePatcher]=VPhoneExecutable/VPhoneCommand/FirmwarePatcher
  [VPhoneSign]=VPhoneExecutable/VPhoneCommand/VPhoneSign
  [VPhoneCoreKit]=VPhoneKit/VPhoneCoreKit
  [VPhoneArchiveKit]=VPhoneKit/VPhoneArchiveKit
  [VPhoneCommand]=VPhoneExecutable/VPhoneCommand/VPhoneCommand
  [VPhoneRestore]=VPhoneExecutable/VPhoneCommand/VPhoneRestore/VPhoneRestore
  [MobileRestoreCore]=VPhoneExecutable/VPhoneCommand/VPhoneRestore/MobileRestoreCore
  [MobileRecoveryCore]=VPhoneExecutable/VPhoneCommand/VPhoneRestore/MobileRecoveryCore
)
IMPORTS='^(@preconcurrency import |@_implementationOnly import |import )(Darwin|MachO|Compression|CoreGraphics|CoreServices|UniformTypeIdentifiers|CryptoKit|zlib)( *//.*)?$'
for t in "${!SRC[@]}"; do
  rm -rf "Sources/$t"; mkdir -p "Sources/$t"
  src=$(realpath "$V/${SRC[$t]}")
  (cd "$src" && find . -name '*.swift' -o -name '*.c' -o -name '*.h' -o -name '*.modulemap') | while read -r f; do
    f=${f#./}
    grep -qxF "$f" "excludes/$t.txt" 2>/dev/null && continue
    [ -e "overlay/$t/$f" ] && continue
    mkdir -p "Sources/$t/$(dirname "$f")"; ln -s "$src/$f" "Sources/$t/$f"
  done
  [ -d "overlay/$t" ] && cp -r "overlay/$t/." "Sources/$t/"
  # Apple module imports -> the VL* stand-ins. A file that needs it becomes a rewritten copy.
  { grep -RlE "$IMPORTS" "Sources/$t" --include='*.swift' || true; } | while read -r f; do
    sed -E "s#$IMPORTS#\\1VL\\2\\3#" "$f" > "$f.tmp" && rm -f "$f" && mv "$f.tmp" "$f"
  done
  # One-line Linux fixes: edits/<Target>.txt holds "relative/path.swift<TAB>sed expression" lines.
  if [ -f "edits/$t.txt" ]; then
    while IFS=$'\t' read -r f expr; do
      [ -z "$f" ] || [ "${f:0:1}" = "#" ] && continue
      g="Sources/$t/$f"
      sed -E "$expr" "$g" > "$g.tmp"
      cmp -s "$g" "$g.tmp" && { echo "edit did not apply: $t/$f: $expr" >&2; exit 1; }
      rm -f "$g" && mv "$g.tmp" "$g"
    done < "edits/$t.txt"
  fi
  # Module-qualified libc calls (Darwin.write) are Glibc.write on Linux.
  { grep -RlE '\bDarwin\.[a-z]' "Sources/$t" --include='*.swift' || true; } | while read -r f; do
    sed -E 's/\bDarwin\.([a-z])/Glibc.\1/g' "$f" > "$f.tmp" && rm -f "$f" && mv "$f.tmp" "$f"
  done
  echo "$t: $(find "Sources/$t" -name '*.swift' | wc -l) Swift files"
done
