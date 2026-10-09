#!/usr/bin/env python3
"""Generate lossless Kotlin protocol wrappers and bundle the fixture corpus.

The Rust schema is authoritative. Generated objects keep their complete raw
JsonObject so fields added by newer hosts survive a decode/encode round trip.
The generator supports ordinary JSON Schema $defs/$ref, nullable unions,
arrays and scalar properties. Tagged unions remain JsonElement values; the
hand-written Block decoder is the compatibility boundary for unknown blocks.
"""
import argparse
import json
import pathlib
import re
import sys
from typing import Optional

ROOT = pathlib.Path(__file__).resolve().parents[2]
PACKAGE = "bot.mac.mobile.core.protocol.generated"
OUTPUT = ROOT / "clients/mobile/shared/src/commonMain/kotlin/bot/mac/mobile/core/protocol/generated/SchemaModels.kt"
KOTLIN_KEYWORDS = {
    "as", "break", "class", "continue", "do", "else", "false", "for", "fun", "if", "in", "interface",
    "is", "null", "object", "package", "return", "super", "this", "throw", "true", "try", "typealias",
    "typeof", "val", "var", "when", "while", "by", "catch", "constructor", "delegate", "dynamic", "field",
    "file", "finally", "get", "import", "init", "param", "property", "receiver", "set", "setparam", "where",
}


def kotlin_name(value: str) -> str:
    parts = re.split(r"[^A-Za-z0-9]+", value)
    name = "".join(part[:1].upper() + part[1:] for part in parts if part)
    return name if name and name[0].isalpha() else "SchemaObject" + name


def property_name(value: str) -> str:
    parts = value.split("_")
    name = parts[0] + "".join(part[:1].upper() + part[1:] for part in parts[1:])
    name = name if name and name[0].isalpha() else "field"
    return f"`{name}`" if name in KOTLIN_KEYWORDS else name


def ref_name(ref: str) -> str:
    return kotlin_name(ref.rsplit("/", 1)[-1])


def is_nullable(schema: dict) -> bool:
    if schema.get("nullable") is True:
        return True
    types = schema.get("type")
    if isinstance(types, list) and "null" in types:
        return True
    for key in ("anyOf", "oneOf"):
        choices = schema.get(key)
        if isinstance(choices, list) and any(item.get("type") == "null" for item in choices if isinstance(item, dict)):
            return True
    return False


def non_null_schema(schema: dict) -> dict:
    if isinstance(schema.get("type"), list):
        for kind in schema["type"]:
            if kind != "null":
                return {**schema, "type": kind}
    for key in ("anyOf", "oneOf"):
        choices = schema.get(key)
        if isinstance(choices, list):
            non_null = [item for item in choices if item.get("type") != "null"]
            if len(non_null) == 1:
                return non_null[0]
    return schema


def schema_registry(schemas: list[tuple[pathlib.Path, dict]]) -> dict[str, dict]:
    """Index local definitions. Origin schemas repeat identical definitions per file."""
    registry: dict[str, dict] = {}
    for _, document in schemas:
        definitions = document.get("$defs") or document.get("definitions") or {}
        for name, schema in definitions.items():
            if isinstance(schema, dict):
                registry.setdefault(name, schema)
    return registry


def resolve_ref(schema: dict, registry: dict[str, dict]) -> dict:
    current = schema
    seen: set[str] = set()
    while isinstance(current, dict):
        current = non_null_schema(current)
        if "$ref" not in current:
            return current
        ref = current["$ref"]
        if ref in seen:
            return current
        seen.add(ref)
        target = registry.get(ref.rsplit("/", 1)[-1])
        if not isinstance(target, dict):
            return current
        current = target
    return current


def resolved_ref_name(schema: dict, registry: dict[str, dict]) -> str:
    """Return the concrete object name through nullable alias refs."""
    current = schema
    seen: set[str] = set()
    last = None
    while isinstance(current, dict):
        current = non_null_schema(current)
        ref = current.get("$ref")
        if not ref or ref in seen:
            break
        seen.add(ref)
        last = ref_name(ref)
        target = registry.get(ref.rsplit("/", 1)[-1])
        if not isinstance(target, dict):
            break
        current = target
    return last or ref_name(schema.get("$ref", "SchemaObject"))


def schema_kind(schema: dict, registry: dict[str, dict]) -> str:
    base = non_null_schema(schema)
    if "$ref" in base:
        base = resolve_ref(base, registry)
    if base.get("type") == "array":
        return "array"
    if base.get("type") == "object" or "properties" in base or "additionalProperties" in base:
        return "object"
    if is_object_union(base, registry):
        return "object_union"
    if base.get("type") in ("boolean", "integer", "number", "string") or "enum" in base:
        return "scalar"
    return "union"


def is_object_union(schema: dict, registry: dict[str, dict], seen: Optional[set[str]] = None) -> bool:
    base = non_null_schema(schema)
    if "$ref" in base:
        ref = base["$ref"]
        seen = set() if seen is None else seen
        if ref in seen:
            return False
        target = registry.get(ref.rsplit("/", 1)[-1])
        return isinstance(target, dict) and is_object_union(target, registry, seen | {ref})
    choices = base.get("oneOf") or base.get("anyOf")
    if not isinstance(choices, list) or not choices:
        return False
    for choice in choices:
        if not isinstance(choice, dict) or choice.get("type") == "null":
            continue
        if not is_object_like(choice, registry, seen):
            return False
    return True


def is_object_like(schema: dict, registry: dict[str, dict], seen: Optional[set[str]] = None) -> bool:
    base = non_null_schema(schema)
    if base.get("type") == "object" or "properties" in base or "additionalProperties" in base:
        return True
    if "$ref" in base:
        ref = base["$ref"]
        seen = set() if seen is None else seen
        if ref in seen:
            return False
        target = registry.get(ref.rsplit("/", 1)[-1])
        return isinstance(target, dict) and is_object_like(target, registry, seen | {ref})
    return is_object_union(base, registry, seen)


def kotlin_type(schema: dict, registry: dict[str, dict], nullable: bool = True) -> str:
    base = non_null_schema(schema)
    kind = schema_kind(base, registry)
    if kind in {"object", "object_union"}:
        resolved = resolve_ref(base, registry)
        result = resolved_ref_name(base, registry) if "$ref" in base else "JsonObject"
        if kind == "object_union" and "$ref" not in base:
            result = "JsonElement"
        if "$ref" in base and not (kind in {"object", "object_union"}):
            result = "JsonElement"
    elif kind == "array":
        result = f"List<{kotlin_type(base.get('items', {}), registry, nullable=False)}>"
    elif kind == "scalar":
        resolved = resolve_ref(base, registry)
        scalar_type = resolved.get("type")
        if scalar_type == "boolean":
            result = "Boolean"
        elif scalar_type == "integer":
            result = "Long"
        elif scalar_type == "number":
            result = "Double"
        else:
            result = "String"
    else:
        result = "JsonElement"
    return result + ("?" if nullable and is_nullable(schema) else "")


def scalar_expression(key: str, schema: dict, required: bool, registry: dict[str, dict]) -> str:
    nullable = not required or is_nullable(schema)
    base = non_null_schema(schema)
    quoted = json.dumps(key)
    if "$ref" in base:
        target = resolve_ref(base, registry)
        kind = schema_kind(target, registry)
        if kind in {"object", "object_union"}:
            name = resolved_ref_name(base, registry)
            return f"raw[{quoted}]?.jsonObject?.let(::{name})" if nullable else f"{name}(raw.obj({quoted}))"
        if kind == "scalar":
            base = target
        else:
            return f"raw[{quoted}]" if nullable else f"raw[{quoted}] ?: JsonNull"
    if base.get("type") == "object" or "properties" in base or "additionalProperties" in base:
        return f"raw.objectOrNull({quoted})" if nullable else f"raw.obj({quoted})"
    if base.get("type") == "array":
        item = non_null_schema(base.get("items", {}))
        expression = array_expression(f"raw.arr({quoted})", item, registry)
        return expression if required else f"if (raw[{quoted}] == null) null else {expression}"
    if base.get("type") == "boolean":
        return f"raw.boolOrNull({quoted})" if nullable else f"raw.boolean({quoted})"
    if base.get("type") == "integer":
        return f"raw.longOrNull({quoted})" if nullable else f"raw.long({quoted})"
    if base.get("type") == "number":
        return f"raw.doubleOrNull({quoted})" if nullable else f"raw.double({quoted})"
    if base.get("type") == "string" or "enum" in base:
        return f"raw.stringOrNull({quoted})" if nullable else f"raw.str({quoted})"
    return f"raw[{quoted}]" if nullable else f"raw[{quoted}] ?: JsonNull"


def array_expression(source: str, item: dict, registry: dict[str, dict]) -> str:
    """Read an array while keeping nested arrays and raw union values typed correctly."""
    item = non_null_schema(item)
    kind = schema_kind(item, registry)
    target = resolve_ref(item, registry) if "$ref" in item else item
    if kind == "array":
        nested = array_expression("((element as? JsonArray) ?: JsonArray(emptyList()))", target.get("items", {}), registry)
        return f"{source}.map {{ element -> {nested} }}"
    if kind == "scalar" and (target.get("type") == "string" or "enum" in target):
        return f"{source}.mapNotNull {{ (it as? JsonPrimitive)?.contentOrNull }}"
    if kind == "scalar" and target.get("type") == "integer":
        return f"{source}.mapNotNull {{ (it as? JsonPrimitive)?.longOrNull }}"
    if kind == "scalar" and target.get("type") == "number":
        return f"{source}.mapNotNull {{ (it as? JsonPrimitive)?.doubleOrNull }}"
    if kind == "scalar" and target.get("type") == "boolean":
        return f"{source}.mapNotNull {{ (it as? JsonPrimitive)?.booleanOrNull }}"
    if kind in {"object", "object_union"} and "$ref" in item:
        return f"{source}.mapNotNull {{ (it as? JsonObject)?.let(::{resolved_ref_name(item, registry)}) }}"
    return f"{source}.toList()"


def collect_objects(schemas: list[tuple[pathlib.Path, dict]], registry: Optional[dict[str, dict]] = None) -> dict[str, dict]:
    registry = registry or schema_registry(schemas)
    objects: dict[str, dict] = {}
    for _, document in schemas:
        definitions = document.get("$defs") or document.get("definitions") or {}
        for name, schema in definitions.items():
            if isinstance(schema, dict) and (
                schema.get("type") == "object" or "properties" in schema or "additionalProperties" in schema
                or is_object_union(schema, registry)
            ):
                objects.setdefault(kotlin_name(name), schema)
        if document.get("title") and (
            document.get("type") == "object" or "properties" in document or "additionalProperties" in document
            or is_object_union(document, registry)
        ):
            objects.setdefault(kotlin_name(document["title"]), document)
    return objects


def generate_models(objects: dict[str, dict], registry: dict[str, dict]) -> str:
    lines = [
        "// Generated by protocol/kotlin/generate.py; do not edit.",
        f"package {PACKAGE}",
        "",
        "import bot.mac.mobile.core.protocol.RawModelSerializer",
        "import bot.mac.mobile.core.protocol.RawProtocolModel",
        "import kotlinx.serialization.Serializable",
        "import kotlinx.serialization.json.*",
        "",
        "private fun JsonObject.str(key: String): String = (this[key] as? JsonPrimitive)?.contentOrNull.orEmpty()",
        "private fun JsonObject.stringOrNull(key: String): String? = (this[key] as? JsonPrimitive)?.contentOrNull",
        "private fun JsonObject.long(key: String): Long = (this[key] as? JsonPrimitive)?.longOrNull ?: 0L",
        "private fun JsonObject.longOrNull(key: String): Long? = (this[key] as? JsonPrimitive)?.longOrNull",
        "private fun JsonObject.double(key: String): Double = (this[key] as? JsonPrimitive)?.doubleOrNull ?: 0.0",
        "private fun JsonObject.doubleOrNull(key: String): Double? = (this[key] as? JsonPrimitive)?.doubleOrNull",
        "private fun JsonObject.boolean(key: String): Boolean = (this[key] as? JsonPrimitive)?.booleanOrNull ?: false",
        "private fun JsonObject.boolOrNull(key: String): Boolean? = (this[key] as? JsonPrimitive)?.booleanOrNull",
        "private fun JsonObject.obj(key: String): JsonObject = this[key] as? JsonObject ?: buildJsonObject {}",
        "private fun JsonObject.objectOrNull(key: String): JsonObject? = this[key] as? JsonObject",
        "private fun JsonObject.arr(key: String): JsonArray = this[key] as? JsonArray ?: JsonArray(emptyList())",
        "private fun JsonObject.objects(key: String): List<JsonObject> = arr(key).mapNotNull { it as? JsonObject }",
        "",
    ]
    for name, schema in objects.items():
        lines.append(f"@Serializable(with = {name}Serializer::class)")
        lines.append(f"data class {name}(override val raw: JsonObject) : RawProtocolModel {{")
        properties = schema.get("properties", {})
        # HeatmapResult is an untagged object union. Keep the historical raw
        # accessors for callers that receive a wrapped calendar/weekhour value;
        # the serializer still preserves the complete raw object for the direct
        # wire shapes used by the result fixtures.
        if name == "HeatmapResult" and not properties:
            lines.append("    val calendar: CalendarHeatmap? get() = raw[\"calendar\"]?.jsonObject?.let(::CalendarHeatmap)")
            lines.append("    val weekhour: WeekhourHeatmap? get() = raw[\"weekhour\"]?.jsonObject?.let(::WeekhourHeatmap)")
        required = set(schema.get("required", []))
        for key, property in properties.items():
            if not isinstance(property, dict):
                continue
            kotlin = kotlin_type(property, registry, nullable=False)
            if key not in required or is_nullable(property):
                kotlin += "?"
            lines.append(f"    val {property_name(key)}: {kotlin} get() = {scalar_expression(key, property, key in required, registry)}")
        lines.extend([
            "}",
            f"object {name}Serializer : RawModelSerializer<{name}>(::{name})",
            "",
        ])
    return "\n".join(lines) + "\n"


def load_fixtures(fixtures: list[pathlib.Path]) -> list[dict]:
    corpus = []
    for path in fixtures:
        content = path.read_text()
        values = [json.loads(line) for line in content.splitlines() if line.strip()] if path.suffix == ".jsonl" else [json.loads(content)]
        for index, value in enumerate(values):
            corpus.append({"source": str(path.relative_to(ROOT)), "line": index + 1, "value": value})
    return corpus


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true", help="fail when schemas or fixtures are unavailable")
    parser.add_argument("--no-generate", action="store_true", help="only build the fixture inventory")
    args = parser.parse_args()

    schemas = sorted((ROOT / "protocol/schema").rglob("*.json"))
    fixtures = sorted(path for path in (ROOT / "protocol/fixtures").rglob("*") if path.suffix in (".json", ".jsonl"))
    missing = []
    if not schemas:
        missing.append("authoritative schema")
    if not fixtures:
        missing.append("protocol fixtures")

    loaded = []
    for path in schemas:
        try:
            loaded.append((path, json.loads(path.read_text())))
        except (OSError, json.JSONDecodeError) as error:
            print(f"Unable to read schema {path}: {error}", file=sys.stderr)
            return 1

    objects = collect_objects(loaded)
    registry = schema_registry(loaded)
    if loaded and not args.no_generate:
        OUTPUT.parent.mkdir(parents=True, exist_ok=True)
        OUTPUT.write_text(generate_models(objects, registry))

    corpus = load_fixtures(fixtures) if fixtures else []
    output = ROOT / "clients/mobile/shared/src/androidUnitTest/resources"
    output.mkdir(parents=True, exist_ok=True)
    (output / "protocol-fixtures.json").write_text(json.dumps(corpus, ensure_ascii=False, indent=2) + "\n")
    inventory = {
        "schemas": [str(path.relative_to(ROOT)) for path in schemas],
        "fixtures": len(corpus),
        "generated_models": str(OUTPUT.relative_to(ROOT)) if loaded and not args.no_generate else None,
    }
    (ROOT / "protocol/kotlin/schema-inventory.json").write_text(json.dumps(inventory, ensure_ascii=False, indent=2) + "\n")

    if missing:
        print("Waiting for server-mac: missing " + " and ".join(missing) + ".", file=sys.stderr)
        return 1 if args.check else 0
    print(f"Bundled {len(corpus)} fixture values from {len(fixtures)} files; generated {len(objects)} schema objects.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
