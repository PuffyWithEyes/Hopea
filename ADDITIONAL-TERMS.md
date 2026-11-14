# Additional terms for this fork's own code

This program is free software under the GNU General Public License, version 3
or any later version (see `COPYING`). Nothing here takes away any of the
freedoms that license gives: anyone may run, study, change and share the
program.

The code written for this fork, that is, every file whose SPDX copyright line
names Daniil Klimov (all of `node/` and `common/`, `src/library/`,
`src/library_panel.rs` and others), is also covered by the following terms. Section 7 of
the GPL version 3 allows them for material added to a covered work, under
points (c) and (e):

1. **Modified versions must say that they are modified**, in reasonable ways:
   in the name or version they show to the people using them, and in how they
   present themselves to other computers on the network. A modified version
   must not send the `User-Agent` this program sends (`hopea/<version>`), or
   anything that could be mistaken for it.
2. **They must not claim to be the original program**, or suggest that its
   authors made or approve of them.
3. **The name "hopea" is not licensed** for use as the name of a modified
   version.

Why: the network works because every node follows the same rules, such as
sharing at most seven times slower than it downloads
(`SpeedLimits::fair` in `common/src/api.rs`). The license can't forbid
changing those rules, and doesn't try to. These terms only make sure that a
changed program can't pass itself off as this one, so that other nodes can
tell it apart and treat it as they choose.

These terms don't cover the code of Hopea, on which this program is built,
which is under the GPL alone.
