"""Render support facts from the catalog; --check rejects documentation drift."""
import argparse
import json
from pathlib import Path

START = "<!-- runtime-catalog:start -->"
END = "<!-- runtime-catalog:end -->"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parent.parent)
    args = parser.parse_args()
    catalog = json.loads((args.root / "adapters/catalog.json").read_text())
    if catalog["schemaVersion"] != 1:
        parser.error("unsupported catalog version")
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
    path = args.root / "docs/agent-adapters.md"
    original = path.read_text()
    before, rest = original.split(START, 1)
    _, after = rest.split(END, 1)
    updated = before + START + "\n\n" + "\n".join(rows) + "\n\n" + END + after
    if args.check:
        if updated != original:
            parser.exit(1, "Runtime support facts drifted; run python3 scripts/runtime-catalog-docs.py\n")
    else:
        path.write_text(updated)


if __name__ == "__main__":
    main()
