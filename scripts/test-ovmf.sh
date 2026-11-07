#!/usr/bin/env bash
# The driver under OVMF in QEMU, on the build box (needs no root):
#
#   sc-build scripts/test-ovmf.sh
#
# Builds the check image (scripts/build-image.sh) with a UDP echo port for
# slirp, then boots it with QEMU's `igb` device (QEMU 8.0 and later: an
# Intel 82576, 8086:10c9), without an option ROM: OVMF has no igb driver, so
# ours is the only one. The check app (check/) loads and connects the
# driver and runs smoltcp over its SNP. slirp leases 10.0.2.15 and answers
# pings to its 10.0.2.2; the UDP echo goes to a python echo server on the
# build box's loopback, which slirp reaches as 10.0.2.2.
#
# Two boots: `quiet` (the default console: the driver's one line) and
# `verbose` (StormnicVerbose set by the check image, so the whole bring-up
# trace is printed: the MAC reset, PHY, DMA check). Each must print
# `STORMNIC-IGB CHECK PASS` and the driver's one console line.
#
# Needs qemu-system-x86_64 with the igb device, OVMF, python3, mkfs.fat,
# mtools. KVM when /dev/kvm is writable, TCG otherwise.
set -euo pipefail
cd "$(dirname "$0")/.."
OVMF_CODE=${OVMF_CODE:-/usr/share/edk2/ovmf/OVMF_CODE.fd}
OVMF_VARS=${OVMF_VARS:-/usr/share/edk2/ovmf/OVMF_VARS.fd}
LIMIT=${LIMIT:-180}
say() { printf 'test-ovmf: %s\n' "$*"; }
die() { say "FAIL: $*"; exit 1; }
command -v qemu-system-x86_64 >/dev/null || die "qemu-system-x86_64 is not installed"
[[ -r "$OVMF_CODE" ]] || die "no OVMF at $OVMF_CODE"
say "$(qemu-system-x86_64 --version | head -1)"
qemu-system-x86_64 -device help 2>/dev/null | grep -q '"igb"' || die "this QEMU has no igb device (needs QEMU 8.0 or later)"

W=$(mktemp -d "${TMPDIR:-/tmp}/test-ovmf.XXXXXX")
ECHO_PID=""
cleanup() { [[ -n "$ECHO_PID" ]] && kill "$ECHO_PID" 2>/dev/null; rm -rf "$W"; }
trap cleanup EXIT

# A UDP echo server; port 0 lets the kernel pick, written down for the image.
python3 - "$W/echo.port" <<'PY' &
import socket, sys
s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
s.bind(("127.0.0.1", 0))
open(sys.argv[1], "w").write(str(s.getsockname()[1]))
while True:
    data, peer = s.recvfrom(65536)
    s.sendto(data, peer)
PY
ECHO_PID=$!
for _ in $(seq 50); do [[ -s "$W/echo.port" ]] && break; sleep 0.1; done
PORT=$(cat "$W/echo.port") || die "the UDP echo server did not start"
say "UDP echo on 127.0.0.1:$PORT (10.0.2.2:$PORT from the guest)"

CHECK_CONF="udp_port=$PORT" scripts/build-image.sh "$W/quiet.img"
CHECK_CONF="$(printf 'udp_port=%s\nverbose=1' "$PORT")" scripts/build-image.sh "$W/verbose.img"

accel=tcg
[[ -w /dev/kvm ]] && accel=kvm
boot() {
    local name=$1 log="$W/$1.log"
    cp "$OVMF_VARS" "$W/vars.fd"
    say "boot $name (igb, $accel)"
    timeout "$LIMIT" qemu-system-x86_64 -machine q35,accel="$accel" -m 512 -no-reboot \
        -drive if=pflash,format=raw,readonly=on,file="$OVMF_CODE" \
        -drive if=pflash,format=raw,file="$W/vars.fd" \
        -drive file="$W/$name.img",format=raw,if=none,id=disk -device virtio-blk-pci,drive=disk,bootindex=1 \
        -netdev user,id=n0 -device igb,netdev=n0,romfile= \
        -serial file:"$log" -monitor none -display none </dev/null >/dev/null 2>&1 || true
    sed -e 's/\x1b\[[0-9;]*[A-Za-z]//g' "$log" | tr -d '\r' | grep -E '^(stormnic-igb|check:|STORMNIC-IGB)' || true
    grep -q 'STORMNIC-IGB CHECK PASS' "$log" || die "$name: no PASS line"
    grep -q "stormnic-igb [0-9.]*: .*8086:10c9 .*SNP installed" "$log" || die "$name: no driver line for 8086:10c9"
    say "$name: PASS"
}
boot quiet
if grep -q 'stormnic-igb: .*MAC reset' "$W/quiet.log"; then die "quiet: the bring-up trace was printed without StormnicVerbose"; fi
boot verbose
grep -q 'stormnic-igb: .*DMA check: broadcast frame sent' "$W/verbose.log" || die "verbose: no DMA check line in the trace"
say "all boots passed"
