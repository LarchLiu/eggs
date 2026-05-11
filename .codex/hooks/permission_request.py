#!/usr/bin/env python3
from common import done, read_payload, send_hook, shorten
from post_tool_use import command_from_payload, detail_for_tool, label_for_tool


def reason_from_payload(payload):
    for key in ("reason", "justification", "message", "description"):
        value = payload.get(key)
        if isinstance(value, str) and value.strip():
            return shorten(value, 90)
    return ""


def format_permission_message(payload):
    tool = payload.get("tool_name") or payload.get("tool") or payload.get("name") or "unknown"
    command = command_from_payload(payload)
    label = label_for_tool(tool, command)
    detail = detail_for_tool(tool, command)
    reason = reason_from_payload(payload)
    message = f"Permission: {label}: {detail}"
    if reason:
        message = f"{message} | {reason}"
    return message


def main():
    send_hook(format_permission_message(read_payload()))
    done()


if __name__ == "__main__":
    main()
