# Documentation screenshots

The README images are rendered from Hopea's actual GTK widgets, models and
factories. They use an isolated demonstration catalog featuring Linkin Park,
Янка Дягилева, Великие Октябри, Егор Летов and Гражданская Оборона.
Playback position, waveform samples, track durations and network status are
simulated. No music recordings, service credentials or artist cover artwork are
included, and no peers are contacted. These images demonstrate the interface,
not availability of these recordings on the network.

The desktop layout is captured at 1100 × 760 and the mobile layout at 390 × 780.
Mobile captures use the adaptive GTK layout and Android panel policy on an X11
test display. They are not screenshots from an Android device and do not verify
APK behavior. All interface text is English; artist and song names retain their
original script.

## Recreate the images

Configure and compile `_builddir` using the [contributing guide](CONTRIBUTING.md).
Then run the fixture with Xvfb in addition to the usual development dependencies:

```sh
nix-shell --expr '
  let
    pkgs = import (builtins.fetchTree
      (builtins.fromJSON (builtins.readFile ./flake.lock)).nodes.nixpkgs.locked) {};
    shell = import ./nix/shell.nix { inherit pkgs; };
  in shell.overrideAttrs (old: {
    nativeBuildInputs = old.nativeBuildInputs ++ [ pkgs.xorg.xorgserver ];
  })
' --run 'bash tests/capture_screenshots.sh'
```

On another development system with Xvfb available, run
`bash tests/capture_screenshots.sh`. `HOPEA_XVFB` can select its executable.
The script starts a private display and creates temporary XDG directories and
memory-backed preferences. It does not attach to or restart a running Hopea.
Images are written to `docs/screenshots/`.

The same fixture checks that the import chooser opens and cancels cleanly,
that its description uses the card width with Russian text at 360, 390 and
560 pixels, and that its window fits the viewport. `mobile-import.png` records
the corrected layout, with buttons below descriptions instead of beside them.
