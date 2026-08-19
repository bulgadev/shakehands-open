# Shakehands

[![CI](https://github.com/bulgadev/shakehands-open/actions/workflows/ci.yml/badge.svg)](https://github.com/bulgadev/shakehands-open/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

Shakehands is a Linux input filter for keyboard bounce and accidental mouse
middle-clicks. It reads selected `evdev` devices, applies configurable timing
rules, and exposes matching virtual devices through `uinput` before events
reach Wayland or X11 applications.

> **Status:** experimental utility snapshot. The daemon grabs physical input
> devices and currently targets selected keyboard and mouse capabilities. Test
> the configuration with a spare input device available before enabling the
> system service.

## Why Shakehands?

Some faulty switches produce duplicate keyboard presses or phantom middle
clicks. Shakehands filters those events locally instead of changing every
application or modifying the kernel. Rules are explicit TOML and suppression
reasons are rate-limited in the system journal.

## Current Behavior

- Discards a same-key re-press only when the previous hold and gap match the
  configured bounce thresholds.
- Suppresses middle presses near wheel activity or another middle press.
- Optionally waits before emitting a middle press to reject short switch chatter.
- Can disable middle-click entirely with `block_middle = true`.
- Reconnects to matching devices after hotplug or service restarts.
- Keeps keyboard timing capture opt-in and writes aggregate numeric data only.

The current implementation is intentionally conservative about supported input
capabilities. It forwards key and relative events and does not claim to proxy
every capability of every input device.

## Requirements

- Linux with `evdev`, `uinput`, and systemd for the packaged service
- Rust 1.85 or newer
- Permission to inspect `/dev/input` and create `/dev/uinput`
- A spare keyboard, console, SSH session, or other recovery path

The checked-in packaging files currently target Arch-family systems such as
Arch and CachyOS. Other distributions may work, but are not yet packaged or
tested by this snapshot.

## Build and Safe First Run

```bash
cargo build --release --locked
sudo install -Dm755 target/release/shakehands /usr/local/bin/shakehands
sudo install -Dm644 config/shakehands.toml.example /etc/shakehands/config.toml
sudo install -Dm644 packaging/shakehands.service /etc/systemd/system/shakehands.service
```

Discover devices before editing the configuration:

```bash
./target/release/shakehands devices
sudoedit /etc/shakehands/config.toml
sudo shakehands validate --config /etc/shakehands/config.toml
sudo shakehands monitor --config /etc/shakehands/config.toml
```

`monitor` observes selected devices without grabbing them. Stop it with
`Ctrl-C`, then enable the service only after the matchers are specific:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now shakehands.service
sudo shakehands status --config /etc/shakehands/config.toml
```

The command-line binary and service paths may be adjusted for a local install.
The example file uses placeholders deliberately; do not enable it unchanged.

## Recovery

Keep another input path available before starting the daemon. If the selected
device becomes unavailable or the configuration is too broad, stop the service
from a second input path:

```bash
sudo systemctl disable --now shakehands.service
```

The service is configured to restart and release its grab when the process
exits, but recovery behavior should still be tested on the target machine.

## Commands

```text
shakehands devices                 List evdev devices and stable by-id names
shakehands validate                Validate the TOML configuration
shakehands monitor                 Print events without grabbing devices
shakehands status                  Show service and matched-device status
shakehands daemon                  Run the reconnecting filtering service
```

Inspect rate-limited filter events with:

```bash
journalctl -u shakehands.service -f
```

## Optional Keyboard Timing Capture

The example configuration disables capture by default. When explicitly enabled,
Shakehands writes one five-minute aggregate report to
`/var/lib/shakehands/keyboard-timing.json`. It stores numeric key codes and
timing histograms, not typed text or an event timeline, but users should still
review the privacy implications before enabling it.

## Testing

```bash
cargo fmt --check
cargo test --locked
cargo clippy --locked -- -D warnings
cargo build --release --locked
```

The unit suite does not replace hardware testing. Integration coverage for
uinput delivery, LEDs, hotplug, permissions, service failure, and multiple
matching devices is still a roadmap item.

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a change. Hardware
reports should include the distribution, desktop session, device metadata, and
the smallest safe configuration that reproduces the issue. Do not include
personal serial numbers or full event logs.

## Security

Shakehands reads input devices and the packaged service currently runs as root
because device grabbing and uinput access vary by distribution. Review
[SECURITY.md](SECURITY.md) and the service file before using it on a machine
with sensitive input. This snapshot is not a least-privilege claim.

## License

MIT. See [LICENSE](LICENSE).
