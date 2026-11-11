{
  description = "Rust development environment";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    { nixpkgs, rust-overlay, ... }:
    let
      forEachSystem = nixpkgs.lib.genAttrs nixpkgs.lib.systems.flakeExposed;

      # Use the project's own toolchain file when it has one.
      toolchainFile = builtins.head (
        builtins.filter builtins.pathExists [
          ./rust-toolchain.toml
          ./rust-toolchain
        ]
        ++ [ null ]
      );
    in
    {
      devShells = forEachSystem (
        system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [ rust-overlay.overlays.default ];
          };
          toolchain =
            if toolchainFile != null then
              pkgs.rust-bin.fromRustupToolchainFile toolchainFile
            else
              pkgs.rust-bin.stable.latest.default.override {
                extensions = [
                  "rust-src"
                  "rustfmt"
                ];
              };
        in
        {
          default = pkgs.mkShell {
            packages = with pkgs; [
              toolchain
              openssl
              pkg-config
              cargo-deny
              cargo-edit
              cargo-watch
              rust-analyzer
            ];

            env = {
              # Required by rust-analyzer
              RUST_SRC_PATH = "${toolchain}/lib/rustlib/src/rust/library";
            };
          };
        }
      );
    };
}
