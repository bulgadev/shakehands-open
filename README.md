# Shakehands

Shakehands is a small Linux service that filters keyboard bounce and accidental mouse-middle clicks before Wayland or X11 applications receive them. It uses `evdev` to grab only the selected physical devices, then creates matching virtual input devices through `uinput`.

## Behavior

- A same-key press is discarded only when its previous hold was at most 12 ms and its re-press arrives within 45 ms; normal fast re-taps and kernel typematic repeats are preserved.
- Middle presses within 250 ms of wheel activity are discarded.
- A physical middle press within 350 ms of the prior physical middle press is discarded, even when the prior press was already rejected.
- Optional `middle_min_hold_ms` (example: 80): a middle press is only emitted after it has been held that long. Short phantom clicks from a flaky wheel switch are dropped; intentional clicks still work (press is flushed once the hold matures, or press+release are emitted together on release).
- Optional `block_middle = true` drops every middle press and release when you never need middle-click paste or free-scroll.
- All thresholds are in milliseconds and configurable in TOML.

## Build and install (CachyOS/Arch)

```bash
cargo build --release --offline
sudo install -Dm755 target/release/shakehands /usr/local/bin/shakehands
sudo install -Dm644 config/shakehands.toml.example /etc/shakehands/config.toml
sudo install -Dm644 packaging/shakehands.service /etc/systemd/system/shakehands.service
```

First identify the exact device metadata, then edit `/etc/shakehands/config.toml` to use the most specific available `by_id_contains` and/or vendor/product pair:

```bash
./target/release/shakehands devices
sudo systemctl daemon-reload
sudo systemctl enable --now shakehands.service
./target/release/shakehands status
```

`devices` is safe to run as a regular user. `monitor` reads configured devices without grabbing them and normally needs access to `/dev/input`; use it briefly while diagnosing. `daemon` grabs the chosen device, so run it only through the service or a root shell.

## Commands

```text
shakehands devices                 List evdev devices and stable /dev/input/by-id names
shakehands validate                Validate the TOML configuration
shakehands monitor                 Print raw events from selected devices without grabbing
shakehands status                  Show systemd activity and currently connected matched devices
shakehands daemon                  Run the reconnecting filtering service
```

Every filter action is sent to the journal as `shakehands:` with per-reason rate limiting. Inspect it with `journalctl -u shakehands.service -f`.

## Keyboard timing capture

The example config enables one five-minute capture after the service starts. It runs inside the daemon, so filtering remains active while you type, edit, and use gaming keys. Its report is written to `/var/lib/shakehands/keyboard-timing.json`; the report contains aggregate numeric key-code timing histograms and suppression candidates only—never characters or a key-event timeline.

After five minutes of mixed normal use, inspect it with:

```bash
sudo jq . /var/lib/shakehands/keyboard-timing.json
```

`keyboard_bounce_pulse_ms` is the maximum duration of the first physical pulse and `keyboard_repress_gap_ms` is the maximum pause before the second pulse. Start with 12 ms and 45 ms respectively. If the report shows real bounce candidates outside those values, adjust the config and restart the service. Set `[capture].enabled = false` after collecting the report to avoid replacing it on future restarts.

## Safety note

Before enabling the service, keep another input device available. A too-broad matcher can make a selected physical device unavailable if the service cannot create its virtual replacement; systemd will restart it and release the grab when the process exits.
