#!/bin/bash
# SSH into the booted SSH ramdisk over usbmuxd (dropbear, root/alpine).
#   tools/ssh-ramdisk.sh 'id; mount'
# Optional env: USBMUX_DEVICE=<n> to pin the usbmux handle.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
ASKPASS=/tmp/vphone-askpass.sh
if [ ! -x "$ASKPASS" ]; then
    printf '#!/bin/sh\necho alpine\n' > "$ASKPASS"
    chmod 700 "$ASKPASS"
fi

export DISPLAY="${DISPLAY:-:0}"
export SSH_ASKPASS="$ASKPASS"
export SSH_ASKPASS_REQUIRE=force

exec setsid -w ssh \
    -o StrictHostKeyChecking=no \
    -o UserKnownHostsFile=/dev/null \
    -o ProxyCommand="/usr/bin/python3 $ROOT/tools/usbmux-connect.py proxy 22" \
    -o NumberOfPasswordPrompts=1 \
    -o ConnectTimeout=15 \
    -o PreferredAuthentications=password,keyboard-interactive \
    root@vphone "$@"
