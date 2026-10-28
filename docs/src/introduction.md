# Introduction

Cleanse finds files you can safely remove, explains why, and removes only what you approve. It
runs on Windows, macOS, and Linux, with particular attention to what developers accumulate:
dependency folders, build output, package-manager caches, and the files AI coding agents leave
behind.

These docs describe the design. Cleanse is pre-alpha, so they describe intended behavior that
is being built milestone by milestone; each page says what is implemented.

- [Architecture](architecture.md): how the code is organized and why.
- [Safety model](safety-model.md): how Cleanse decides what it may touch.
- [Rules](rules.md): how cleanup knowledge is written down.
- [Decision records](adr/README.md): the significant choices and their trade-offs.
