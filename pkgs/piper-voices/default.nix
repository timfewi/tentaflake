{
  lib,
  stdenvNoCC,
  fetchurl,
}:

let
  voices = {
    en_US-lessac-medium = rec {
      model = fetchurl {
        name = "en_US-lessac-medium.onnx";
        url = "https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/en/en_US/lessac/medium/en_US-lessac-medium.onnx";
        hash = "sha256-Xv4J5pkCGHgnr2RuGm6dJp3udp+Yd9F7FrG0buqvAZ8=";
      };
      config = fetchurl {
        name = "en_US-lessac-medium.onnx.json";
        url = "https://huggingface.co/rhasspy/piper-voices/resolve/v1.0.0/en/en_US/lessac/medium/en_US-lessac-medium.onnx.json";
        hash = "sha256-7+GcQXvtBV8taZCCSMa6ZQ+hNbyGiw5quz2hgdq2kKA=";
      };
    };
  };
in
stdenvNoCC.mkDerivation {
  pname = "piper-voices";
  version = "1.0.0";
  phases = [ "installPhase" ];
  installPhase = ''
    mkdir -p $out/share/piper-voices
    ${lib.concatStringsSep "\n" (
      lib.mapAttrsToList (name: v: ''
        ln -s ${v.model} $out/share/piper-voices/${name}.onnx
        ln -s ${v.config} $out/share/piper-voices/${name}.onnx.json
      '') voices
    )}
  '';
  meta = {
    description = "MIT-licensed Piper TTS voice models for the optional integration";
    license = lib.licenses.mit;
    homepage = "https://github.com/rhasspy/piper";
    platforms = lib.platforms.all;
  };
}
