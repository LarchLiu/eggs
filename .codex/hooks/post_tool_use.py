#!/usr/bin/env python3
from common import done, read_payload, send_hook

payload = read_payload()
tool = payload.get("tool_name") or payload.get("tool") or payload.get("name") or "tool"
send_hook(f"Tool done: {tool}")
done()
