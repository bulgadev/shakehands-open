# Contributing

Thanks for helping improve Shakehands. Changes should make filtering safer,
more predictable, or easier to install without hiding important limitations.

## Before Opening an Issue

- Confirm the problem still exists on the latest `main`.
- Run `shakehands validate` with the smallest relevant configuration.
- Do not include personal serial numbers, full input event logs, or private
  machine details.
- For hardware issues, include the Linux distribution, desktop session,
  permissions, device class, and a redacted `shakehands devices` result.

## Local Checks

```bash
cargo fmt --check
cargo test --locked
cargo clippy --locked -- -D warnings
cargo build --release --locked
```

Hardware and service changes also need a manual recovery test with a spare
input path available. Do not test a new matcher on your only keyboard or mouse.

## Pull Requests

Explain the user problem, the safety boundary, how the change was tested, and
any supported-platform limitations. Keep unrelated refactors out of the same
change. New claims about input preservation need a regression test or a clear
hardware-test note.
