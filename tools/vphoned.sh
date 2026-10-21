#!/bin/bash
# Talk to vphoned (vphone-cli's guest daemon) over vsock from the Linux host.
#
#   vphoned.sh [-c CID] METHOD PATH [JSON-BODY]
#   vphoned.sh GET /v1/health
#   vphoned.sh GET /v1/device
#   vphoned.sh POST /v1/rpc '{"method":"device.snapshot","params":{}}'
#
# CID defaults to $CID or 3 (run-vphone.sh's default guest CID). The write side stays open
# (ignoreeof): vphoned drops a half-closed connection before it answers an RPC.
# A VM booted with VSOCK=uds has a Unix socket $VSOCK_DIR/$CID.sock instead of host AF_VSOCK
# (hybrid vsock: "CONNECT 1339" first; its "OK" line goes with the HTTP headers).
set -euo pipefail
cid=${CID:-3}
if [ "${1:-}" = -c ]; then cid=$2; shift 2; fi
method=${1:?usage: vphoned.sh [-c CID] METHOD PATH [JSON]}
path=${2:?usage: vphoned.sh [-c CID] METHOD PATH [JSON]}
body=${3:-}

uds=${VSOCK_DIR:-/tmp/vphone-vsock}/$cid.sock
# AF_VSOCK: 4 MiB send buffer (SO_VM_SOCKETS_BUFFER_MAX_SIZE, then _SIZE; u64). Linux caps the
# send credit at it, and iOS sends a credit update only once its 512 KiB window is nearly
# full: with the 256 KiB default, a request body over 256 KiB stalls for good.
buf=x0000400000000000
if [ -S "$uds" ]; then target=UNIX-CONNECT:$uds
else target=VSOCK-CONNECT:$cid:1339,setsockopt=40:2:$buf,setsockopt=40:0:$buf; fi
{
  [ -S "$uds" ] && printf 'CONNECT 1339\n'
  printf '%s %s HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n' "$method" "$path"
  if [ -n "$body" ]; then
    printf 'Content-Type: application/json\r\nContent-Length: %d\r\n' "${#body}"
  fi
  printf '\r\n%s' "$body"
} | socat -T "${VPHONED_TIMEOUT:-300}" STDIN,ignoreeof!!STDOUT "$target" | sed '1,/^\r$/d'
echo
