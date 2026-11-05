# Only narrows the reviewed Pi connection; it never admits host services.
{ lib }:
let
  descriptor = builtins.fromJSON (builtins.readFile ../schema/system-v1.json);
  supported = descriptor.agent.tools.items.values;
in
{
  inherit supported;
  select =
    tools:
    if
      !builtins.isList tools
      || !lib.all (tool: builtins.isString tool && builtins.elem tool supported) tools
      || builtins.length tools != builtins.length (lib.unique tools)
    then
      throw "tentaflake: Pi Durable tools must be a unique subset of the reviewed stable tool IDs."
    else
      lib.filter (tool: builtins.elem tool tools) supported;
}
