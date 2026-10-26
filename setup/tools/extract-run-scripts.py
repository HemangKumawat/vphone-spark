#!/usr/bin/env python3
"""Extract every shell `run:` block of a GitHub workflow to files.

Usage: extract-run-scripts.py <workflow.yml> <outdir>

Each block lands in <outdir>/<job>__<sanitized step name>.sh. Steps without
a string run block are skipped (uses:, run: |, etc.). Exits non-zero when
nothing usable was extracted."""
import os
import re
import sys

import yaml  # PyYAML

# GitHub's ${{ ... }} expressions are not bash; sanitize them so bash and
# shellcheck can parse the file. The block still proves the real script's
# structure; expressions are validated by GitHub, not here.
_GHA = re.compile(r"\$\{\{\s*[^}]+\s*\}\}")


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__)
        return 2
    wf, outdir = sys.argv[1], sys.argv[2]
    os.makedirs(outdir, exist_ok=True)
    with open(wf) as f:
        data = yaml.safe_load(f)
    n = 0
    for jname, job in (data.get("jobs") or {}).items():
        for step in (job or {}).get("steps", []):
            if not isinstance(step, dict):
                continue
            run = step.get("run")
            if not isinstance(run, str) or not run.strip():
                continue
            label = step.get("name") or "unnamed"
            slug = re.sub(r"[^A-Za-z0-9]+", "_", f"{jname}__{label}").strip("_")
            with open(os.path.join(outdir, f"{slug}.sh"), "w") as out:
                out.write("#!/bin/bash\n")
                out.write("# ${{ ... }} expressions are GitHub-resolved at run time; they\n")
                out.write("# are replaced here with a literal so bash/shellcheck can parse.\n")
                out.write(_GHA.sub("__GHA_EXPR__", run) + "\n")
            n += 1
    if not n:
        print(f"no run blocks extracted from {wf}", file=sys.stderr)
        return 1
    print(f"{n} run blocks -> {outdir}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
