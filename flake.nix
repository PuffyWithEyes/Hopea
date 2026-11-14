# SPDX-FileCopyrightText: 2026 Daniil Klimov
# SPDX-License-Identifier: CC0-1.0
{
  description = "Hopea music player and Nix integrations";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
      # Only the installation destination differs between module systems.
      mkModule =
        destination:
        {
          config,
          lib,
          pkgs,
          ...
        }:
        {
          options.programs.hopea = {
            enable = lib.mkEnableOption "the Hopea music player";
            package = lib.mkOption {
              type = lib.types.package;
              default = pkgs.callPackage ./nix/package.nix { };
              defaultText = lib.literalExpression "hopea.lib.mkPackage pkgs";
              description = "Package to install; does not enable sharing or alter privacy settings.";
            };
          };
          config = lib.mkIf config.programs.hopea.enable (
            lib.setAttrByPath destination [ config.programs.hopea.package ]
          );
        };
    in
    {
      overlays.default = final: prev: { hopea = final.callPackage ./nix/package.nix { }; };
      packages = forAllSystems (pkgs: rec {
        hopea = pkgs.callPackage ./nix/package.nix { };
        default = hopea;
      });
      apps = forAllSystems (pkgs: rec {
        hopea = {
          type = "app";
          program = "${self.packages.${pkgs.stdenv.hostPlatform.system}.hopea}/bin/hopea";
          meta.description = "Hopea music player";
        };
        default = hopea;
      });
      devShells = forAllSystems (pkgs: {
        default = import ./nix/shell.nix { inherit pkgs; };
      });
      nixosModules.default = mkModule [
        "environment"
        "systemPackages"
      ];
      darwinModules.default = mkModule [
        "environment"
        "systemPackages"
      ];
      nixbsdModules.default = mkModule [
        "environment"
        "systemPackages"
      ];
      homeManagerModules.default = mkModule [
        "home"
        "packages"
      ];
      nixOnDroidModules.default = mkModule [
        "environment"
        "packages"
      ];
      # FreeBSD uses its host's package set (often NixBSD's patched nixpkgs).
      # Keeping it out of `packages` avoids advertising an unverified binary.
      lib.mkPackage = pkgs: pkgs.callPackage ./nix/package.nix { };
    };
}
