#!/bin/sh
# Run a command inside a private PipeWire session with test devices, for
# the integration test (CI runs `scripts/pipewire-test-session.sh cargo
# test -- --ignored`). Meant for CI and containers: on a desktop the
# private WirePlumber would also try to open your sound cards.
#
# The session has two null sinks (test_speakers, test_headphones), a
# virtual microphone (test_mic) and a player stream (spotify) playing
# silence, like the fixture in tests/fixtures/pw-dump.json.
set -eu

for tool in dbus-daemon pipewire wireplumber pw-cli pw-cat pw-dump; do
  command -v "$tool" >/dev/null || { echo "$tool is needed" >&2; exit 2; }
done

RUN=$(mktemp -d)
chmod 700 "$RUN"
export XDG_RUNTIME_DIR="$RUN" XDG_STATE_HOME="$RUN/state" XDG_CONFIG_HOME="$RUN/config"
export DBUS_SESSION_BUS_ADDRESS="unix:path=$RUN/bus"
pids=""
cleanup() {
  for p in $pids; do kill "$p" 2>/dev/null || true; done
  # Equalizers the test started, if any are left.
  pkill -f "pipewire -c $RUN/" 2>/dev/null || true
  rm -rf "$RUN"
}
trap cleanup EXIT INT TERM

dbus-daemon --session --fork --address="$DBUS_SESSION_BUS_ADDRESS" --print-pid=1 > "$RUN/dbus.pid"
pids="$(cat "$RUN/dbus.pid")"
pipewire > "$RUN/pipewire.log" 2>&1 &
pids="$pids $!"
for _ in $(seq 50); do [ -S "$RUN/pipewire-0" ] && break; sleep 0.1; done
wireplumber > "$RUN/wireplumber.log" 2>&1 &
pids="$pids $!"
sleep 2

sink() {
  pw-cli create-node adapter "{ factory.name=support.null-audio-sink node.name=$1 node.description=\"$2\" media.class=$3 object.linger=true audio.position=[ $4 ] }" >/dev/null
}
sink test_speakers Speakers Audio/Sink "FL FR"
sink test_headphones "USB Headphones" Audio/Sink "FL FR"
sink test_mic Microphone Audio/Source/Virtual MONO
pw-cat --playback --format s16 --rate 48000 --channels 2 \
  -P '{ application.name=Spotify node.name=spotify media.name="Lo-fi beats" }' - < /dev/zero &
pids="$pids $!"
for _ in $(seq 50); do pw-dump 2>/dev/null | grep -q '"node.name": "spotify"' && break; sleep 0.1; done

status=0
"$@" || status=$?
if [ "$status" -ne 0 ]; then
  echo "--- pipewire.log" >&2; tail -n 20 "$RUN/pipewire.log" >&2 || true
  echo "--- wireplumber.log" >&2; tail -n 20 "$RUN/wireplumber.log" >&2 || true
fi
exit "$status"
