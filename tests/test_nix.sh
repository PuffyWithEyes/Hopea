#!/usr/bin/env bash
# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail
cd "$(dirname "$0")/.."
# Evaluate module contracts without activating any user/system configuration.
nix --extra-experimental-features 'nix-command flakes' eval --impure --json --expr '
let
  lock = (builtins.fromJSON (builtins.readFile ./flake.lock)).nodes.nixpkgs.locked;
  pkgs = import (builtins.fetchTree lock) {};
  inherit (pkgs) lib;
  flake = (import ./flake.nix).outputs { self = flake; nixpkgs = { inherit lib; }; };
  evaluate = module: destination: enable: custom:
    let
      result = lib.evalModules {
        specialArgs = { inherit pkgs; };
        modules = [
          module
          { options = lib.setAttrByPath destination (lib.mkOption {
              type = lib.types.listOf lib.types.package; default = [];
            });
          }
          ({ ... }: { programs.hopea = { inherit enable; }
            // lib.optionalAttrs custom { package = pkgs.hello; }; })
        ];
      };
      packages = lib.getAttrFromPath destination result.config;
    in assert builtins.length packages == (if enable then 1 else 0);
      assert !enable || (builtins.head packages).pname == (if custom then "hello" else "hopea");
      true;
  cases = [
    [ flake.nixosModules.default [ "environment" "systemPackages" ] ]
    [ flake.darwinModules.default [ "environment" "systemPackages" ] ]
    [ flake.nixbsdModules.default [ "environment" "systemPackages" ] ]
    [ flake.homeManagerModules.default [ "home" "packages" ] ]
    [ flake.nixOnDroidModules.default [ "environment" "packages" ] ]
  ];
in assert lib.all (case: lib.all (enable: lib.all (custom:
  evaluate (builtins.elemAt case 0) (builtins.elemAt case 1) enable custom
) [ false true ]) [ false true ]) cases;
{ moduleCases = 20; passed = true; }
'
