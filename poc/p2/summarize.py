"""PoC P2 한 경우의 stream-json 기록을 한 줄 요약으로 바꾼다.

인자: 이름 jsonl stderr 종료코드 표식상태 훅호출수
"""

import json
import sys

name, jsonl, err, code, marker, guard = sys.argv[1:7]

init = {}
result = {}
tool_uses = []
hook_events = []
denied_msgs = 0
tool_errors = []

try:
    lines = open(jsonl, encoding="utf-8").read().splitlines()
except OSError:
    lines = []

for line in lines:
    line = line.strip()
    if not line:
        continue
    try:
        d = json.loads(line)
    except json.JSONDecodeError:
        continue
    t, st = d.get("type"), d.get("subtype")
    if t == "system" and st == "init":
        init = d
    elif t == "system" and st in ("hook_started", "hook_response"):
        if d.get("hook_event") == "PreToolUse":
            hook_events.append(f"{st}:{d.get('exit_code')}")
    elif t == "system" and "denied" in str(st):
        denied_msgs += 1
    elif t == "assistant":
        for block in d.get("message", {}).get("content", []) or []:
            if block.get("type") == "tool_use":
                tool_uses.append(f"{block.get('name')}({str(block.get('input', {}).get('command', ''))[:40]})")
    elif t == "user":
        for block in d.get("message", {}).get("content", []) or []:
            if isinstance(block, dict) and block.get("type") == "tool_result" and block.get("is_error"):
                content = block.get("content")
                text = content if isinstance(content, str) else json.dumps(content, ensure_ascii=False)
                tool_errors.append(text[:90])
    elif t == "result":
        result = d

plugins = [p.get("name") for p in init.get("plugins", []) if not str(p.get("path", "")).startswith("builtin")]
stderr = ""
try:
    stderr = open(err, encoding="utf-8").read().strip().replace("\n", " | ")[:200]
except OSError:
    pass

print(f"== {name}")
print(f"   exit={code} marker={marker.strip()} guard_calls={guard.strip()}")
if init:
    print(f"   init: permissionMode={init.get('permissionMode')} tools={init.get('tools')} plugins={plugins}")
else:
    print("   init: (없음)")
print(f"   tool_uses={tool_uses} pretooluse_hook_events={hook_events}")
if result:
    print(
        f"   result: is_error={result.get('is_error')} terminal={result.get('terminal_reason')} "
        f"denials={len(result.get('permission_denials') or [])} text={str(result.get('result'))[:80]!r}"
    )
if tool_errors:
    print(f"   tool_errors={tool_errors}")
if denied_msgs:
    print(f"   denied_system_msgs={denied_msgs}")
if stderr:
    print(f"   stderr={stderr}")
