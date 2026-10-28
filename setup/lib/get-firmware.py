#!/usr/bin/env python3
"""Resolve a pairing's download URLs from vphone-cli's fw catalog.

Usage: get-firmware.py <vphone-cli> <DEVICE> <OSVERSION> [CLOUDOS_LABEL]

Prints one line per image as:  <name>\t<url>
  name = the file to cache it as (the .ipsw basename, or a stable
         cloudOS-<build>.ipsw built from the catalog's own cloudOS name)

CLOUDOS_LABEL (e.g. 26.6-23G71) selects the release's OWn cloudOS instead
of the recommended one when the label names a different build. Exit 1 when
the pairing is not in the catalog."""
import json
import re
import subprocess
import sys


def catalog(vc, device):
    raw = subprocess.run([vc, "fw", "catalog", "--json", "--device", device],
                         capture_output=True, text=True, timeout=180, check=True).stdout
    import json as _j
    data = _j.loads(raw[raw.index("{"):])
    import sys as _s
    if data.get("device") and data["device"] != device and data.get("pairings"):
        print(f"warn: the fw catalog is hard-coded to {data['device']} "
              f"(--filter {device} not honoured); resolving from that list",
              file=_s.stderr)
    return data.get("pairings")


def main() -> int:
    vc, device, ver = sys.argv[1], sys.argv[2], sys.argv[3]
    cloudos_label = sys.argv[4] if len(sys.argv) > 4 else ""
    data = catalog(vc, device)   # the list of pairings for the device
    if not data:
        return 1
    pairings = data

    ios = cloud = None
    for it in pairings:
        name = (it.get("ios") or {}).get("name", "")
        m = re.match(r"^(?:iOS|iPadOS|watchOS|tvOS|HomePod Software) +(\S+)$", name)
        if m and m.group(1) == ver:
            ios = it["ios"]
            rec = it.get("recommendedCloudOS") or {}
            mat = it.get("matchingCloudOS") or {}
            label_build = cloudos_label.rsplit("-", 1)[-1]
            cloud = mat if label_build and label_build in mat.get("name", "") else rec

    if not ios or not cloud.get("url"):
        print(f"no catalog pairing for {device} {ver} "
              f"(cloudOS {cloudos_label or 'recommended'})", file=sys.stderr)
        return 1

    out = []
    for key, item in (("ios", ios), ("cloudos", cloud)):
        url = item["url"]
        base = url.rsplit("/", 1)[-1]
        if re.match(r"^(iPhone|iPad|Watch|AppleTV|Audio)", base):
            fname = base
        else:  # cloudOS URLs are opaque CDN hashes
            build = re.search(r"\(([0-9A-Za-z]+)\)", item["name"])
            plain = re.match(r"cloudOS ([0-9A-Za-zb.x]+)$", item["name"].strip())
            if build:
                fname = "cloudOS-%s.ipsw" % build.group(1)
            elif plain:
                fname = "cloudOS-%s.ipsw" % plain.group(1)
            else:
                fname = "cloudOS-unknown-%d.ipsw" % (abs(hash(url)) % 10**8)
        out.append(f"{fname}\t{url}")
    print("\n".join(out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
