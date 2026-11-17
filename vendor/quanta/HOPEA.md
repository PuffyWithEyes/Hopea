# Hopea compatibility patch

This is quanta 0.12.6 from crates.io, with the one-line `timespec::default()`
fix from [upstream PR #121](https://github.com/metrics-rs/quanta/pull/121),
commit `8089ec8f791de608f93bd1b7b377f8d044578bb4` by Leon White.
It allows compilation with libc's private padding fields on 32-bit musl.
Clock selection, calibration and rate-limit behavior are unchanged.

Quanta 0.13.0 includes the fix, but governor 0.10.4 still requires quanta 0.12.
Remove this path patch when governor accepts a fixed published release.
The upstream source and MIT license notices are retained.
