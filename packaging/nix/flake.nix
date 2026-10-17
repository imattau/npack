{
  description = "Consume npack nix-recipe artifacts as ordinary Nix packages";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-25.05";

  outputs = { self, nixpkgs }: {
    # lib.<system> = { fetchRecipe, fromPins }
    lib = nixpkgs.lib.genAttrs [ "x86_64-linux" "aarch64-linux" ] (system:
      import ./lib.nix { pkgs = nixpkgs.legacyPackages.${system}; });

    # pkgs.npackRecipes.{ fetchRecipe, fromPins }
    overlays.default = final: _prev: {
      npackRecipes = import ./lib.nix { pkgs = final; };
    };
  };
}
