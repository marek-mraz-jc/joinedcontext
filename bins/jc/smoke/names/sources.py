"""Tool names from Rust source text, read the same way from prod's pinned commits (git show, in
fetch_prod_tools.py) and from this clone (the jc the suite compares): the gateway's TOOLS and
ANONYMOUS_TOOLS (context-gateway src/mcp/tools.rs) and the broker's TOOLS (antares-mcp
src/tools.rs). Standard library only."""

from __future__ import annotations

import re
from pathlib import Path

GATEWAY_TOOLS = "crates/context-gateway/src/mcp/tools.rs"
BROKER_TOOLS = "crates/antares-mcp/src/tools.rs"
# Where this clone keeps them (jc vendors the gateway; Antares is a workspace member).
JC_GATEWAY_TOOLS = "vendor/joinedcontext-platform/" + GATEWAY_TOOLS
JC_BROKER_TOOLS = "crates/antares/" + BROKER_TOOLS

_ARRAY = r"const {name}\s*:\s*(?:&\[[^\]]*\]|\[[^\]]*\])\s*=\s*&?\["
# One entry of a tool table: `ctor(\n "name"` or `Struct {\n name: "name"`, at the table's indent.
_ENTRY = re.compile(r'^    (?:\w+\(\s*"([a-z_][a-z0-9_]*)"|\w+\s*\{\s*name:\s*"([a-z_][a-z0-9_]*)")', re.MULTILINE)
_STRING = re.compile(r'^\s*"([a-z_][a-z0-9_]*)",?\s*$', re.MULTILINE)


def _block(text: str, name: str) -> str | None:
    m = re.search(_ARRAY.format(name=name), text)
    if not m:
        return None
    end = text.find("\n];", m.end())
    return text[m.end():end if end >= 0 else len(text)]


def tool_table(text: str, name: str = "TOOLS") -> list[str]:
    """The tool names of a `const <name>: ... = [ ... ];` table of tool definitions."""
    block = _block(text, name)
    if block is None:
        return []
    return [a or b for a, b in _ENTRY.findall(block)]


def string_list(text: str, name: str) -> list[str]:
    """The strings of a `const <name>: &[&str] = &[ "a", "b", ];` list, one per line."""
    block = _block(text, name)
    return _STRING.findall(block) if block is not None else []


def source_tools(gateway_text: str, broker_text: str) -> dict:
    """{"gateway": [...], "anonymous": [...], "broker": [...]}, each sorted."""
    return {"gateway": sorted(set(tool_table(gateway_text))),
            "anonymous": sorted(set(string_list(gateway_text, "ANONYMOUS_TOOLS"))),
            "broker": sorted(set(tool_table(broker_text)))}


def jc_source_tools(repo: Path) -> dict:
    """The same names from this clone's sources."""
    return source_tools((repo / JC_GATEWAY_TOOLS).read_text(), (repo / JC_BROKER_TOOLS).read_text())
