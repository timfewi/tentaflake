# ────────────────────────────────────────────────────────────
# editor.nix — Neovim via nvf (optional)
#
# A batteries-included Neovim (LSP, treesitter, telescope, git, completion)
# built from the nvf flake (https://github.com/NotAShelf/nvf).
#
# This module is deliberately NOT part of modules/default.nix: it needs the
# `nvf` flake input, which external consumers of nixosModules.default may not
# have. The template wires it into its own hosts and exports it as
# nixosModules.editor; consumers opt in by adding the nvf input and importing
# this module. Toggle with tentaflake.editor.nvf.enable.
# ────────────────────────────────────────────────────────────
{
  config,
  lib,
  inputs,
  ...
}:
let
  cfg = config.tentaflake.editor;
in
{
  imports = [ inputs.nvf.nixosModules.default ];

  options.tentaflake.editor.nvf.enable =
    lib.mkEnableOption "nvf-based Neovim (LSP, treesitter, telescope, git, completion)";

  config = lib.mkIf cfg.nvf.enable {
    # Make nvim the editor. mkOverride 900 beats NixOS's default EDITOR=nano
    # (priority 1000) while staying below a normal user override.
    environment.variables.EDITOR = lib.mkOverride 900 "nvim";

    programs.nvf = {
      enable = true;
      settings.vim = {
        viAlias = true;
        vimAlias = true;

        # ── Look & feel ──
        theme = {
          enable = true;
          name = "catppuccin";
          style = "mocha";
        };
        statusline.lualine.enable = true;
        visuals = {
          nvim-web-devicons.enable = true;
          indent-blankline.enable = true;
        };
        ui = {
          noice.enable = true;
          colorizer.enable = true;
        };

        options = {
          tabstop = 2;
          shiftwidth = 2;
          expandtab = true;
          number = true;
          relativenumber = true;
        };

        # ── LSP + diagnostics ──
        lsp = {
          enable = true;
          formatOnSave = true;
          lspkind.enable = true;
          trouble.enable = true;
        };

        # ── Languages ──
        # Lean set for a headless agent host: the languages you actually edit
        # here (Nix config, shell, Lua, docs/data). No heavy dev stack
        # (TypeScript/.NET/Python/…) — a fork can add languages.<lang>.enable.
        languages = {
          enableTreesitter = true;
          enableFormat = true;
          nix.enable = true;
          bash.enable = true;
          lua.enable = true;
          markdown.enable = true;
          yaml.enable = true;
        };

        # ── Completion / navigation / git / editing ──
        autocomplete.blink-cmp.enable = true;
        telescope.enable = true;
        filetree.neo-tree.enable = true;
        binds.whichKey.enable = true;
        git.gitsigns.enable = true;
        autopairs.nvim-autopairs.enable = true;
        comments.comment-nvim.enable = true;
        treesitter.context.enable = true;
      };
    };
  };
}
