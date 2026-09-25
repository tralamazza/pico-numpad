#!/usr/bin/env bash
# Resilient RTT capture: re-attach whenever the debug link drops, so a gesture is
# not lost because probe-rs died mid-session (this link fails regularly with
# Dap(NoAcknowledge) ~16s in).
#
#   ./tools/capture.sh [logfile]        default: /tmp/rtt.log
#
# Stop with: pkill -f capture.sh
set -u

LOG="${1:-/tmp/rtt.log}"
cd "$(dirname "$0")/.." || exit 1

echo "=== capture started $(date '+%H:%M:%S') -> $LOG ===" >>"$LOG"
trap 'echo "=== capture stopped $(date "+%H:%M:%S") ===" >>'"$LOG"'; exit 0' INT TERM

while true; do
    if ! probe-rs list 2>/dev/null | grep -q "Debug Probe"; then
        sleep 2
        continue
    fi
    # Attach without reflashing; keep only app-level lines plus a compact HCI trace.
    probe-rs attach --chip RP235x \
        firmware/target/thumbv8m.main-none-eabihf/diagnostic/pico-numpad \
        2>&1 | tee -a "$LOG"
    echo "=== link dropped $(date '+%H:%M:%S'), re-attaching in 2s ===" >>"$LOG"
    sleep 2
done
