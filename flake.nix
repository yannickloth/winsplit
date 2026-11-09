{
  description = "winsplit — nested Wayland compositor: two apps in one resizable split window";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAll (pkgs: {
        default = pkgs.rustPlatform.buildRustPackage rec {
          pname = "winsplit";
          version = "0.1.0";
          src = self;
          cargoLock.lockFile = ./Cargo.lock;
        };
      });

      devShells = forAll (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            # Rust toolchain
            rustc
            cargo
            clippy
            rustfmt
            rust-analyzer

            # Wayland runtime client libs (winit nested backend links against them)
            wayland

            # Input / keymap (winit)
            libxkbcommon

            # Rendering: GPU (glow -> EGL/GL ES, from mesa) + CPU fallback (pixman)
            mesa
            pixman
          ];
        };
      });
    };
}
