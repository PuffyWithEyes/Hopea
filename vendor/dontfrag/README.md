# dontfrag-rs

[![Crate](https://shields.io/crates/v/dontfrag?style=for-the-badge)](https://crates.io/crates/dontfrag/)
[![Documentation](https://img.shields.io/docsrs/dontfrag?style=for-the-badge)](https://docs.rs/dontfrag/latest/dontfrag/)

Rust library for setting Don't Fragment option (DF) on UDP sockets.

## Platform support

* BSD-like: macOS, iOS, tvOS, watchOS, FreeBSD, Dragonfly, OpenBSD, NetBSD
* Linux-like: Linux, L4RE, Android, Emscripten
* Windows

PRs for additional platforms are welcome!

## Optional features

* `tokio`: Adds support for `tokio::net::UdpSocket`.

## License

Licensed under either of

 * Apache License, Version 2.0
   ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license
   ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.

## Hopea portability patch

NetBSD, OpenBSD and DragonFly BSD do not expose the per-socket IPv4
`IP_DONTFRAG` option assumed upstream. IPv4 accessors return `Unsupported` on
these targets; IPv6 still uses `IPV6_DONTFRAG`. librqbit's uTP socket setup
already handles this error and continues without the optional IPv4 setting.

Kernel headers: [NetBSD](https://github.com/NetBSD/src/blob/trunk/sys/netinet/in.h),
[OpenBSD](https://github.com/openbsd/src/blob/master/sys/netinet/in.h),
[DragonFly BSD](https://github.com/DragonFlyBSD/DragonFlyBSD/blob/master/sys/netinet/in.h).
