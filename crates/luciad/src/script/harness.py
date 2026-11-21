# Lucia 스크립트 실행기(결정 0025). 데몬이 스크립트 샌드박스에서 `python3 -I -u -c <이 글> <표지>`로 띄운다.
#
# 표준 입력 첫 줄: {"code", "input", "state", "printed_max", "proxy"}
# 도구 부르기: 표준 출력(복제한 원래 fd 1)으로 {"op": "call"|"fetch", "id", ...} 한 줄을 쓰고, 표준 입력으로 답
# {"id", "ok", "value"|"error"} 한 줄을 읽는다. 끝나면 {"op": "end", ...} 한 줄.
# 코드의 print와 자식 프로세스의 출력이 이 통로를 더럽히지 않게 fd 1은 stderr로 돌리고 print는 따로 모은다.
import base64
import io
import json
import os
import sys
import traceback
import types


class ToolError(Exception):
    """도구가 거절했거나 실패했다."""


class _Capture(io.TextIOBase):
    def __init__(self, limit):
        self.parts = []
        self.size = 0
        self.limit = limit
        self.cut = False

    def writable(self):
        return True

    def write(self, s):
        if not isinstance(s, str):
            s = str(s)
        room = self.limit - self.size
        if room <= 0:
            if s:
                self.cut = True
            return len(s)
        if len(s) > room:
            self.parts.append(s[:room])
            self.size += room
            self.cut = True
        else:
            self.parts.append(s)
            self.size += len(s)
        return len(s)

    def text(self):
        out = "".join(self.parts)
        if self.cut:
            out += "\n…(출력이 길어 잘림)"
        return out


def _main():
    rpc_out = os.fdopen(os.dup(1), "w", encoding="utf-8")
    rpc_in = sys.stdin
    os.dup2(2, 1)
    job = json.loads(rpc_in.readline())
    # 바깥에 나가도 되는 실행이면 데몬의 출구 프록시를 쓴다(결정 0058). 코드와 코드가 띄운 프로그램이 이 값을 본다.
    proxy = job.get("proxy")
    if isinstance(proxy, str) and proxy:
        for key in ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"):
            os.environ[key] = proxy
            os.environ[key.lower()] = proxy
        os.environ["NO_PROXY"] = os.environ["no_proxy"] = "localhost,127.0.0.1,::1"
    printed = _Capture(int(job.get("printed_max") or 16000))
    seq = [0]

    def rpc(msg):
        seq[0] += 1
        msg["id"] = seq[0]
        rpc_out.write(json.dumps(msg, ensure_ascii=False, default=str) + "\n")
        rpc_out.flush()
        line = rpc_in.readline()
        if not line:
            raise SystemExit("데몬과의 연결이 끊겼다")
        reply = json.loads(line)
        if not reply.get("ok"):
            raise ToolError(reply.get("error") or "실패")
        return reply.get("value")

    effects = {"notify": [], "wake": None, "next": None, "done": None}
    returned = []

    m = types.ModuleType("lucia")
    m.__doc__ = "Lucia 도구와 루틴 효과. 결정 0025."
    m.ToolError = ToolError
    m.input = job.get("input") if isinstance(job.get("input"), dict) else {}
    m.state = job.get("state") if isinstance(job.get("state"), dict) else {}
    # 이 실행의 코드가 바깥 인터넷에 직접 나갈 수 있는가.
    m.online = bool(isinstance(proxy, str) and proxy)

    def call(tool, args=None, **kw):
        """Lucia 도구를 부른다. JSON을 주는 도구는 list/dict, 나머지는 글(str)."""
        a = dict(args or {})
        a.update(kw)
        a = {k: v for k, v in a.items() if v is not None}
        return rpc({"op": "call", "tool": str(tool), "args": a})

    def fetch(url, session=False):
        """웹 주소를 GET으로 가져온다. 본문 글(HTML·JSON·텍스트). 기본은 쿠키 없음.
        session=True면 로그인된 브라우저의 세션(쿠키)을 빌린다(사이트 위임한 사이트만, 루틴·스킬은 fetch_session 선언)."""
        return rpc({"op": "fetch", "url": str(url), "session": bool(session)})

    def file_info(id):
        """파일 id(f_…)의 정보: {"id", "name", "mime", "kind", "size"}."""
        return rpc({"op": "file_info", "file": str(id)})

    def read_file(id):
        """파일 id(f_…)의 내용을 bytes로 읽는다(사용자가 붙인 파일, 만든 그림, 웹에서 받은 파일 등). 100 MB까지."""
        first = rpc({"op": "file_read", "file": str(id), "offset": 0})
        parts = [base64.b64decode(first["data"])]
        got, size = len(parts[0]), int(first.get("size") or 0)
        # 큰 파일은 조각으로 온다.
        while got < size:
            more = base64.b64decode(rpc({"op": "file_read", "file": str(id), "offset": got})["data"])
            if not more:
                break
            parts.append(more)
            got += len(more)
        return b"".join(parts)

    def download(url, session=False, name=None):
        """웹 주소의 파일을 받아 파일로 남기고 새 파일 id를 돌려준다(zip·PDF·그림 등 종류를 가리지 않는다, 100 MB까지).
        내용은 read_file(id)로 읽는다. session=True면 로그인된 브라우저의 세션으로 받는다(사이트 위임한 사이트만.
        위임하지 않은 사이트의 로그인 파일은 스크립트 밖에서 file_download 도구로 받는다).
        스크립트 시간 안에 다 받지 못할 큰 파일도 file_download 도구로 받는다."""
        msg = {"op": "download", "url": str(url), "session": bool(session)}
        if name:
            msg["name"] = str(name)
        return rpc(msg)["id"]

    def save_file(name, data):
        """bytes(또는 글)를 파일로 남기고 새 파일 id를 돌려준다. 30 MB까지.
        이 id는 report_result의 attachments나 expression_add의 file에 쓴다."""
        if isinstance(data, str):
            data = data.encode("utf-8")
        encoded = base64.b64encode(bytes(data)).decode("ascii")
        return rpc({"op": "file_save", "name": str(name), "data": encoded})["id"]

    def notify(text, batch=False):
        """사용자에게 바로 알린다. batch면 모아 보는 소식으로(30분 안에 묶어서). 방해 금지 시간에는 늘 모은다."""
        effects["notify"].append({"text": str(text), "batch": bool(batch)})

    def wake(note, data=None):
        """(루틴) Lucia를 깨워 판단을 맡긴다. note는 무엇을 볼지, data는 넘길 자료."""
        effects["wake"] = {"note": str(note), "data": data}

    def next_run(minutes):
        """(루틴) 다음 실행을 지금부터 minutes분 뒤로 한 번 당기거나 미룬다."""
        effects["next"] = float(minutes)

    def finish(reason=""):
        """(루틴) 이 루틴을 끝낸다(목표를 이뤘을 때)."""
        effects["done"] = str(reason) or "끝"

    def result(value):
        """부른 쪽에 돌려줄 값(JSON으로 바꿀 수 있는 것)."""
        returned[:] = [value]

    def mail_search(query, limit=10):
        return call("mail_search", query=query, limit=limit)

    def mail_read(id):
        return call("mail_read", id=id)

    def calendar_events(start=None, end=None, query=None, limit=30):
        return call("calendar_events", {"from": start, "to": end, "query": query, "limit": limit})

    def discord_messages(channel, limit=30, before=None, after=None, around=None):
        return call(
            "discord_messages",
            channel=str(channel),
            limit=limit,
            before=before,
            after=after,
            around=around,
        )

    for fn in (call, fetch, notify, wake, next_run, finish, result, mail_search, mail_read,
               calendar_events, discord_messages, file_info, read_file, save_file, download):
        setattr(m, fn.__name__, fn)
    sys.modules["lucia"] = m

    sys.stdout = printed
    ns = {"__name__": "__main__", "lucia": m}
    ok, error = True, None
    try:
        exec(compile(job["code"], "<code>", "exec"), ns)
    except SystemExit as e:
        if e.code not in (None, 0):
            ok, error = False, f"SystemExit: {e.code}"
    except BaseException as e:  # noqa: BLE001 - 코드의 모든 실패를 보고한다
        ok = False
        frames = [f for f in traceback.extract_tb(e.__traceback__) if f.filename == "<code>"]
        error = "".join(traceback.format_list(frames[-6:]))
        error += "".join(traceback.format_exception_only(type(e), e))
    sys.stdout = sys.__stdout__

    value = returned[0] if returned else ns.get("result")
    if callable(value) or isinstance(value, types.ModuleType):
        value = None
    end = {
        "op": "end",
        "ok": ok,
        "printed": printed.text(),
        "result": value,
        "error": error,
        "effects": effects,
        "state": m.state if isinstance(m.state, dict) else {},
    }
    try:
        line = json.dumps(end, ensure_ascii=False, default=str)
    except (TypeError, ValueError) as e:
        end["result"], end["state"] = None, {}
        end["ok"], end["error"] = False, f"결과를 JSON으로 바꾸지 못했다: {e}"
        line = json.dumps(end, ensure_ascii=False, default=str)
    rpc_out.write(line + "\n")
    rpc_out.flush()


_main()
