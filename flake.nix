{
  description = "rox - a desktop music player for large, carefully tagged local libraries";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forEachSystem =
        f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      devShells = forEachSystem (
        pkgs:
        let
          inherit (pkgs) lib stdenv;

          # GPUI dlopens these at runtime on Linux (blade renders via Vulkan,
          # windowing via Wayland or X11). They have to be on the library path
          # because nothing links them at build time.
          runtimeLibs = lib.optionals stdenv.isLinux (
            with pkgs;
            [
              vulkan-loader
              wayland
              libxkbcommon
            ]
          );
        in
        {
          default = pkgs.mkShell {
            packages =
              with pkgs;
              [
                rustc
                cargo
                rustfmt
                clippy
                rust-analyzer
                pkg-config
              ]
              ++ lib.optionals stdenv.isLinux [
                # windowing and input
                wayland
                libxkbcommon
                libxcb
                libx11
                # rendering
                vulkan-loader
                # text
                fontconfig
                freetype
                # audio output (cpal)
                alsa-lib
                # misc native deps
                openssl
                zlib
              ];

            env = lib.optionalAttrs stdenv.isLinux {
              LD_LIBRARY_PATH = lib.makeLibraryPath runtimeLibs;
            };
          };
        }
      );
    };
}
