#!/usr/bin/env python3
"""Generate design/prototype/tokens.css from design/tokens.json.

Token `bg.sidebar` becomes CSS var `--bg-sidebar`. Themes are selected with
<html data-theme="light|dark" data-material="opaque|translucent">.
"""
import json
import pathlib

root = pathlib.Path(__file__).resolve().parent.parent
tokens = json.loads((root / "tokens.json").read_text())


def var(name):
    return "--" + name.replace(".", "-")


def block(selector, mapping):
    lines = [f"{selector} {{"]
    for k, v in mapping.items():
        if k.startswith("$"):
            continue
        lines.append(f"  {var(k)}: {v};")
    lines.append("}")
    return "\n".join(lines)


out = ["/* GENERATED from design/tokens.json — do not edit by hand. */"]

static = {}
for k, v in tokens["space"].items():
    static[f"space.{k}"] = f"{v}px"
for k, v in tokens["radius"].items():
    static[f"radius.{k}"] = f"{v}px"
for k, v in tokens["elevation"].items():
    static[f"elevation.{k}"] = v
for k, v in tokens["font"].items():
    if not k.startswith("$"):
        static[f"font.{k}"] = v
for k, v in tokens["motion"].items():
    if isinstance(v, int):
        static[k] = f"{v}ms"
    elif k.startswith("ease"):
        static[k] = v
for i, c in enumerate(tokens["avatar"]["colors"]):
    static[f"avatar.{i}"] = c
static["avatar.glasses"] = tokens["avatar"]["glasses"]
t = tokens["translucent"]
for k in ("blur.sidebar", "blur.panel", "blur.toolbar", "blur.popover"):
    static[k] = f"{t[k]}px"
static["saturate"] = str(t["saturate"])
for platform in ("desktop", "mobile"):
    for name, (size, lh, weight, ls) in tokens["type"][platform].items():
        if name.startswith("$"):
            continue
        p = "d" if platform == "desktop" else "m"
        static[f"type.{p}.{name}.size"] = f"{size}px"
        static[f"type.{p}.{name}.lh"] = f"{lh}px"
        static[f"type.{p}.{name}.weight"] = str(weight)
out.append(block(":root", static))

out.append(block(":root, [data-theme='light']", tokens["themes"]["light"]))
out.append(block("[data-theme='dark']", tokens["themes"]["dark"]))
out.append(block("[data-theme='light'][data-material='translucent']", t["light"]))
out.append(block("[data-theme='dark'][data-material='translucent']", t["dark"]))

(root / "prototype" / "tokens.css").write_text("\n\n".join(out) + "\n")
print("wrote prototype/tokens.css")
