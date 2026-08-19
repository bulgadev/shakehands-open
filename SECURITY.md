# Security Policy

Shakehands is an experimental input service. The packaged systemd unit runs as
root on the currently supported installations, reads selected `/dev/input`
devices, grabs them, and creates virtual devices through `/dev/uinput`.

## Reporting a Vulnerability

Do not open a public issue for a vulnerability. Use GitHub's private security
reporting for this repository if it is enabled, or contact the maintainer
through the account profile before sharing details publicly.

Include the affected commit, distribution, service configuration, and a safe
reproduction. Remove hardware serial numbers, credentials, and private logs.

## Current Security Boundaries

- Treat the configuration as privileged input.
- Use the narrowest possible `by_id_contains`, name, vendor, and product
  matchers.
- Keep keyboard timing capture disabled unless its privacy tradeoff is accepted.
- Keep a second input or remote recovery path available.
- Do not use this snapshot as a least-privilege or production-hardening claim.
