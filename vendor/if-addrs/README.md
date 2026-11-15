# if-addrs
https://crates.io/crates/if-addrs

## Overview

Retrieve network interface info for all interfaces on the system:

```rust
// List all of the machine's network interfaces
for iface in if_addrs::get_if_addrs().unwrap() {
    println!("{:#?}", iface);
}
```

Get notifications for changes in network interfaces:

```rust
let mut notifier = if_addrs::IfChangeNotifier::new().unwrap();
loop {
    if let Ok(details) = notifier.wait(None) {
        println!("{:#?}", details);
    }
}
```

## License

This SAFE Network library is dual-licensed under the Modified BSD ([LICENSE-BSD](LICENSE-BSD) https://opensource.org/licenses/BSD-3-Clause) or the MIT license ([LICENSE-MIT](LICENSE-MIT) http://opensource.org/licenses/MIT) at your option.

## Contribution

Copyrights in the SAFE Network are retained by their contributors. No copyright assignment is required to contribute to this project.

## Hopea portability patch

DragonFly BSD uses the existing `getifaddrs` enumeration path, like FreeBSD,
NetBSD and OpenBSD. It must not compile the Linux Netlink change notifier.
The notifier API is unavailable on these BSD targets; interface addresses,
indices and flags remain available. Hopea's mdns-sd dependency already polls
for interface changes and does not use this notifier API.

The notifier example uses the same platform exclusions so all examples can be
checked on BSD too.
