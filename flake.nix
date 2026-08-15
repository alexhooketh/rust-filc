{
  description = "A safe, generated Rust bridge to whole-program Fil-C helpers";

  # Keep clean-worker realizations inside modest resource envelopes and permit
  # local reconstruction when a configured binary cache is unavailable.
  nixConfig = {
    cores = 2;
    fallback = true;
    max-jobs = 1;
  };

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.11";
  };

  outputs =
    { self, nixpkgs }:
    let
      systems = [
        "aarch64-darwin"
        "x86_64-darwin"
        "x86_64-linux"
      ];
      forAllSystems = nixpkgs.lib.genAttrs systems;
      mkFilc =
        pkgs:
        pkgs.stdenvNoCC.mkDerivation {
          pname = "filc";
          version = "0.683";
          src = pkgs.fetchurl {
            url = "https://github.com/pizlonator/fil-c/releases/download/v0.683/filc-0.683-linux-x86_64.tar.xz";
            hash = "sha256-D7whNa0w1bCt8xKJvMbw2gzI2yMj9OrCl41fg1ONEMY=";
          };
          sourceRoot = "filc-0.683-linux-x86_64";
          nativeBuildInputs = [ pkgs.patchelf ];
          dontBuild = true;
          dontStrip = true;
          installPhase = ''
            runHook preInstall
            mkdir -p "$out"
            cp -a . "$out/"
            chmod -R u+w "$out"

            patchelf \
              --set-interpreter "${pkgs.stdenv.cc.bintools.dynamicLinker}" \
              --set-rpath "${pkgs.lib.makeLibraryPath [ pkgs.glibc ]}" \
              "$out/build/bin/clang-20"

            runtime_rpath="$out/pizfix/lib64:$out/pizfix/lib"
            for library in \
              pizfix/lib/libc.so \
              pizfix/lib/libpizlo.so \
              pizfix/lib/libc++.so.1.0 \
              pizfix/lib/libc++abi.so.1.0 \
              pizfix/lib_test/libpizlo.so \
              pizfix/lib_test_gcverify/libpizlo.so \
              pizfix/lib_gcverify/libpizlo.so
            do
              patchelf --set-rpath "$runtime_rpath" "$out/$library"
            done

            mkdir "$out/pizfix/os-include"
            ln -s "${pkgs.linuxHeaders}/include/linux" "$out/pizfix/os-include/linux"
            ln -s "${pkgs.linuxHeaders}/include/asm" "$out/pizfix/os-include/asm"
            ln -s "${pkgs.linuxHeaders}/include/asm-generic" "$out/pizfix/os-include/asm-generic"
            runHook postInstall
          '';
        };
    in
    {
      devShells = forAllSystems (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
          filc = if system == "x86_64-linux" then mkFilc pkgs else null;
          filcPackages = pkgs.lib.optionals (system == "x86_64-linux") [
            filc
          ];
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              cargo
              clippy
              just
              rustc
              rustfmt
            ] ++ filcPackages;
            FILC_CC =
              if system == "x86_64-linux" then
                "${filc}/build/bin/clang"
              else
                "";
          };
        }
      );

      checks.x86_64-linux =
        let
          pkgs = import nixpkgs { system = "x86_64-linux"; };
          filc = mkFilc pkgs;
        in
        {
          workspace = pkgs.stdenv.mkDerivation {
            pname = "rust-filc-check";
            version = "0.1.0";
            src = self;
            nativeBuildInputs = with pkgs; [
              cargo
              clippy
              rustc
              rustfmt
              filc
            ];
            FILC_CC = "${filc}/build/bin/clang";
            buildPhase = ''
              cargo test --locked --workspace
            '';
            installPhase = ''
              touch $out
            '';
          };
        };
    };
}
