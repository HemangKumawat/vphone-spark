#!/bin/bash
# vsock-boot-check.sh N CID0: N boots of the home-screen clone; for each, whether vphoned
# answers and the vsock rings' layout (tools/vsock-rings.py), then the VM is stopped.
cd "$(dirname "$0")/.."
N=$1; CID=$2
for i in $(seq 1 $N); do
  r=vsk$i
  WINDOW=0 tools/boot-test.sh $r $CID 2>&1 | head -1 &
  # vphoned's first answer, or 300 s
  for t in $(seq 1 60); do
    CID=$CID VPHONED_TIMEOUT=4 timeout 5 tools/vphoned.sh GET /v1/health 2>/dev/null | grep -q '"status":"ok"' && break
    sleep 5
  done
  ok=$(CID=$CID VPHONED_TIMEOUT=4 timeout 5 tools/vphoned.sh GET /v1/health 2>/dev/null | grep -c '"status":"ok"')
  echo "boot $i (CID $CID): vphoned ok=$ok"
  tools/vsock-rings.py runs/$r/qmp.sock | sed 's/^/  /'
  for p in $(pgrep -f -- "-qmp unix:$PWD/runs/$r/"); do kill $p 2>/dev/null; done
  for p in $(pgrep -f "tools/boot-test.sh $r "); do kill $p 2>/dev/null; done
  sleep 8
  CID=$((CID + 1))
done
