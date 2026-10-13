"""Generate the three-part plugin API index from the committed JSON Schemas."""

import argparse
import json
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]
DOC = ROOT / "docs/plugins.md"
START = "<!-- plugin-api:start -->"
END = "<!-- plugin-api:end -->"
def fields(schema, prefix=""):
    """List named fields and enum cases without expanding referenced types twice."""
    if "properties" in schema:
        for name, child in schema["properties"].items():
            path = f"{prefix}.{name}" if prefix else name
            yield path
            yield from fields(child, path)
    elif "oneOf" in schema:
        for case in schema["oneOf"]:
            tag = case.get("const") or case.get("properties", {}).get("kind", {}).get("const")
            if tag:
                path = f"{prefix}.{tag}" if prefix else tag
                yield path
                if "properties" in case:
                    yield from fields(case, path)
            else:
                yield from fields(case, prefix)
    elif "enum" in schema:
        for value in schema["enum"]:
            yield f"{prefix}.{value}" if prefix else value
    # Untagged enums (a flattened `CallTarget`, `DecideOut`) list each shape's fields.
    for case in schema.get("anyOf", []):
        yield from fields(case, prefix)
def part(schema_name, type_name, field):
    if schema_name == "manifest":
        if type_name == "TerminalCaps" or type_name == "UiCaps" and field == "keys":
            return "terminal-only"
        if type_name == "DesktopCaps":
            return "desktop-only"
        if type_name == "Capabilities":
            if field == "terminal":
                return "terminal-only"
            if field == "desktop":
                return "desktop-only"
        return "shared"
    if type_name == "KeyDecl" or type_name == "InitOut" and field == "keys":
        return "terminal-only"
    if type_name in {"DesktopOut", "TabDecl", "ActionDecl", "DesktopNotice", "NoticeAction"}:
        return "desktop-only"
    if type_name == "InitOut" and field == "desktop":
        return "desktop-only"
    if type_name == "Slot" and field == "desktop.inspector":
        return "desktop-only"
    if type_name == "CommandOut" and field.startswith("open_inspector"):
        return "desktop-only"
    if type_name == "Widget" and field.startswith("image"):
        return "desktop-only"
    return "shared"
def generate():
    grouped = {name: {} for name in ("shared", "terminal-only", "desktop-only")}
    for schema_name, filename in (("manifest", "plugin.schema.json"), ("ABI", "plugin-abi.schema.json")):
        schema = json.loads((ROOT / "docs" / filename).read_text())
        definitions = schema.get("$defs", {})
        if schema_name == "manifest":
            definitions = {"PluginManifest": schema, **definitions}
        for type_name, definition in definitions.items():
            for field in dict.fromkeys(fields(definition)):
                bucket = grouped[part(schema_name, type_name, field)]
                bucket.setdefault((schema_name, type_name), []).append(field)
    lines = [START, "## API field index", "", "Generated from [manifest](plugin.schema.json) and [ABI](plugin-abi.schema.json) schemas by `scripts/plugin_api_reference.py`. Each entry lists a type's fields and enum cases; referenced types have their own entries. Run `python3 scripts/plugin_api_reference.py --write` after an intentional schema change."]
    for name in grouped:
        lines += ["", f"### {name.capitalize()}", ""]
        for (schema_name, type_name), names in grouped[name].items():
            lines.append(f"- {schema_name} `{type_name}`: {', '.join(f'`{field}`' for field in names)}")
    return "\n".join(lines + [END])
def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true", help="update the generated API index")
    args = parser.parse_args()
    old = DOC.read_text()
    before, marker, rest = old.partition(START)
    if not marker:
        raise SystemExit(f"{DOC}: missing {START}")
    _, marker, after = rest.partition(END)
    if not marker:
        raise SystemExit(f"{DOC}: missing {END}")
    expected = before + generate() + after
    if args.write:
        DOC.write_text(expected)
    elif old != expected:
        raise SystemExit("docs/plugins.md API field index is stale; run python3 scripts/plugin_api_reference.py --write")


if __name__ == "__main__":
    main()
