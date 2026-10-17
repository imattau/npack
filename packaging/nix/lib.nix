# Generic npack -> Nix integration.
#
# Evaluation is pure: every recipe is pinned by sha256 and mirror URLs taken
# from `npack resolve` (see npack-nix-pin), so Nix never talks to a relay.
# Nix's own fetcher re-verifies the sha256. Recipes are Nix code evaluated by
# this machine: only pin publishers you trust. Evaluating one uses
# import-from-derivation (the .npk is unpacked by a derivation first).
{ pkgs }:
rec {
  # Fetches a recipe .npk and callPackage's its default.nix.
  fetchRecipe =
    { name
    , sha256
    , urls
    , kind ? "nix-recipe"
    , args ? { }
    , callPackage ? pkgs.callPackage
    , ...
    }:
    assert pkgs.lib.assertMsg (kind == "nix-recipe")
      "npack artifact ${name} has kind '${kind}', expected 'nix-recipe'";
    let
      npk = pkgs.fetchurl { inherit urls sha256; name = "${name}.npk"; };
      unpacked = pkgs.runCommand "${name}-recipe"
        { nativeBuildInputs = [ pkgs.zstd pkgs.gnutar ]; }
        ''
          mkdir $out
          tar --zstd -xf ${npk} -C $out --no-same-owner
        '';
    in
    callPackage "${unpacked}/default.nix" args;

  # Reads a pins file written by npack-nix-pin: { attr = { name, sha256, urls, ... }; }
  # Recipes are built in one scope, so a recipe can take another pinned recipe
  # (e.g. a declared dependency) as a plain function argument by its attr name.
  # With trustedPublishers (hex pubkeys), evaluation fails if any pin comes from
  # another publisher, so a hand-edited or merged pins file cannot add one silently.
  fromPinsWith = { pinsFile, trustedPublishers ? null }:
    let
      pins = builtins.fromJSON (builtins.readFile pinsFile);
      checked = builtins.mapAttrs
        (attr: pin:
          assert trustedPublishers == null || pkgs.lib.assertMsg
            (builtins.elem pin.publisher trustedPublishers)
            "npack pin '${attr}' is from publisher ${pin.publisher}, which is not in trustedPublishers";
          pin)
        pins;
      scope = pkgs.lib.makeScope pkgs.newScope (self:
        builtins.mapAttrs (_: pin: fetchRecipe (pin // { callPackage = self.callPackage; })) checked);
    in
    pkgs.lib.getAttrs (builtins.attrNames pins) scope;

  fromPins = pinsFile: fromPinsWith { inherit pinsFile; };
}
