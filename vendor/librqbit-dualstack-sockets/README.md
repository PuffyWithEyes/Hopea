# librqbit-dualstack-sockets

Hopea vendors version 0.7.0 from crates.io. The local change restricts
`Socket::bind_device` to the platforms where socket2 provides that method.
Other platforms return `BindDeviceNotSupported` when binding is requested;
ordinary unbound sockets remain available. macOS keeps its index-based binding.

A library that provides dual-stack tokio sockets for use in [rqbit](https://github.com/ikatson/rqbit) torrent client.

It converts between SocketAddr addresses so that your app sees IPv4 (not IPv4-mapped IPv6) addresses.

If you listen on IPv6::UNSPECIFIED, it will enter dualstack mode (if requested) and listen on both IPv4 and IPv6. However all addresses will be converted
to their canonical form.
