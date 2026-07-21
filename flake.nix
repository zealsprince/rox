{
  description = "rox - a desktop music player for large, carefully tagged local libraries";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
  };

  outputs =
    { self, nixpkgs }:
    let
      lib = nixpkgs.lib;
      systems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];
      forEachSystem = f: lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});

      # gpui dlopens these at runtime on Linux (blade renders via Vulkan,
      # windowing via Wayland or X11). They have to be on the library path
      # because nothing links them at build time.
      runtimeLibs =
        pkgs: with pkgs; [
          vulkan-loader
          wayland
          libxkbcommon
        ];

      # Native libraries the Linux build links against.
      linuxBuildInputs =
        pkgs: with pkgs; [
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

      # A crates.io tarball with our patches from patches/<name>/ applied,
      # mirroring what scripts/vendor-gpui.sh produces for non-nix builds.
      # The build sandbox has no network, so the shellHook approach can't
      # work here; this is the same fetch-verify-patch done as a derivation.
      # Versions and tarball checksums are duplicated from the script's
      # crates table, bump both together.
      vendoredCrate =
        pkgs: name: version: sha256:
        pkgs.stdenvNoCC.mkDerivation {
          name = "${name}-${version}-patched";
          src = pkgs.fetchurl {
            url = "https://static.crates.io/crates/${name}/${name}-${version}.crate";
            inherit sha256;
          };
          unpackCmd = ''tar -xzf "$curSrc"'';
          patches = lib.filesystem.listFilesRecursive ./patches/${name};
          dontConfigure = true;
          dontBuild = true;
          installPhase = "cp -r . $out";
        };

      # Linux only: on macOS gpui's build script compiles Metal shaders with
      # Apple's toolchain, which nix can't ship. Use the dev shell there.
      mkRox =
        pkgs:
        let
          gpui = vendoredCrate pkgs "gpui" "0.2.2"
            "979b45cfa6ec723b6f42330915a1b3769b930d02b2d505f9697f8ca602bee707";
          gpuiComponent = vendoredCrate pkgs "gpui-component" "0.5.1"
            "d021d46b4088d3d93a57ccdf443da85695a77272108caca2f6fe5369f584966a";
        in
        pkgs.rustPlatform.buildRustPackage {
          pname = "rox";
          version = (lib.importTOML ./Cargo.toml).workspace.package.version;
          src = self;

          # gpui and gpui-component resolve as path deps in the lock, so no
          # extra hashes are needed for them here.
          cargoLock.lockFile = ./Cargo.lock;

          # Materialize the patched vendor copies [patch.crates-io] points
          # at, in place of the script run the dev shellHook does.
          postPatch = ''
            mkdir -p vendor
            cp -r ${gpui} vendor/gpui
            cp -r ${gpuiComponent} vendor/gpui-component
          '';

          nativeBuildInputs = [ pkgs.pkg-config ];
          buildInputs = linuxBuildInputs pkgs;

          # The dlopened libs are unused at link time, so the fixup phase's
          # rpath shrink would strip them; append after it runs.
          postFixup = ''
            patchelf --add-rpath ${lib.makeLibraryPath (runtimeLibs pkgs)} $out/bin/rox
          '';

          postInstall = ''
            install -Dm644 crates/rox/assets/app/rox.desktop $out/share/applications/rox.desktop
            install -Dm644 crates/rox/assets/app/rox.svg $out/share/icons/hicolor/scalable/apps/rox.svg
            install -Dm644 crates/rox/assets/app/rox.png $out/share/pixmaps/rox.png
          '';

          meta = {
            description = "A desktop music player for large, carefully tagged local libraries";
            homepage = "https://github.com/zealsprince/rox";
            license = lib.licenses.agpl3Only;
            mainProgram = "rox";
            platforms = lib.platforms.linux;
          };
        };
    in
    {
      packages = forEachSystem (
        pkgs:
        lib.optionalAttrs pkgs.stdenv.isLinux (
          let
            rox = mkRox pkgs;
          in
          {
            default = rox;
            inherit rox;
          }
        )
      );

      overlays.default = final: prev: { rox = mkRox final; };

      devShells = forEachSystem (
        pkgs:
        let
          inherit (pkgs) stdenv;
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
              ++ lib.optionals stdenv.isLinux (linuxBuildInputs pkgs);

            env = lib.optionalAttrs stdenv.isLinux {
              LD_LIBRARY_PATH = lib.makeLibraryPath (runtimeLibs pkgs);
            };

            # Regenerate the patched gpui copy Cargo's [patch.crates-io]
            # points at (see patches/gpui). Stamped, so it's a no-op on
            # every shell entry after the first.
            shellHook = ''
              ./scripts/vendor-gpui.sh
            ''
            # gpui's build script compiles Metal shaders with `xcrun metal`,
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
