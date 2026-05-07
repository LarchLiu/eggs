#!/usr/bin/env python3
from common import done, read_payload, send_hook

payload = read_payload()
tool = payload.get("tool_name") or payload.get("tool") or "unknown"
reason = payload.get("reason") or payload.get("justification") or ""
send_hook(f"Permission: {tool} {reason}".strip())
done()
