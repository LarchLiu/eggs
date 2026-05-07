#!/usr/bin/env python3
from common import done, read_payload, send_hook

payload = read_payload()
prompt = payload.get("prompt") or payload.get("user_prompt") or payload.get("input") or ""
send_hook(f"User: {prompt}")
done()
