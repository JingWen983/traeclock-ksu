#!/system/bin/sh
# TraeClock KernelSU module — boot service.
# Starts the daemon (no wake locks; the daemon itself decides when rounds run).

MODDIR=${0%/*}
BIN="$MODDIR/bin/traeclockd"
DATA_DIR="/data/adb/traeclock"
PID_FILE="$DATA_DIR/daemon.pid"

mkdir -p "$DATA_DIR"
chmod 700 "$DATA_DIR" 2>/dev/null

# do not double-start
if [ -f "$PID_FILE" ]; then
    OLD_PID=$(cat "$PID_FILE" 2>/dev/null)
    if [ -n "$OLD_PID" ] && [ -d "/proc/$OLD_PID" ]; then
        exit 0
    fi
fi

if [ -x "$BIN" ]; then
    # ABI check: only run if the binary matches this device
    if "$BIN" --version >/dev/null 2>&1 || true; then
        nohup "$BIN" > "$DATA_DIR/logs/daemon.out" 2>&1 &
        echo $! > "$PID_FILE"
    fi
fi
