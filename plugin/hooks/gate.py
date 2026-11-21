# Lucia의 PreToolUse 훅(결정 0003·0059). only-lucia-tools.sh가 부른다. 표준 입력은 훅 JSON이다.
# 보호 영역: 읽기 전용으로 마운트되고, 사용자가 요청할 때만 바꾼다.
#
# 1. 도구 이름: 데몬 MCP 도구(mcp__lucia__*)와, 데몬이 이 실행에 열어 준 내장 도구(LUCIA_BUILTIN_TOOLS)만 통과시킨다.
#    데몬이 알려 주지 않았으면 WebSearch만이다(예전 규칙).
# 2. Bash: 되돌릴 수 없이 바깥에 영향을 줄 수 있는 명령(git push, 릴리스·배포·패키지 게시, 바깥으로 보내는 쓰기 요청,
#    원격 접속 등)은 실행 전에 데몬에 물어 사용자의 승인 카드를 기다린다. 승인되면 통과, 아니면 막는다.
#    명령의 모양으로 가려내므로 놓치는 것이 있다(코드 안에서 보내는 요청 등). 그런 일은 모델이 approval_request로 묻는다.
# 종료 코드: 0 통과, 2 막음(표준 오류의 글이 모델에게 간다). 해석하지 못하면 막는다.
import json
import os
import re
import sys
import time
import urllib.request

WAIT_SECONDS = 280  # 승인을 기다리는 시간. hooks.json의 timeout보다 짧아야 한다.


def deny(message):
    print(message, file=sys.stderr)
    sys.exit(2)


READ_ONLY_GH = re.compile(
    r"\b(?:gh|glab)\s+(?:(?:[\w-]+\s+)?(?:list|ls|view|show|status|diff|checks|download|clone|checkout|watch|logs?)\b"
    r"|(?:search|browse|help|version|completion|extension\s+list|auth\s+status)\b|--version\b|--help\b)"
)
# 옵션은 대소문자를 가린다(-F와 -f는 다르다). 메서드 이름만 가리지 않는다.
API_WRITE = re.compile(
    r"(?:-X|--method)[ =]*['\"]?(?i:post|put|patch|delete)\b|\s(?:-f|-F|--field|--raw-field|--input)(?:[ =]|$)"
)
CURL_WRITE = re.compile(
    r"(?:-X|--request)[ =]*['\"]?(?i:post|put|patch|delete)\b"
    r"|\s(?:-d|--data(?:-[a-z]+)?|-F|--form(?:-string)?|-T|--upload-file|--json)(?:[ =]|$)"
)
RULES = [
    ("git push", re.compile(r"\bgit\b[^;&|\n]*\b(?:push|send-email)\b")),
    ("패키지·이미지 게시", re.compile(
        r"\b(?:npm|pnpm|yarn|bun|poetry|uv|flit|hatch|gem|cargo|helm|docker|podman|twine|pod)\b[^;&|\n]*\b(?:publish|push|upload|yank)\b")),
    ("배포·클라우드 도구", re.compile(
        r"\b(?:vercel|netlify|flyctl|fly|firebase|wrangler|heroku|serverless|sls|amplify|pulumi|terraform|tofu|kubectl"
        r"|ansible-playbook|aws|gcloud|az|doctl|supabase|railway|surge|mvn\s[^;&|\n]*deploy|gradlew?\s[^;&|\n]*publish)\b")),
    ("원격 서버 접속·전송", re.compile(r"\b(?:ssh|scp|sftp|ftp|lftp)\b|\brsync\b[^;&|\n]*\S+:")),
    ("메일 보내기", re.compile(r"\b(?:sendmail|msmtp|mutt|mailx?|swaks)\b")),
    ("바깥으로 보내는 쓰기 요청(wget)", re.compile(
        r"\bwget\b[^;&|\n]*(?:--post-data|--post-file|--body-data|--body-file|--method[ =]*['\"]?(?:post|put|patch|delete))")),
    ("바깥으로 보내는 쓰기 요청(httpie)", re.compile(r"\b(?:https?|xh)\s+(?:-\S+\s+)*(?:post|put|patch|delete)\b")),
]


def risky(command):
    """바깥에 되돌릴 수 없는 영향을 줄 수 있는 명령이면 무엇인지(카드의 제목), 아니면 None."""
    text = command.lower()
    for label, pattern in RULES:
        if pattern.search(text):
            return label
    for raw in re.split(r"[;&|\n]+", command):
        piece = raw.lower()
        if re.search(r"\b(?:gh|glab)\b", piece):
            if re.search(r"\b(?:gh|glab)\s+api\b", piece):
                if API_WRITE.search(raw):
                    return "GitHub·GitLab에 쓰는 요청"
            elif not READ_ONLY_GH.search(piece):
                return "GitHub·GitLab에 쓰는 명령(릴리스·PR·이슈 등)"
        if re.search(r"\bcurl\b", piece) and CURL_WRITE.search(raw):
            return "바깥으로 보내는 쓰기 요청(curl)"
    return None


def ask(url, token, body):
    data = json.dumps(body).encode()
    request = urllib.request.Request(
        url, data=data, method="POST",
        headers={"Authorization": "Bearer " + token, "Content-Type": "application/json"},
    )
    # 데몬은 VM의 프록시 설정으로도, 직접으로도 닿을 수 있다. 차례로 해 본다.
    last = None
    for opener in (urllib.request.build_opener(), urllib.request.build_opener(urllib.request.ProxyHandler({}))):
        try:
            with opener.open(request, timeout=40) as response:
                return json.loads(response.read().decode())
        except Exception as error:  # noqa: BLE001 - 닿지 못하면 막는다
            last = error
    raise RuntimeError(str(last))


def approval(what, command):
    url, token = os.environ.get("LUCIA_APPROVAL_URL"), os.environ.get("LUCIA_RUN_TOKEN")
    if not url or not token:
        deny(f"lucia: 이 명령({what})은 사용자 승인이 필요한데, 이 실행에서는 승인을 받을 길이 없다. 실행하지 않는다.")
    started = time.monotonic()
    body = {"what": what, "detail": command}
    while True:
        try:
            answer = ask(url, token, body)
        except Exception as error:  # noqa: BLE001
            deny(f"lucia: 이 명령({what})은 사용자 승인이 필요한데 데몬에 묻지 못했다({error}). 실행하지 않는다.")
        status = answer.get("status")
        if status == "approved":
            return
        message = answer.get("message") or ""
        if status != "pending":
            deny(f"lucia: 이 명령({what})을 실행하지 않았다. {message}")
        if time.monotonic() - started > WAIT_SECONDS:
            deny(f"lucia: 이 명령({what})은 사용자 승인을 기다리고 있다. {message} 승인된 뒤 같은 명령을 그대로 다시 실행하면 통과한다.")
        body = {"id": answer.get("id")}


def main():
    try:
        event = json.load(sys.stdin)
        name = event.get("tool_name") or ""
    except Exception:  # noqa: BLE001
        deny("lucia: 허용되지 않은 도구 (해석 실패)")
    if name.startswith("mcp__lucia__"):
        return
    allowed = [t for t in os.environ.get("LUCIA_BUILTIN_TOOLS", "WebSearch").split(",") if t]
    if not name or name not in allowed:
        deny(f"lucia: 허용되지 않은 도구 {name or '(해석 실패)'}")
    if name != "Bash":
        return
    command = (event.get("tool_input") or {}).get("command") or ""
    what = risky(command)
    if what:
        approval(what, command)


if __name__ == "__main__":
    main()
