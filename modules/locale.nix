{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.tentaflake;
  setVtFont = !cfg.modernConsole.enable && cfg.consoleFont != null;
in
lib.mkIf cfg.locale.enable {
  time.timeZone = cfg.timeZone;
  i18n.defaultLocale = cfg.defaultLocale;
  console.keyMap = cfg.consoleKeyMap;

  # ── Physical console ──
  #
  # The kernel's built-in VT font has 256 CP437-era glyphs and a Linux VT can
  # map at most 512, so on a bare console btop's graphs and the braille logo in
  # `tentaflake-status` come out as garbage boxes. No console font fixes that —
  # the 512-glyph ceiling is the VT itself.
  #
  # kmscon replaces the VT with a KMS/DRI terminal that renders real TTF fonts
  # via pango: full Unicode, proper antialiasing, a font size that fits the
  # panel. Cascadia Mono covers braille (U+2800–28FF) and box drawing, which
  # most "modern" monospace fonts (JetBrains Mono, Hack, Fira) do not.
  #
  # Turn it off with `tentaflake.modernConsole.enable = false` — the Terminus
  # VT font below is the fallback, still far better than the kernel default.
  # Only in the fallback case: with kmscon running, its TTF font is what you
  # see, so a setfont at boot would reconfigure fbcon for nothing.
  console.packages = lib.mkIf setVtFont [ pkgs.terminus_font ];
  console.font = lib.mkIf setVtFont (
    if lib.hasPrefix "/" cfg.consoleFont then
      cfg.consoleFont
    else
      "${pkgs.terminus_font}/share/consolefonts/${cfg.consoleFont}.psf.gz"
  );

  services.kmscon = lib.mkIf cfg.modernConsole.enable {
    enable = true;
    # mkDefault throughout so a consumer flake can swap font or size in one line.
    config = {
      font-name = lib.mkDefault "Cascadia Mono";
      font-size = lib.mkDefault cfg.modernConsole.fontSize;
    };
    # kmscon replaces getty, so console.keyMap no longer reaches the physical
    # console — feed the same layout in through xkb instead.
    useXkbConfig = true;
  };
  fonts = lib.mkIf cfg.modernConsole.enable {
    fontconfig.enable = true; # asserted by the kmscon module when font-name is set
    packages = [ pkgs.cascadia-code ];
  };
  services.xserver.xkb.layout = lib.mkIf cfg.modernConsole.enable (lib.mkDefault cfg.consoleKeyMap);
}
