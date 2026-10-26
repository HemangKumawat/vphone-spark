#!/usr/bin/env python3
"""Query data/compatibility.json. One command per invocation, result on stdout.

Usage: json_query.py <command> [args]

  devices                       -> "id<TAB>name" for every device
  choices <DEVICE>              -> selectable pairings for a device (excludes unsupported):
                                   "osVersion<TAB>build<TAB>cloudOS<TAB>preset<TAB>flags"
  find <DEVICE> <OSVERSION>     -> pairing fields, dot-joined keys a=b
  validate                      -> schema/consistency check, exits non-zero on problems
"""
import json
import sys
from pathlib import Path

DATA = Path(__file__).resolve().parent.parent / "data" / "compatibility.json"
FLAGS = {"linuxTested": "linux-tested", "macTested": "mac-tested",
         "requiresMacBuilder": "needs-mac-builder", "recommended": "recommended"}

def load():
    with DATA.open() as f:
        return json.load(f)


def main() -> int:
    cmd = sys.argv[1]
    data = load()
    pairings = data["pairings"]

    if cmd == "devices":
        for d in data["devices"]:
            if d.get("unsupported"):
                continue
            print(f'{d["id"]}\t{d.get("name", d["id"])}')
        return 0

    if cmd == "choices":
        dev = sys.argv[2]
        for p in pairings:
            if p["device"] != dev or p.get("unsupported"):
                continue
            if p.get("linuxTested"):
                tested = "TESTED-ON-LINUX"
            elif p.get("macTested"):
                tested = "mac-tested -- under test"
            else:
                tested = "untested"
            flags = [tested] + [v for k, v in FLAGS.items()
                                if p.get(k) and k != "macTested"]
            print("\t".join((p["osVersion"], p["build"], p["cloudOS"],
                             p["preset"], " ".join(flags))))
        return 0

    if cmd == "find":
        dev, ver = sys.argv[2], sys.argv[3]
        for p in pairings:
            if p["device"] == dev and p["osVersion"] == ver:
                if p.get("unsupported"):
                    sys.exit(3)
                print("\t".join((
                    p["ipsw"], p["cloudOS"], p["preset"], p["build"],
                    "1" if p.get("linuxTested") else "0",
                    "1" if p.get("macTested") else "0",
                    "1" if p.get("requiresMacBuilder") else "0",
                    p.get("note", ""))))
                return 0
        return 1

    if cmd == "devname":
        dev = sys.argv[2]
        for d in data["devices"]:
            if d["id"] == dev:
                print(d.get("name", d["id"])); return 0
        return 1

    if cmd == "validate":
        problems = []
        ids = {d["id"] for d in data["devices"]}
        for i, p in enumerate(pairings):
            where = f'pairings[{i}] ({p.get("device")} {p.get("osVersion")})'
            if p["device"] not in ids:
                problems.append(f"{where}: unknown device id")
            for key in ("osVersion", "build", "cloudOS", "preset"):
                if not p.get(key):
                    problems.append(f"{where}: missing {key}")
            if not p.get("unsupported"):
                if not p.get("ipsw"):
                    problems.append(f"{where}: missing ipsw")
                pkey = (p["device"], p["osVersion"], p["cloudOS"])
                seen = {j: (q["device"], q["osVersion"], q["cloudOS"])
                        for j, q in enumerate(pairings) if not q.get("unsupported")
                        and j < i}
                if seen and pkey in seen.values():
                    problems.append(f"{where}: duplicate device/osVersion/cloudOS")
        if problems:
            print("\n".join(problems), file=sys.stderr)
            return 1
        print(f"ok: {len(pairings)} pairings across "
              f"{len(ids)} devices")
        return 0

    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
