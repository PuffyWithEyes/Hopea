<!-- Hopea local patch -->
Hopea vendors librqbit 9.0.1 from crates.io. The local HTTP client helper
limits interface binding to platforms supported by reqwest. Explicit binding
on unsupported platforms returns an error; an unconstrained client still works.
The library is patched in both the player and standalone node Cargo manifests.

# librqbit

A fully featured, easy to use torrent downloading library used as a backbone of [rqbit](https://github.com/ikatson/rqbit).

## Basic example
See [examples on GitHub](https://github.com/ikatson/rqbit/tree/main/crates/librqbit/examples).

## Documentation
[librqbit at docs.rs](https://docs.rs/librqbit/latest/librqbit/)