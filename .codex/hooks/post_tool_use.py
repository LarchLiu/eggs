#!/usr/bin/env python3
import shlex

from common import done, read_payload, send_hook, shorten


READ_COMMANDS = {
    "cat",
    "head",
    "tail",
    "less",
    "more",
    "sed",
    "nl",
    "wc",
    "ls",
    "find",
    "pwd",
    "tree",
}
SEARCH_COMMANDS = {"rg", "grep", "ag", "ack"}
WRITE_COMMANDS = {
    "apply_patch",
    "tee",
    "touch",
    "mkdir",
    "cp",
    "mv",
    "install",
}


def first_string(value):
    if isinstance(value, str):
        return value
    if isinstance(value, dict):
        for key in ("cmd", "command", "query", "url", "path", "file_path"):
            item = value.get(key)
            if isinstance(item, str) and item.strip():
                return item
    return ""


def tool_input(payload):
    for key in ("tool_input", "input", "arguments", "args", "parameters"):
        value = payload.get(key)
        if value:
            return value
    return {}


def command_from_payload(payload):
    data = tool_input(payload)
    if isinstance(data, dict):
        for key in ("cmd", "command", "script", "query", "url"):
            value = data.get(key)
            if isinstance(value, str) and value.strip():
                return value
    return first_string(data)


def command_head(command):
    try:
        parts = shlex.split(command)
    except ValueError:
        parts = command.split()
    if not parts:
        return ""
    head = parts[0].split("/")[-1]
    if head in {"env", "command", "xargs"} and len(parts) > 1:
        head = parts[1].split("/")[-1]
    return head


def label_for_tool(tool, command):
    normalized = (tool or "").strip()
    lower = normalized.lower()
    cmd = command_head(command).lower()

    if lower in {"apply_patch", "edit", "write", "multiedit"}:
        return "Write file"
    if lower in {"read", "glob", "list", "ls"}:
        return "Read file"
    if lower in {"grep", "search"}:
        return "Search"
    if "web_search" in lower or lower.endswith("websearch"):
        return "Web search"
    if "web_fetch" in lower or lower.endswith("webfetch"):
        return "Web fetch"
    if lower.startswith("mcp__"):
        if "search" in lower:
            return "MCP search"
        if "fetch" in lower or "read" in lower:
            return "MCP fetch"
        return "MCP tool"

    if cmd in SEARCH_COMMANDS:
        return "Search"
    if cmd in READ_COMMANDS:
        return "Read file"
    if cmd in WRITE_COMMANDS:
        return "Write file"
    if cmd in {"curl", "wget", "http", "https"}:
        return "Web fetch"
    if normalized in {"Bash", "exec_command"} or cmd:
        return "Run command"
    return "Tool done"


def detail_for_tool(tool, command):
    if command:
        return shorten(command, 80)
    if tool:
        return tool
    return "tool"


def format_hook_message(payload):
    tool = payload.get("tool_name") or payload.get("tool") or payload.get("name") or "tool"
    command = command_from_payload(payload)
    label = label_for_tool(tool, command)
    detail = detail_for_tool(tool, command)
    return f"{label}: {detail}"


def main():
    send_hook(format_hook_message(read_payload()))
    done()


if __name__ == "__main__":
    main()
