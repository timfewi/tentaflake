"""Generate the CLI catalog and support table from adapter-owned Nix presets."""
import argparse
import json
from pathlib import Path
import subprocess

START = "<!-- runtime-catalog:start -->"
END = "<!-- runtime-catalog:end -->"


def catalog_from_adapters(root):
    # Pure facts only: no nixpkgs, derivations, daemon, credentials or downloads.
    result = subprocess.run(
        [
            "nix-instantiate", "--eval", "--strict", "--json", "--readonly-mode",
            str(root / "lib/runtimeCatalog.nix"),
        ],
        capture_output=True, text=True, check=True, timeout=30,
    )
    catalog = json.loads(result.stdout)
    if catalog.get("schemaVersion") != 1 or not isinstance(catalog.get("presets"), dict):
        raise ValueError("unsupported catalog version or missing presets")
    return catalog


def rendered_outputs(root, catalog):
    rows = [
        "| Preset | Version | Declaration | Fixture | Vendor acceptance |",
        "| --- | --- | --- | --- | --- |",
    ]
    for name, preset in sorted(catalog["presets"].items()):
        identity, evidence = preset["identity"], preset["evidence"]
        rows.append(
            f"| {name} | {identity['version']} | {identity['status']} | "
            f"{evidence['fixture']} | {evidence['vendorAcceptance']} |"
        )
    guide = root / "docs/agent-adapters.md"
    original = guide.read_text()
    if original.count(START) != 1 or original.count(END) != 1:
        raise ValueError("adapter guide requires exactly one support-table marker pair")
    before, rest = original.split(START, 1)
    _, after = rest.split(END, 1)
    return {
        root / "adapters/catalog.json": json.dumps(catalog, indent=2, ensure_ascii=False) + "\n",
        guide: before + START + "\n\n" + "\n".join(rows) + "\n\n" + END + after,
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parent.parent)
    args = parser.parse_args()
    try:
        outputs = rendered_outputs(args.root, catalog_from_adapters(args.root))
    except (OSError, ValueError, subprocess.SubprocessError):
        parser.exit(1, "Cannot generate adapter facts; check the adapter presets and guide markers.\n")
    drift = [path for path, content in outputs.items()
             if not path.exists() or path.read_text() != content]
    if args.check:
        if drift:
            parser.exit(1, "Generated adapter facts drifted; run python3 scripts/runtime-catalog-docs.py\n")
    else:
        for path, content in outputs.items():
            path.write_text(content)


if __name__ == "__main__":
    main()
