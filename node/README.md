# Peer-to-peer music sharing node

Every copy of the player runs this node. It shares the music its owner adds,
finds music others share, and streams it, with no server involved. The
player starts it once you join from the **Shared Music** panel (the button in
the window header, or Ctrl+U), and stops it when the app quits.

## How it works

- **Sharing.** A shared file is validated and rebuilt (see below), then
  shared over BitTorrent as its own single-file torrent. The node signs an
  *entry* for it with its Ed25519 key. The entry holds the track's details and
  the SHA-1 (info hash) and SHA-256 of its torrent `info` dictionary, about
  1 KiB in all.
- **Being found.** Nothing on the network holds a list of everyone's
  music. A node announces itself in the public BitTorrent DHT (BEP 5) under one
  keys for words of the artists, genres, titles and albums it has, and their
  three-letter prefixes. Catalogues exceeding precise-key coverage also use
  up to 256 prefix-hash routing groups within the same 400-key budget, every 24–25
  minutes after initial retries. Relays can advertise up to 1,000 additional
  keys. Deadlines are tracked per key: importing one track does not reannounce
  the entire library. Batches contain at most 64 due keys with 16 lookups in
  parallel; starts and refreshes have jitter. The DHT only stores addresses.
- **Searching.** A search looks up the DHT keys of the query's longest words
  and of their first three letters, plus routing groups, then asks the nodes found there
  (`GET /p2p/v1/search?q=…`). They answer with signed entries of matching
  tracks. Matching forgives (`src/fuzzy.rs`): "link" and "linkn prak" both
  find Linkin Park, since a word may be the start or part of another, or
  differ by a typo or two depending on its length. A typo in the first three
  letters is only forgiven among results already known. File extensions in a
  query are ignored, so "A.mp3" finds a track called "A". Results are
  remembered for a week, at most 20,000 of them, so earlier finds show up
  instantly. Answers are stored as they arrive, at most 40 nodes are asked in
  total across configured/LAN/remembered and DHT addresses. Eight nodes are asked at
  once, an answer may be at most 1 MiB, and at most three searches run at
  once, which bounds the memory a burst of searches takes. Each search
  returns a report (`SearchReport`): whether the DHT had contact, how many
  nodes were asked and answered, and why most of the others didn't, which
  the player turns into an explanation.
  Results are ordered by how closely they match; of equally close ones, the
  most shared (by the number of nodes known to have them) comes first.
  `order=popular` puts the most shared first.
  For popular words and routing groups the DHT returns a sample of nodes;
  collisions and finite candidate budgets can still miss online sources.
  Groups remove the deterministic omission of later catalogue terms, without
  making discovery exhaustive. Both sides need the updated routing logic for
  overflow coverage; legacy precise keys and wire messages remain compatible.
- **Playing.** Playing a track first gets its info dictionary (the bulk of a
  `.torrent`, up to 20 KiB of piece hashes) from a node that answered
  (`GET /p2p/v1/info`), and accepts it only if both hashes match the signed
  entry. It then adds the torrent, using the nodes that returned it as
  first peers. The BitTorrent session fetches the pieces being
  played first, so playback starts and seeks right away.
- **Ahead of time.** The player asks the node to download the next song
  while the current one plays (`POST /api/v1/tracks/{id}/prefetch`), and
  shows how songs are coming along (`GET /api/v1/tracks/{id}/transfer`).
- **Names.** A song is only shared with a name people can find: its title
  and artist together need at least five letters or digits.
- **Passing on.** Once a download completes and passes validation, the node
  keeps it, shares it, and returns its entry (still signed by the original
  publisher) in searches. Downloads are kept in a cache (5 GiB) that drops the
  least recently played first.
- **The library.** A listener's own tracks and their favorites are one
  library: own tracks always count as favorites. Everything in it is kept
  and shared, and never dropped from the cache. A favorite that is removed
  leaves the computer right away (`DELETE /api/v1/tracks/{id}/favorite`), so
  nothing outside the library stays but recent plays.
- **Cover art.** A track's picture is given when it is shared, and never
  changes: the one in the file, or one the publisher chose for the upload
  (`POST /api/v1/covers`, then the upload's `x-anysik-cover` header), or the
  folder's (`x-anysik-fallback-cover`, used only without one in the file).
  It is re-encoded as a JPEG of at most 1024 pixels, and its SHA-256 goes in
  the signed entry. Only the publisher's key can sign it. Nodes take a newer
  entry from the same publisher only if the one they have names no cover, so
  a cover can't be swapped later. Others fetch the picture when the track is
  shown (`GET /p2p/v1/cover`, at most 2 MiB) from a node that has it, keep
  it only if the hash matches, and pass it on unchanged.
- **Albums.** A node puts an album together from what it knows and from a
  network search for its artist and title (`GET /api/v1/album`), one copy
  per song, in track order. Files the node has in full and verified can be
  saved by the player (`GET /api/v1/tracks/{id}/file`), which is how whole
  albums are downloaded to a folder; nothing else is ever handed out.
- **Speed limits.** Downloading and sharing over BitTorrent can be limited
  (`PUT /api/v1/limits`, `--download-limit-kib`, `--upload-limit-kib`, or
  "Speed Limits…" in the player), and changes apply right away. Sharing may
  be at most seven times slower than downloading (`SpeedLimits::fair` in
  `common`): it can only be limited when downloading is, and never below a
  seventh of the download limit, so nobody takes much more than they give.
  Defaults are 20 MiB/s down and 10 MiB/s up. Explicit zero settings disable
  the corresponding limit. These bandwidth limits apply to BitTorrent;
  public HTTP search and relay requests have separate admission limits.
- **For you.** Taste is computed on the node from favorites (weighted most),
  play counts and own tracks. The top genres and artists are searched for,
  and the results the listener hasn't played or starred are ranked by how
  well their genre and artist match. Only the search words leave the computer.

A track is only available while someone who has it is online. Search finds
it only then too, but nobody could send it otherwise.

## Local scaling safeguards

Fuzzy scoring reads metadata in batches of 256 rows and scores outside the
SQLite connection lock. Up to four query score lists share a 2 MiB allocation
budget (plus small container overhead). Local writes and external-connection
changes invalidate cached scores. Availability and peer counts are always read
again; caching must not keep offline results visible. The first query still
scans the selected catalogue; this is not a global search index.

Each application-side DHT result collector retains at most 256 unique addresses,
processes at most 4,096 replies, and periodically yields even if a peer stream
keeps producing duplicates. Announcements drive lookups without collecting
unused peer addresses. These bounds do not change librqbit's internal queues or
constitute a guarantee about its total memory use.

A search has at most nine DHT routes for three query words, with six lookups
running concurrently under one four-second deadline. These routes share the
existing 40-node request budget, without recursive forwarding to more nodes.
Relays still ask at most eight registered clients and rotate candidates between
requests. Routing hints do not relax signature, ingress or response-size checks.

The embedded player's default-on Game Mode applies temporary BitTorrent caps
of 512 KiB/s down and 128 KiB/s up, preserves lower fair limits, and restores the
requested values afterward. It limits outgoing search concurrency to two per
search and skips speculative prefetch/recommendation work. DHT announcements
and explicit playback continue. The standalone node has no OS game detector.

These changes bound some local work and reduce redundant requests. The existing
wire protocol and DHT key scheme are unchanged. Popular word keys and the common
discovery key can still be hot spots, and relay capacity remains finite. No test
here establishes readiness for 100 million simultaneous users; that requires
larger private-network simulations and measurements of discovery and relay load.

## Behind a router

Hopea remembers up to 128 previously contacted public peers for seven days.
Searches for other tracks and relay discovery reuse them after restart, even
when DHT discovery fails. Unresponsive cached peers receive a bounded retry
delay; candidate selection rotates without increasing the per-search request
limit. Private LAN addresses are not retained in this cache. Relays are other
users' devices, not permanent Hopea servers. At least one reachable discovery
path is still necessary; neither an arbitrary network nor every track can be
guaranteed reachable.

On the same network, nodes advertise and browse `_hopea._tcp.local.` with
DNS-SD/mDNS (`src/lan.rs`). Search queries these local addresses alongside
configured peers and DHT candidates; it does not require NAT hairpinning or
internet access. Only the node's public key, protocol version and search port
are advertised, never the library or player API credentials. Expired services
are removed and discovery stops with the node. The `local_discovery` switch
controls both this discovery and BitTorrent LSD; IPv4-only VPN isolation turns
both off. Wi-Fi client isolation or a firewall blocking multicast/connections
can still prevent direct LAN discovery and transfer.

Run the real multicast regression explicitly on a host network:
`cargo test --test p2p lan_discovers -- --ignored --test-threads=1`.
It discovers two nodes without configured peers or DHT, searches, downloads,
checks the shared hash and verifies removal after shutdown.

Others can only find music shared from a node they can connect to. A node
tries, in order (`src/reach.rs`):

1. **UPnP.** It asks the router to forward the search port, and the
   BitTorrent port.
2. **A check.** It asks up to three other nodes, over IPv4 and over IPv6, to
   connect back to it (`GET /p2p/v1/dialback`). They only ever connect to
   the address the request came from, to fetch the node's own hello, so the
   check can't be used against anyone else. Passing over either family is
   enough: the search port listens on both, and the DHT announces both
   addresses.
3. **A relay.** Otherwise, for example behind carrier-grade NAT, it picks a
   reachable node that offers to relay and keeps polling it over a
   connection it opens itself (`src/relay.rs`). The relay announces the
   node's words under its own address and forwards matching searches to it.
   To send a track, the relay has the node upload it, validates it like any
   download, and shares it over BitTorrent. Every reachable node relays for
   up to 16 others unless told not to.

Successful reachability is checked again every 30 minutes. Inconclusive checks
and unsuccessful relay discovery retry after two minutes, rotating the helpers
instead of repeatedly asking the same first three addresses. A failed or
unsupported dialback does not stop a node from trying a relay. Relay hello probes
use at most four concurrent requests over 64 distinct candidates, with at most
four registration attempts per round; candidate order rotates between rounds. Configured relays and the active relay also
participate in searches when DHT is unavailable, within the existing peer budget.

The search screen shows the last checked reachability and connection suggestions.
An unconfirmed bootstrap state never appears as confirmed online status. When a
node is unreachable, searching reachable sources can still work, but its own
shares may remain undiscoverable.

Public DHT answers used for Hopea HTTP discovery must contain a native global
unicast address and an unprivileged port. Local, loopback, link-local, multicast,
reserved and IPv4 transition/translation destinations are rejected. Explicit
local peers and LAN discovery remain separate paths. Peer HTTP redirects are
never followed. Relay clients limit polling frequency and keep at most two tasks
in flight. Polling maintains the session during long tasks; excess work is not
queued and expires under the relay’s existing request timeout.
Pending tasks are cancelled when their relay session ends. Existing
public request admission, signature checks and media validation remain in force.

There is no bundled, permanently available public relay. Two nodes behind
restrictive NATs need another reachable Hopea node to relay for them (discovered
through DHT or configured with `--relay`). LAN discovery does not replace that
requirement. The relay integration test exercises forwarding and validated
audio transfer on loopback, not connectivity between real internet providers.

## Privacy

The node does not hide anyone. Other nodes and DHT participants see the IP
address of every node that announces, answers a search or shares a file, as
with any BitTorrent client. Entries name their publisher's key, and
optionally a self-chosen name, which is not verified. A relay sees the
addresses of the nodes it relays for, and the tracks they send through it.
The player says before you join that others can see your address.

## Security

Everything received from other nodes is untrusted:

- **Entries** are verified before being parsed (`src/entry.rs`). Every field
  is then checked like an upload: text must already be sanitized, numbers in
  range, codec allowed for the format, and the torrent must match the size
  and format.
- **Torrents** are accepted in one exact shape only (`src/torrent_info.rs`):
  a single file named `track.<ext>`, canonical piece length, no other keys.
  The dictionary is re-encoded and must come out byte for byte the same, so
  no name or path from anyone else ever reaches the disk.
- **The player never sees another person's file.** The node demuxes and
  decodes it with Symphonia (pure Rust), using only the reader for the
  claimed format, and serves 16-bit PCM in a WAV container whose header it
  writes itself (`src/stream.rs`). GStreamer only parses that. Range
  requests map to sample-accurate seeks. If the source runs short or is
  damaged, the rest is silence.
- **Downloads** are validated like uploads once complete, and must also come
  out of the rebuild byte for byte unchanged (`ingest::verify_download`). A
  file that fails is deleted and never fetched again.
- **Cover art** from others must match the hash in the signed entry, and
  the player only ever gets a copy decoded and re-encoded by its own node
  (`ingest::reencode_cover`: PNG or JPEG only, with size limits).
- **Relayed uploads** are only accepted for a track the relay asked for, from
  the client that offered it, up to the size the torrent says. They must
  match the torrent and pass validation like any download.
- **Answers** from other nodes are capped in size and time. The search
  endpoint is read-only, rate limited, and the only thing exposed besides
  BitTorrent. The player's API listens on loopback only and requires a token
  that is new on every start.

### How shared files are checked

Public HTTP admission (`src/ingress.rs`) uses the actual socket address, never
`Forwarded` or `X-Forwarded-For`. Searches allow a burst of six per source
(IPv6 /64), refill at one per second, and run at most two per source and eight
overall. A global search bucket refills at four per second, with a burst of
16. Excess work returns 429 without entering a waiting queue. General HTTP
requests, source-table size, socket count and connection lifetime are also
bounded. A search only consults local data and at most eight relay clients;
it never recursively starts a DHT search. Relay registrations require a fresh
Ed25519 signature and updates require the current session token. Unsigned
registrations from older builds are rejected. Task answers are bound to the
session that received the task, and pending tasks are removed on cancellation.

Incoming byte limits count actual streamed data before appending or writing,
including bodies without Content-Length. Uploads with a declared length must
match it. PNG/JPEG covers have a 16 MiB encoded-input ceiling, 8192-pixel edge
ceiling, 16,777,216-pixel ceiling and 64 MiB decoder allocation budget before
being resized and re-encoded. Optional tag reading is limited to 32 MiB of
actual reads, 65,536 seeks and eight seconds checked at I/O operations. CPU
validation permits stay with blocking tasks even if a requester disconnects.
Compressed ID3 frame support is disabled in both the node and player builds:
an encoded-input limit alone cannot bound the expanded tag payload. Image
allocation limits apply during header/metadata parsing as well as pixel decoding.
`vendor/lofty` is the published 0.25.5 source from crates.io, with only its
no-compression error return updated to the current `FrameParseError` API.
The root and standalone node manifests both use this local patch, rather than
modifying Cargo's registry cache or fetching an unreviewed development branch.

These limits are regression-tested with raw HTTP, oversized chunked bodies,
forged lengths, image headers claiming enormous dimensions, and hostile
numeric values. They do not constitute an exhaustive audit of native or Rust
dependencies, or a hard operating-system sandbox around media decoders.

The extension alone is never trusted. A file is accepted only after all of
these steps, implemented in `src/ingest/`:

1. **Extension allowlist.** Anything else is refused before the body is read.
2. **Signature.** The first bytes must be the container signature of the
   format the extension claims. Renamed executables, archives, scripts, HTML
   and documents stop here, with a message saying what the file really looks
   like.
3. **Rebuild.** The file is rewritten keeping only the audio stream and the
   headers needed to decode it. Tags, embedded pictures, padding, application
   blocks, unknown chunks and trailing data are dropped. For MP4 they are
   overwritten with zeros, since removing them would shift sample offsets.
   Text tags are re-created from sanitized values: control characters and
   bidirectional overrides are removed and the length is limited. Cover art
   is decoded and re-encoded as a plain JPEG, and only that copy is shared.
4. **Full decode.** The *rebuilt* file is demuxed with the reader for exactly
   the claimed format, never auto-detected, and every packet is decoded with
   Symphonia. The file must contain a single audio stream in an allowed
   codec, decode without errors, and pass the codec's checksums (such as the
   FLAC MD5). It must also stay within the duration, channel and sample rate
   limits, which stops "decompression bombs" that declare hours of audio in a
   few kilobytes. MP4 box structure is checked separately, so a video or
   subtitle track is rejected even if the decoder would ignore it.
5. **Byte accounting.** Almost every byte must belong to a decoded audio
   packet. Junk between MP3 frames, extra Ogg streams, or a ZIP appended to a
   FLAC all exceed the small allowance for container overhead and get the
   file rejected.

Accepted, audio only: FLAC, MP3, MPEG-4 audio (`.m4a`, AAC or ALAC, and no
video track), Ogg Vorbis and WAV (PCM). `.mp4` files are refused, since that
extension names video. Opus, Matroska/WebM and other
formats are rejected, because the validator cannot fully decode them.

### What this does not protect against

- Data hidden *inside* the audio samples (steganography), or in the small
  overhead allowance of step 5, can't be detected. Such data is inert: it
  never reaches the player, which only gets decoded PCM.
- A malicious node can answer searches with entries for real files that are
  mislabelled. Validation catches files that aren't clean audio, not audio
  that isn't what its title says.
- Copyright: you are responsible for what you share.

## Running a node without the player

The same node can run on its own, for example on a computer that is always
on, to keep your music available while you're offline:

```sh
cargo build --release
./target/release/hopea-node serve --name alice      # data in ~/.local/share/hopea-node
./target/release/hopea-node token                   # the token for its player API
```

It works best with the search port (47391 by default) and the BitTorrent
port reachable from the internet; see "Behind a router". The player
API stays on loopback; reach it over SSH (`ssh -L 8740:127.0.0.1:8740 host`)
to share files from elsewhere. `serve --help` lists the options. `--peer
host:port` always searches a given node, for example a friend on the same
network; `--no-dht` searches only those. `--relay host:port` tries that node
first as a relay, `--force-relay` always uses one, and `--no-relay` stops
this node from relaying for others.

The player has the same option as a setting:
`gsettings set io.hopea.Hopea library-peers "['192.168.1.20:47391']"`.

To check a file without sharing it, run `hopea-node check song.flac`.

## Development

```sh
cargo test                       # everything
cargo test --test p2p            # real nodes on loopback: share, search, stream, pass on, feed
cargo test --test ingest         # the validation pipeline (real and hostile files)
cargo test --test api accepts    # a single test by name
```

Fixtures in `tests/fixtures/` are one-second test tones generated with
GStreamer (`audiotestsrc`). The hostile files are built from them at runtime
inside the tests.

### Cover and lyrics alternatives

Attachments are keyed by the SHA-256 of the audio packets, separately from the
file hash and its original signed entry. Reimporting audio already held locally
reuses its track ID, even with different tags or artwork. Uploading an identical
attachment reuses its content hash. Discovery and recommendations group matching
audio; the original entries remain available internally for torrent verification.

The player enables automatic selection of the most shared cover and lyrics by
default. These are independent choices. Disabling the setting offers a manual
choice when adding a song; the song menu also opens the chooser. Lyrics comparison
uses the main player and returns to the same chooser without saving the audition.

`GET /p2p/v1/metadata?audio=<sha256>&nonce=<random-sha256>` returns a signed
manifest. Nonce-bound proofs of content possession allow reuse of cached bytes.
New bytes are fetched from `/p2p/v1/metadata/data?audio=...&hash=...` and checked
against their audio-bound content hash before decoding. Public endpoints serve
only local shared data and never initiate further network requests. The player
API exposes `/api/v1/tracks/{id}/variants` and `/variants/{hash}`; PUT on the latter
selects a variant. User-contributed alternatives remain shared; downloaded previews
are advertised only after selection. Both are stored independently of audio files.

Popularity counts distinct responding keys seen within 60 seconds, plus this
node if it shares that variant. A peer's supplied rating is ignored. The query
checks at most eight known endpoints, with a 20-second total budget and a
15-second refresh cooldown. Counts describe observed copies, not a global census
or unique people: independent keys do not provide Sybil resistance. Track ranking
likewise counts recent distinct source endpoints instead of historical downloads.

Limits: 16 variants and 4 MiB per audio identity, 128 MiB of attachment data in
total, 1 MiB per image, and 256 KiB per lyrics document. Lyrics additionally have
line-count, line-length and control-character checks; images use the bounded
PNG/JPEG decoder. Declared lengths never override streamed-byte limits. Relay
attachment uploads require an authenticated session and an outstanding audio
request. Old peers can still transfer the unchanged torrent; updated peers are
needed for alternatives. Legacy publisher-signed lyrics remain supported.

Both client and node use `common::lyrics` to parse and validate LRC. Limits are
2,000 physical/expanded lines, 16 KiB per line and a 24-hour absolute timestamp
ceiling. Negative/overflowing timestamps, seconds >= 60, malformed numeric tags,
nonzero offsets and unsupported word-level timing are rejected, not silently
interpreted as plain text. Known audio duration further bounds timestamps.
Completed downloads must match their signed duration as measured by the decoder;
received metadata is not advertised or served before that verification. Older
held audio is rechecked on resume. Invalid cached lyrics cannot be selected or
forwarded. Hash binding identifies the audio, but cannot establish whether a
well-formed text is semantically the correct lyrics; users still need comparison.

`vendor/socket-pktinfo` is upstream 0.4.2 with a small portability patch:
OpenBSD/DragonFly use FreeBSD's IP_RECVDSTADDR/IP_RECVIF implementation, and
NetBSD/OpenBSD omit the optional indexed IPv4 multicast methods absent in
socket2. Hopea's mDNS uses the address-based methods. FreeBSD and NetBSD API
cross-checks pass; OpenBSD and DragonFly still require native validation.
