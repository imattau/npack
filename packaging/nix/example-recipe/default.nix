# npack recipe: callPackage-style derivation, evaluated and built by Nix.
{ stdenv, fetchurl }:
stdenv.mkDerivation {
  pname = "hello-recipe";
  version = "2.12.1";
  src = fetchurl {
    url = "https://ftp.gnu.org/gnu/hello/hello-2.12.1.tar.gz";
    hash = "sha256-jZkUKv2SV28wsM18tCqNxoCZmLxdYH2Idh9RLibH2yA=";
  };
  doCheck = false;
}
