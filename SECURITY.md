# Security policy

Cleanse deletes files, so we treat safety bugs as security bugs.

## Reporting a vulnerability

Please report vulnerabilities privately through GitHub: open the repository's **Security** tab
and choose **Report a vulnerability**. Don't open a public issue.

Include the Cleanse version, your operating system, steps to reproduce, and what you believe
the impact is. We aim to acknowledge reports within a week.

## In scope

- Cleanse deleting, moving, or modifying anything outside a plan the user approved
- Bypassing the protected-path policy or a safety gate, for example through crafted paths,
  links, rule files, or plan files
- A rule file causing anything other than path matching, such as running a command
- Privilege escalation through system cleanup (`cleanse system`) or the desktop app's
  elevated helper (once it exists), such as by planting files, links, or folders in the system
  folders it cleans, or in its data folder
- File paths or file contents leaving the machine

A false positive that didn't cause data loss can go through the public **False positive**
issue form instead.

System cleanup's design, and the risks it accepts, such as running a `cleanse` binary that
your own account can replace, are in
[ADR 0010](docs/src/adr/0010-elevated-system-cleanup.md).

## Supported versions

Cleanse is pre-1.0. Only the latest release receives fixes.
