{ pkgs ? import <nixpkgs> { } }:

with pkgs;

mkShell {
  buildInputs = [
    # tools
    gcc
    just
    qemu-user
    rustup
  ];
}
