#!/usr/bin/env python3
from common import done, read_payload, send_hook

payload = read_payload()
text = payload.get("last_assistant_message") or payload.get("assistant_message") or ""
send_hook(f"Agent: {text}")
done()
