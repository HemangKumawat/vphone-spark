#!/bin/bash
# switch-latency.sh RUN CID BUNDLE [URL]: (with URL, vphoned opens the URL in BUNDLE, skipping
# its running-apps listing) ask vphoned to bring BUNDLE forward and report the seconds
# until the screen first changes (QMP screenshots, about two a second, for up to 180 s), and
# when vphoned answered. Returns once both are known, so the next switch starts clean.
cd "$(dirname "$0")/.."
R=runs/$1; CID=$2; B=$3; URL=${4:-}; D=$R/sw-$B; rm -rf "$D"; mkdir -p "$D"
tools/qmp.py $R/qmp.sock shot $D/base.png
start=$(date +%s.%N)
CID=$CID timeout 200 tools/vphoned.sh POST /v1/rpc \
  "{\"method\":\"apps.launch\",\"params\":{\"bundle_id\":\"$B\"${URL:+,\"url\":\"$URL\"}},\"id\":1}" >$D/rpc.json 2>&1 &
rpc=$!
python3 -I - "$D" "$start" "$R/qmp.sock" <<'PY'
import sys, time, subprocess
from PIL import Image, ImageChops
d, start, sock = sys.argv[1], float(sys.argv[2]), sys.argv[3]
base = Image.open(f"{d}/base.png").convert("L").resize((129, 280))
i = 0
while time.time() - start < 180:
    f = f"{d}/{i:03}.png"
    subprocess.run(["tools/qmp.py", sock, "shot", f], check=False)
    t = time.time() - start
    im = Image.open(f).convert("L").resize((129, 280))
    if sum(ImageChops.difference(im, base).histogram()[20:]) > 2000:
        print(f"first change at {t:.1f}s")
        break
    i += 1
    time.sleep(0.3)
else:
    print("no change in 180 s")
PY
wait $rpc
echo "  rpc answered after $(python3 -c "import time;print(round(time.time()-$start,1))")s: $(head -c 80 $D/rpc.json)"
