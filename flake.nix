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

            # Regenerate the patched gpui copy Cargo's [patch.crates-io]
            # points at (see patches/gpui). Stamped, so it's a no-op on
            # every shell entry after the first.
            shellHook = ''
              ./scripts/vendor-gpui.sh
            ''
            # GPUI's build script compiles Metal shaders with `xcrun metal`,
            # and nix can't ship Apple's Metal toolchain. Undo the SDK env
            # mkShell sets, drop the stub xcrun nixpkgs puts on PATH, and
            # lean on real Xcode instead. Xcode 26 users need to grab the
            # toolchain once: xcodebuild -downloadComponent MetalToolchain
            + lib.optionalString stdenv.isDarwin ''
              unset SDKROOT
              export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer
              export PATH=$(printf '%s' "$PATH" | tr ':' '\n' | grep -v xcbuild | paste -sd: -)
            '';
          };
        }
      );
    };
}
