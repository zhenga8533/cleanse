# 0006: Dual MIT and Apache-2.0 license

**Status:** Accepted

## Context

Cleanse should be easy to adopt, package for operating system distributions, and contribute
to. Options considered: a permissive license, GPL-3.0 (copyleft for all redistribution), and
MPL-2.0 (file-level copyleft).

## Decision

License Cleanse under MIT OR Apache-2.0, at the user's option, the convention in the Rust
ecosystem. Contributions are accepted under the same terms (inbound equals outbound), as stated
in the README and CONTRIBUTING.

## Consequences

- No barrier for companies, distributions, or other projects that want to use or embed Cleanse.
- Apache-2.0 adds an explicit patent grant.
- Forks can be closed-source.
- Dependencies must use compatible licenses; cargo-deny enforces an allow-list. GPL-licensed
  components, such as Krokiet's GUI, can't be bundled.
