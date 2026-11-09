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

            # Wayland runtime client libs (winit nested backend dlopens them)
            wayland

            # Input / keymap (winit)
            libxkbcommon

            # Rendering: GPU (glow -> EGL/GL ES): libglvnd = loaders
            # (libEGL.so.1), mesa = drivers/ICD; CPU fallback: pixman
            libglvnd
            mesa
            pixman
          ];

          # The winsplit binary is built by cargo (no nix RPATH patching), so
          # expose the runtime libs to dlopen/ld explicitly.
          LD_LIBRARY_PATH = pkgs.lib.makeLibraryPath (with pkgs; [
            wayland
            libxkbcommon
            libglvnd
            mesa
            pixman
          ]);
        };
      });
    };
}
