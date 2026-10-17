# Generic npack -> Nix integration spike.
# Pure at eval time: the pinned sha256 + URLs come from `npack resolve`.
{ pkgs ? import <nixpkgs> {} }:
let
  fetchRecipe = { name, sha256, urls }:
    let
      npk = pkgs.fetchurl { inherit urls sha256; name = "${name}.npk"; };
      unpacked = pkgs.runCommand "${name}-recipe" { nativeBuildInputs = [ pkgs.zstd pkgs.gnutar ]; } ''
        mkdir $out && tar --zstd -xf ${npk} -C $out
      '';
    in pkgs.callPackage "${unpacked}/default.nix" {};
in {
  hello = fetchRecipe {
    name = "hello-recipe";
    sha256 = "d001b6b908ffcaf181026de74538fd1404b0a66a3625a149eea0b3c8fcb4bc4a";
    urls = [ "http://10.0.2.2:8099/hello-recipe-1.0.0.npk" ];
  };
}
