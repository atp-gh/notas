{
  description = "Notas — a fast, local-first note-taking app for Linux, written in Rust";

  inputs = {
    # Unstable is intentional: gtk4-sys/libadwaita-sys enforce recent minimum
    # GTK versions at build time (v4_22 / v1_9 features), so the packaged GTK
    # must be recent. Pin a specific nixpkgs revision for reproducibility.
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
  };

  outputs =
    {
      self,
      nixpkgs,
    }:
    let
      systems = [ "x86_64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs systems;

      notasFor = pkgs: pkgs.callPackage ./nix/notas.nix { };
    in
    {
      packages = forAllSystems (system: {
        default = notasFor nixpkgs.legacyPackages.${system};
      });

      apps = forAllSystems (system: {
        default = {
          type = "app";
          program = "${self.packages.${system}.default}/bin/notas";
        };
      });

      devShells = forAllSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          notas = notasFor pkgs;
        in
        {
          default = pkgs.mkShell {
            name = "notas-dev";
            inputsFrom = [ notas ];
            packages = with pkgs; [
              cargo
              clippy
              rust-analyzer
              rustc
              rustfmt
            ];
          };
        });

      # Allow `pkgs.notas` via an overlay on NixOS.
      overlays.default = final: prev: {
        notas = notasFor final;
      };
    };
}
