#!/bin/bash
# Resume the parallel download of the iPhone Duo 27.1 IPSW with correct part
# names, then verify and move the result to the Lexar (per the user's request
# to keep the big IPSW off /home).
set -uo pipefail
U="https://updates.cdn-apple.com/2026FallFCS/3f41a447-d8a6-48ae-83ae-056a64f57fd4/iPhone19,4_27.1_24A94425_Restore.ipsw"
SHA=58fbfda85615e90221136d55288f64c9f41386510f8cd77feb8c9d20cc0a782e
TOTAL=12724731538
PARTS=16
CHUNK=$(( (TOTAL + PARTS - 1) / PARTS ))
D=/var/tmp/opencode/duo2
STAGE=/var/tmp/opencode/duo-stage.ipsw
DST_DIR=/run/media/yahia/Lexar/vphone-linux/ipsws
NAME=iPhone19,4_27.1_24A94425_Restore.ipsw
mkdir -p "$D" "$(dirname "$STAGE")" "$DST_DIR"

dl_part() {
	local i=$1
	local start=$(( i * CHUNK ))
	local end=$(( start + CHUNK - 1 )); [ "$end" -ge "$TOTAL" ] && end=$(( TOTAL - 1 ))
	local f="$D/part-$i"
	local len=$(( end - start + 1 ))
	while :; do
		local have=0
		[ -f "$f" ] && have=$( stat -c %s "$f" )
		if [ "$have" -ge "$len" ]; then break; fi
		if [ "$have" -gt "$len" ]; then echo "part $i OVERSIZED ($have > $len)"; exit 1; fi
		curl -sfS --retry 8 --retry-all-errors --connect-timeout 20 \
			--speed-time 90 --speed-limit 20000 \
			-r $(( start + have ))-"$end" -o - "$U" >> "$f" || sleep 3
	done
	echo "part $i complete: $( stat -c %s "$f" ) bytes" >> "$D/done.log"
}

for i in $(seq 0 $(( PARTS - 1 ))); do
	dl_part "$i" &
done
wait
echo "all parts complete, assembling" >> "$D/done.log"
: > "$STAGE"
for i in $(seq 0 $(( PARTS - 1 ))); do
	# Numeric order, never the glob's lexical one.
	cat "$D/part-$i" >> "$STAGE"
done
echo "$SHA  $STAGE" | ( cd "$( dirname "$STAGE" )" && sha256sum -cw - ) || { echo SHA-MISMATCH; exit 1; }
echo "SHA-OK; moving to $DST_DIR"
mv "$STAGE" "$DST_DIR/$NAME"
sync
echo "$SHA  $DST_DIR/$NAME" | ( cd "$DST_DIR" && sha256sum -cw - ) || { echo POST-MOVE-SHA-MISMATCH; exit 1; }
rm -rf "$D"
echo "POST-MOVE SHA-OK: $DST_DIR/$NAME"
