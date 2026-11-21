//! 개발용 점검(`luciad browser-check`): 실제 브라우저 샌드박스로 다음을 확인한다. 실제 계정으로는 로그인하지 않는다.
//!
//! 1. 데몬 프록시를 띄우고(설정 `browser.proxy_bind`) 내장 사이트 첫 페이지 읽기(요청 한 번, 프록시 경유).
//! 2. VM 루프백의 시험 페이지로:
//!    - 로그인(내장 프로필과 같은 칸): 빈 아이디(대화상자 자동 닫기) → 틀린 비밀번호(사이트 오류 문구) →
//!      맞는 비밀번호(공백 포함, 로그인 유지 체크) → 다시 로그인(이미 로그인됨) → 닫았다 열어도 로그인 유지.
//!    - 일반 프로필 로그인(칸 이름 없이 찾기).
//!    - 단계형 로그인(결정 0017): 한 화면에서 비밀번호 칸이 나중에 나오는 것, 두 페이지로 나뉜 것, 다음 단계가
//!      다른 곳으로 넘어가는 것(비밀번호를 넣지 않음), 비밀번호 칸이 끝내 안 나오는 것(시간 안에 멈추고 비움),
//!      이미 로그인돼 넘어간 페이지의 입력칸은 건드리지 않는 것.
//!    - 화면 넘겨받기(결정 0017): 화면 받기, 누른 자리로 새 창 열기(닫지 않고 탭으로), 새 창 화면, 끝내면 닫기,
//!      넘겨받지 않은 상태의 입력 거절.
//!    - 인증 코드(결정 0017): 비밀번호 뒤 코드 칸 찾기, 틀린 코드 거절, 맞는 코드로 로그인, 코드 칸이 없으면 넣지 않음.
//!    - 화면 동작: 요소 등급(링크·펼치기·GET 폼은 그냥, 스크립트 단추·POST 폼은 승인, 로그아웃은 못 함),
//!      펼치기, 확인(confirm) 대화상자 취소, 검색 제출, 뒤로, 가려진 요소, 낡은 참조, 승인된 POST, 스크린샷.
//!      승인 없이 보낸 POST가 하나도 없는지 서버 쪽 수로 확인한다.
//!    - 좌표·iframe(결정 0023): 같은 사이트·다른 사이트(별도 프로세스) iframe의 글과 요소 좌표, 좌표 클릭과 초점 입력,
//!      iframe 안의 POST 제출·스크립트 단추·Enter·Space는 승인, iframe 안 비밀번호 칸에 글 넣기는 못 함, 투명 제출 단추·
//!      `pointer-events:none` 미끼·리스너만 단 div는 그 아래 실제 요소로 판정, 승인을 기다리는 사이 바뀐 단추는 하지 않음,
//!      iframe 안 휠 스크롤. 이 동안 승인 없이 나간 POST가 없는지 서버 쪽 수로 본다.
//!    - 프록시: 사설 주소로 풀리는 이름의 하위 요청·넘겨주기가 막히는지.
//!    - 내려받기(결정 0057, `LUCIA_BROWSER_CHECK_ONLY=downloads`로 이것만): 브라우저는 받지 않고 시작하려던 주소를
//!      알리는지(링크 누르기, 파일 주소 바로 열기), PDF 주소가 빈 화면으로 열리는지.
//!
//! Chrome은 루프백을 프록시로 보내지 않으므로 시험 페이지에 바로 닿는다. 그래서 이 점검만 시험 출처를 주소
//! 검사에서 뺀다([`Egress::with_dev_origin`]).

use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncWriteExt as _;
use tokio_util::sync::CancellationToken;

use super::egress::Egress;
use super::proxy::Proxy;
use super::sandbox::{SbxBrowser, SbxConfig};
use super::sites::{LoginForm, SiteProfile, select};
use super::tools::{Gate, classify, classify_focus, classify_hit};
use super::{Act, Browser, BrowserError, Element, Expect, Focus, LoginResult, PageView, Point};
use crate::config::Config;

const FIXTURE_USER: &str = "tester";
const FIXTURE_PASSWORD: &str = "fixture pw-1";
const FIXTURE: &str = "http://127.0.0.1:8000";

/// 시험용 서버(VM 안 127.0.0.1:8000).
const FIXTURE_SERVER: &str = r#"# lucia-fixture: 시험용 로그인·동작 페이지
import http.server, urllib.parse
USER, PW = "tester", "fixture pw-1"
POSTS = [0]
PAGE = """<!doctype html><meta charset=utf-8><title>시험 로그인</title>
<form id=frm method=post action=/login>
<input id=login_id name=login_id type=text oninput="t()">
<input id=password name=password type=password oninput="t()">
<input type=checkbox id=login-check name=remember><label for=login-check>자동 로그인</label>
<button id=btn-login type=button onclick="loginSubmit()">로그인</button>
</form>
<p class=invalid-feedback>%s</p>
<script>function t(){} function loginSubmit(){ if(!document.getElementById('login_id').value){alert('아이디를 입력하세요');return;} document.getElementById('frm').submit(); }</script>"""
# 이미지 로그인 단추와 제출 검사 스크립트(2026-11-18 운영에서 실패한 사이트와 같은 모양). LOGIN12는 단추가
# 숨어 있어 Enter로 제출한다.
PAGE11 = """<!doctype html><meta charset=utf-8><title>이미지 단추 로그인</title>
<form name=login method=post action=/login onSubmit="check(); return false;">
<input type=hidden name=login value=s>
%s
<input name=login_id type=text maxlength=17>
<input name=password type=password maxlength=17>
<input type=checkbox id=keep name=remember><label for=keep>로그인 유지</label>
</form>
<script>function check(){ var f=document.login; if(f.login_id.value==""){alert("아이디를 입력하세요.");return;} if(f.password.value==""){alert("비밀번호를 입력하세요.");return;} f.submit(); }</script>"""
# 에디터가 글자를 받는 숨은 iframe(문서 전체 편집 모드, 2026-11-18 네이버 카페 글쓰기와 같은 모양).
EDITOR = """<!doctype html><meta charset=utf-8><title>에디터</title><p>본문 영역</p>
<iframe id=buf style="width:300px;height:80px"></iframe>
<script>const f=document.getElementById('buf'); const d=f.contentDocument; d.open(); d.write('<body></body>'); d.close(); d.designMode='on';
setTimeout(()=>{ f.contentWindow.focus(); d.body.focus(); }, 50);</script>"""
PAGE2 = """<!doctype html><meta charset=utf-8><title>시험 로그인 2</title>
<form method=post action=/login><div><input name=login_id autocomplete=username placeholder=아이디></div>
<div><input type=password name=password placeholder=비밀번호></div>
<label><input type=checkbox name=remember> 로그인 상태 유지</label><button>로그인</button></form>
<p class=invalid-feedback>%s</p>"""
STEP = """<!doctype html><meta charset=utf-8><title>시험 단계형 로그인</title>
<form method=post action=/login><input name=login_id autocomplete=username placeholder=아이디>
<button type=button id=next onclick="step()">다음</button>
<div id=s2 hidden><input type=password name=password placeholder=비밀번호>
<label><input type=checkbox name=remember> 로그인 상태 유지</label><button>로그인</button></div></form>
<script>function step(){ %s }</script>"""
STEP_SPA = "document.querySelector('[name=login_id]').readOnly=true; setTimeout(()=>{document.getElementById('s2').hidden=false;document.getElementById('next').hidden=true;}, 800);"
STEP_AWAY = "location.href='http://127.0.0.1:8001/login4';"
STEP_NOTHING = ""
PAGE4 = """<!doctype html><meta charset=utf-8><title>시험 두 페이지 로그인</title>
<form method=post action=/login4/next><input name=login_id placeholder=아이디><button>다음</button></form>"""
PAGE4B = """<!doctype html><meta charset=utf-8><title>시험 두 페이지 로그인 2</title>
<form method=post action=/login><input type=hidden name=login_id value="%s"><p>%s 님</p>
<input type=password name=password><label><input type=checkbox name=remember> 로그인 상태 유지</label>
<button>로그인</button></form>"""
OTP = """<!doctype html><meta charset=utf-8><title>시험 인증</title>
<form method=post action=/otp><label for=otp>인증 번호</label>
<input id=otp name=otp inputmode=numeric autocomplete=one-time-code maxlength=6>
<button>확인</button></form><p class=invalid-feedback>%s</p>"""
ACT = """<!doctype html><meta charset=utf-8><title>시험 동작</title>
<a id=next href=/page2>다음 쪽</a>
<button id=more aria-expanded=false onclick="this.setAttribute('aria-expanded','true');document.getElementById('extra').hidden=false">펼쳐 보기</button>
<p id=extra hidden>펼친 글</p>
<button id=js onclick="if(confirm('정말 보낼까요?')) fetch('/hit',{method:'POST'})">스크립트 보내기</button>
<form method=get action=/search><input name=q type=search placeholder=검색어><button>찾기</button></form>
<form method=post action=/post><input name=title placeholder=제목><button>글 등록</button></form>
<div style="position:relative;width:200px"><button id=under>가려진 단추</button><div style="position:absolute;inset:0;background:#fff"></div></div>
<a href=/logout>로그아웃</a>
<img src="http://10.1.2.3.nip.io/pixel.png" alt="">
<p>POST %d</p>"""
# 결정 0023: iframe(같은 사이트 8001, 다른 사이트 localhost:8002)과 좌표로만 닿는 함정들.
FRAMES = """<!doctype html><meta charset=utf-8><title>시험 iframe</title><style>body{margin:0} iframe{border:0;position:absolute}</style>
<iframe id=same style="left:20px;top:20px;width:400px;height:260px" src="http://127.0.0.1:8001/inner"></iframe>
<iframe id=cross style="left:440px;top:20px;width:400px;height:260px" src="http://localhost:8002/inner"></iframe>
<p style="position:absolute;left:20px;top:300px;margin:0">무해한 글</p>
<form method=post action=/post><button style="position:absolute;left:20px;top:300px;width:200px;height:30px;opacity:0">몰래 등록</button></form>
<form method=post action=/post><button style="position:absolute;left:20px;top:350px;width:200px;height:30px">진짜 등록</button></form>
<div style="position:absolute;left:20px;top:350px;width:200px;height:30px;background:#fff;pointer-events:none">펼치기</div>
<div id=lis style="position:absolute;left:20px;top:400px;width:200px;height:30px;cursor:pointer">좋아요</div>
<script>lis.onclick=()=>fetch('/hit',{method:'POST'})</script>"""
LABELS = """<!doctype html><meta charset=utf-8><title>시험 label</title><style>body{margin:0} .a{position:absolute;left:20px;width:200px;height:30px;margin:0}</style>
<form method=post action=/post><button id=sb class=a style="top:20px">레이블 등록</button></form>
<label for=sb class=a style="top:70px">눌러서 등록</label>
<input type=file id=fl style="position:absolute;left:400px;top:20px"><label for=fl class=a style="top:120px">파일 고르기</label>
<form method=post action=/post><input name=t placeholder=칸 class=a style="top:170px"><input type=checkbox name=ok style="position:absolute;left:20px;top:220px">
<select name=s class=a style="top:260px"><option>가</option><option>나</option></select><button class=a style="top:300px">보내기 둘</button></form>
<form method=get action=/search><input name=q placeholder=찾을칸 class=a style="top:360px"><input type=checkbox name=c style="position:absolute;left:20px;top:410px"><button class=a style="top:450px">찾기 둘</button></form>"""
RELOADING = """<!doctype html><meta charset=utf-8><title>시험 다시 뜨는 iframe</title>
<a href=/page2>다음 쪽</a><iframe src=/tick style="width:300px;height:100px"></iframe>"""
MOVING = """<!doctype html><meta charset=utf-8><title>시험 바뀌는 단추</title><style>body{margin:0}</style>
<form method=post action=/post><button id=mv style="position:absolute;left:20px;top:20px;width:200px;height:30px">등록 둘</button></form>
<script>setTimeout(()=>{mv.textContent='삭제'},4000)</script>"""
INNER = """<!doctype html><meta charset=utf-8><title>위젯</title><body style="margin:0"><p style="position:absolute;left:10px;top:230px;margin:0">위젯 글</p>
<form method=post action="http://127.0.0.1:8000/post"><input name=c placeholder=댓글 style="position:absolute;left:10px;top:10px;width:200px;height:30px"><button style="position:absolute;left:220px;top:10px;width:120px;height:30px">댓글 등록</button></form>
<button id=send style="position:absolute;left:10px;top:60px;width:120px;height:30px">보내기</button>
<form method=post action="http://127.0.0.1:8000/login"><input name=login_id placeholder=아이디 style="position:absolute;left:10px;top:110px;width:200px;height:30px"><input type=password name=password placeholder=비밀번호 style="position:absolute;left:10px;top:150px;width:200px;height:30px"></form>
<div style="height:1500px"></div>
<script>send.onclick=()=>fetch('http://127.0.0.1:8000/hit',{method:'POST',mode:'no-cors'})</script>"""
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def send(self, code, body, headers=()):
        self.send_response(code)
        for k, v in headers: self.send_header(k, v)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.end_headers(); self.wfile.write(body.encode())
    def do_GET(self):
        logged = "lucia_fixture=ok" in (self.headers.get("Cookie") or "")
        path = self.path
        if path.startswith("/login3"):
            if logged: return self.send(302, "", [("Location", "/act")])
            return self.send(200, STEP % STEP_SPA)
        if path.startswith("/login5"):
            return self.send(200, STEP % STEP_AWAY)
        if path.startswith("/login6"):
            return self.send(200, STEP % STEP_NOTHING)
        if path.startswith("/login8"):
            if logged: return self.send(302, "", [("Location", "/")])
            return self.send(200, PAGE2 % "")
        if path.startswith("/otp"):
            return self.send(200, OTP % ("인증 번호가 다릅니다." if "error=1" in path else ""))
        if path.startswith("/login11"):
            if logged: return self.send(302, "", [("Location", "/")])
            return self.send(200, PAGE11 % '<input type=image src="/nope.png" alt="" style="width:80px;height:30px">')
        if path.startswith("/editor"): return self.send(200, EDITOR)
        if path.startswith("/login13"):
            if logged: return self.send(302, "", [("Location", "/")])
            return self.send(200, PAGE11 % '<button type=submit>로그인</button>')
        if path.startswith("/login12"):
            if logged: return self.send(302, "", [("Location", "/")])
            return self.send(200, PAGE11 % '<button type=submit style="display:none">로그인</button>')
        if path.startswith("/login4"):
            if logged: return self.send(302, "", [("Location", "/")])
            return self.send(200, PAGE4)
        if path.startswith("/login2"):
            if logged: return self.send(302, "", [("Location", "/")])
            return self.send(200, PAGE2 % ("아이디 또는 비밀번호가 다릅니다." if "error=1" in path else ""))
        if path.startswith("/login"):
            if logged: return self.send(302, "", [("Location", "/")])
            err = "아이디 또는 비밀번호가 다릅니다." if "error=1" in path else ""
            return self.send(200, PAGE % err)
        if path.startswith("/logout"):
            return self.send(302, "", [("Location", "/"), ("Set-Cookie", "lucia_fixture=; Path=/; Max-Age=0")])
        if path.startswith("/act"): return self.send(200, ACT % POSTS[0])
        if path.startswith("/page2"): return self.send(200, "<title>둘째 쪽</title><p>둘째</p>")
        if path.startswith("/search"):
            q = urllib.parse.parse_qs(urllib.parse.urlparse(path).query).get("q", [""])[0]
            return self.send(200, "<title>검색</title><p>검색: %s</p>" % q)
        if path.startswith("/redir"): return self.send(302, "", [("Location", "http://192.168.0.250.nip.io/")])
        if path.startswith("/popup"):
            return self.send(200, "<title>팝업 시험</title><button id=open style='position:absolute;left:0;top:0;width:200px;height:40px' onclick=\"window.open('/page2','auth','width=400,height=400')\">본인인증 창 열기</button><input id=box placeholder=글칸>")
        if path.startswith("/count"): return self.send(200, "<title>수</title><p>POST %d</p>" % POSTS[0])
        # 내려받기(결정 0057): 첨부로 주는 zip, 넘겨준 뒤 첨부로 주는 것, 그대로 여는 PDF.
        if path.startswith("/dl-page"):
            return self.send(200, "<title>자료실</title><a href=/dl/file>압축 파일</a> <a href=/dl/moved>옮긴 파일</a> <a href=/dl/doc.pdf>안내 PDF</a>")
        if path.startswith("/dl/moved"): return self.send(302, "", [("Location", "/dl/file")])
        if path.startswith("/dl/file"):
            body = b"PK\x03\x04" + b"lucia-fixture-zip" * 4
            self.send_response(200)
            self.send_header("Content-Type", "application/zip")
            self.send_header("Content-Disposition", "attachment; filename*=UTF-8''%EC%9E%90%EB%A3%8C.zip")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers(); self.wfile.write(body); return
        if path.startswith("/dl/doc.pdf"):
            body = b"%PDF-1.4\n1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj\n2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj\n3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 200 200]>>endobj\ntrailer<</Root 1 0 R>>\n%%EOF\n"
            self.send_response(200)
            self.send_header("Content-Type", "application/pdf")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers(); self.wfile.write(body); return
        if path.startswith("/frames"): return self.send(200, FRAMES)
        if path.startswith("/moving"): return self.send(200, MOVING)
        if path.startswith("/labels"): return self.send(200, LABELS)
        if path.startswith("/reloading"): return self.send(200, RELOADING)
        if path.startswith("/tick"): return self.send(200, "<p>광고</p><script>setTimeout(()=>location.reload(),300)</script>")
        if path.startswith("/inner"): return self.send(200, INNER)
        self.send(200, "<title>시험 홈</title><p>%s</p>" % ("로그인됨" if logged else "손님"))
    def do_POST(self):
        n = int(self.headers.get("Content-Length") or 0)
        raw = self.rfile.read(n).decode()
        if self.path.startswith("/hit"):
            POSTS[0] += 1; return self.send(204, "")
        if self.path.startswith("/post"):
            POSTS[0] += 1; return self.send(302, "", [("Location", "/act")])
        f = urllib.parse.parse_qs(raw)
        if self.path.startswith("/otp"):
            if f.get("otp") == ["424242"]:
                return self.send(302, "", [("Location", "/"), ("Set-Cookie", "lucia_fixture=ok; Path=/; Max-Age=3600")])
            return self.send(302, "", [("Location", "/otp?error=1")])
        if self.path.startswith("/login4/next"):
            who = (f.get("login_id") or [""])[0].replace("<", "").replace('"', "")
            return self.send(200, PAGE4B % (who, who))
        ok = f.get("login_id") == [USER] and f.get("password") == [PW] and f.get("remember") == ["on"]
        referer = self.headers.get("Referer") or ""
        back = "/login2" if "login2" in referer else "/login"
        # /login8: 비밀번호가 맞으면 인증 번호 단계로(결정 0017).
        if ok and "login8" in referer: return self.send(302, "", [("Location", "/otp")])
        # /login13: 비밀번호 뒤 사람 확인(캡차) 화면. 비밀번호 칸은 없다(2026-11-18 네이버와 같은 모양).
        if "login13" in referer:
            return self.send(200, "<title>로그인</title><p>추가 보안 확인을 완료해 주세요.</p><p>영수증의 합계는 얼마입니까?</p><input name=answer><button>확인</button>")
        if ok: return self.send(302, "", [("Location", "/"), ("Set-Cookie", "lucia_fixture=ok; Path=/; Max-Age=3600")])
        self.send(302, "", [("Location", back + "?error=1")])
import threading
# 8001: 다른 출처(포트가 다르다). 단계형 로그인이 다른 곳으로 넘어가는 경우를 본다.
# 여러 연결을 함께 받는다(Chrome이 미리 열어 둔 빈 연결이 다음 요청을 막지 않게).
threading.Thread(target=http.server.ThreadingHTTPServer(("127.0.0.1", 8001), H).serve_forever, daemon=True).start()
# 8002: 다른 사이트(localhost는 127.0.0.1과 사이트가 달라 별도 프로세스 iframe이 된다, 결정 0023).
threading.Thread(target=http.server.ThreadingHTTPServer(("127.0.0.1", 8002), H).serve_forever, daemon=True).start()
http.server.ThreadingHTTPServer(("127.0.0.1", 8000), H).serve_forever()
"#;

fn fixture_site() -> SiteProfile {
    SiteProfile {
        id: "fixture".into(),
        name: "시험 로그인".into(),
        origin: FIXTURE.into(),
        hosts: vec!["127.0.0.1".into()],
        login: LoginForm {
            url: format!("{FIXTURE}/login"),
            path: "/login".into(),
            username: "#login_id".into(),
            password: "#password".into(),
            remember: Some("#login-check".into()),
            submit: Some("#btn-login".into()),
            auto: false,
        },
    }
}

/// 일반 프로필과 같은 찾기 방식(https가 아니라 `SiteProfile::generic`을 못 쓰므로 손으로 만든다).
fn fixture_generic() -> SiteProfile {
    fixture_generic_at("/login2")
}

fn fixture_generic_at(path: &str) -> SiteProfile {
    let attr = |v: &str| format!("[{}=\"{v}\"]", super::sites::AUTO_ATTR);
    SiteProfile {
        id: "127.0.0.1".into(),
        name: "127.0.0.1".into(),
        origin: FIXTURE.into(),
        hosts: vec!["127.0.0.1".into()],
        login: LoginForm {
            url: format!("{FIXTURE}{path}"),
            path: path.into(),
            username: attr("user"),
            password: attr("pw"),
            remember: Some(attr("remember")),
            submit: Some(attr("submit")),
            auto: true,
        },
    }
}

async fn sbx(cfg: &Config, args: &[&str], stdin: Option<&str>) -> anyhow::Result<()> {
    let mut child = tokio::process::Command::new(&cfg.sandbox.sbx)
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    if let Some(input) = stdin {
        let mut w = child.stdin.take().expect("stdin piped");
        w.write_all(input.as_bytes()).await?;
    }
    let status = child.wait().await?;
    anyhow::ensure!(status.success(), "sbx {} 실패({status})", args.join(" "));
    Ok(())
}

fn check(ok: bool, what: &str, detail: String) -> anyhow::Result<()> {
    println!("{} {what}: {detail}", if ok { "✓" } else { "✗" });
    anyhow::ensure!(ok, "{what} 실패");
    Ok(())
}

fn find<'a>(view: &'a PageView, label: &str) -> anyhow::Result<&'a Element> {
    view.elements
        .iter()
        .find(|e| e.label == label)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "요소 {label:?} 없음: {:?}",
                view.elements.iter().map(|e| &e.label).collect::<Vec<_>>()
            )
        })
}

/// 요소를 다시 읽어 등급을 정하고, 그냥 할 수 있으면 한다(도구와 같은 흐름).
async fn free_act(browser: &SbxBrowser, act: Act) -> anyhow::Result<PageView> {
    let r = act.target().expect("요소 동작").to_owned();
    let (url, el) = browser.inspect(&r).await?;
    let gate = classify(&act, &el, &url);
    anyhow::ensure!(
        gate == Gate::Free,
        "그냥 할 수 있는 동작이 아니다: {gate:?}"
    );
    let expect = Expect {
        url,
        element: el,
        approved: false,
    };
    Ok(browser.act(&act, Some(&expect)).await?)
}

/// 좌표 클릭을 도구와 같은 흐름으로(결정 0023): 그 점을 읽어 등급을 정하고, 그냥 할 수 있으면 한다.
async fn coord(
    browser: &SbxBrowser,
    at: Point,
) -> anyhow::Result<(Gate, Element, Option<PageView>)> {
    let hit = browser.hit(at).await?;
    let gate = classify_hit(&hit);
    let el = hit.target();
    let view = if gate == Gate::Free {
        let expect = Expect {
            url: hit.url.clone(),
            element: el.clone(),
            approved: false,
        };
        Some(browser.act(&Act::ClickAt { at }, Some(&expect)).await?)
    } else {
        None
    };
    Ok((gate, el, view))
}

/// 초점 동작을 도구와 같은 흐름으로.
async fn focus_act(browser: &SbxBrowser, act: Act) -> anyhow::Result<(Gate, Focus)> {
    let (url, focus) = browser.focused().await?;
    let gate = classify_focus(&act, &url, &focus);
    if gate == Gate::Free {
        let expect = Expect {
            url,
            element: focus.target(),
            approved: false,
        };
        browser.act(&act, Some(&expect)).await?;
    }
    Ok((gate, focus))
}

/// 화면의 iframe 요소(프레임 주소에 `host`가 든 것)의 좌표.
fn frame_point(view: &PageView, host: &str, label: &str) -> anyhow::Result<Point> {
    view.frames
        .iter()
        .filter(|f| f.url.contains(host))
        .flat_map(|f| f.elements.iter())
        .find(|(_, e)| e.label == label)
        .map(|(p, _)| *p)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "iframe({host}) 요소 {label:?} 없음: {:?}",
                view.frames
                    .iter()
                    .map(|f| (
                        &f.url,
                        f.elements.iter().map(|(_, e)| &e.label).collect::<Vec<_>>()
                    ))
                    .collect::<Vec<_>>()
            )
        })
}

fn approval(g: &Gate) -> bool {
    matches!(g, Gate::Approval(_) | Gate::Publish(_))
}

/// 좌표·iframe(결정 0023).
/// 로그인 화면 살펴보기(개발용): 가짜 값으로 넣어 보고 제출 뒤 화면을 시간에 따라 찍는다.
async fn probe_login(browser: &Arc<SbxBrowser>, egress: &Arc<Egress>) -> anyhow::Result<()> {
    let raw = std::env::var("LUCIA_PROBE_URL").map_err(|_| anyhow::anyhow!("LUCIA_PROBE_URL"))?;
    let url = egress.check_url(&raw).map_err(|e| anyhow::anyhow!(e))?;
    let site = SiteProfile::generic(&url).map_err(|e| anyhow::anyhow!(e))?;
    let before = browser.open(url.as_str(), None).await?;
    println!(
        "제출 전: {} · {:?} · 로그인 칸 {} · 요소 {}개",
        before.url, before.title, before.login_form, before.element_total
    );
    let started = std::time::Instant::now();
    let r = browser
        .login(&site, "lucia_probe_user", "probe-Pass-1234")
        .await;
    println!("결과({:.1}초): {r:?}", started.elapsed().as_secs_f32());
    for wait in [0u64, 2, 5] {
        tokio::time::sleep(Duration::from_secs(wait)).await;
        let v = browser.view().await?;
        println!(
            "+{wait}초: {} · {:?} · 로그인 칸 {} · 글 {:?}",
            v.url,
            v.title,
            v.login_form,
            v.text.chars().take(400).collect::<String>()
        );
    }
    Ok(())
}

/// 브라우저에 심은 쿠키를 데몬이 빌려 HTTP로 보내는지(결정 0037). 쿠키 되울림 페이지(httpbin)를 쓴다.
async fn session_section(
    browser: &Arc<SbxBrowser>,
    egress: &Arc<Egress>,
    proxy: &Arc<Proxy>,
) -> anyhow::Result<()> {
    use crate::watch::{Fetch as _, Via};
    const SET: &str = "https://httpbin.org/cookies/set?lucia_check=ok";
    const ECHO: &str = "https://httpbin.org/cookies";
    browser.open(SET, None).await?;
    let plain = Arc::new(crate::watch::SafeFetch {
        egress: egress.clone(),
        proxy: proxy.clone(),
    });
    let store = Arc::new(std::sync::Mutex::new(lucia_store::Store::open_in_memory()?));
    let shared: Arc<dyn Browser> = browser.clone();
    let reader = crate::session::WatchReader::new(plain, Some(shared), None, store);
    let with = reader
        .fetch_via(ECHO, Via::Session)
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let without = reader
        .fetch_via(ECHO, Via::Plain)
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    check(
        with.contains("lucia_check") && !without.contains("lucia_check"),
        "로그인 세션 빌려 HTTP로 읽기(브라우저 쿠키가 데몬 요청에 실림)",
        format!(
            "세션 {} · 쿠키 없이 {}",
            with.split_whitespace().collect::<String>(),
            without.split_whitespace().collect::<String>()
        ),
    )?;
    let (page, arrived) = browser.read_page(ECHO).await?;
    check(
        page.contains("lucia_check"),
        "감시 탭으로 같은 페이지 읽기",
        arrived,
    )?;
    Ok(())
}

/// 내려받기(결정 0057): 브라우저는 받지 않고, 시작하려던 주소를 데몬에 알린다(데몬이 그 주소를 대신 받는다).
/// 시험 페이지는 VM 루프백이라 데몬이 닿지 못하므로, 여기서는 알림까지만 본다.
async fn downloads_section(browser: &SbxBrowser) -> anyhow::Result<()> {
    let file = format!("{FIXTURE}/dl/file");
    let page = browser.open(&format!("{FIXTURE}/dl-page"), None).await?;
    let _ = browser.take_downloads();
    // 링크를 누르면 화면은 그대로이고 내려받기 주소가 온다.
    let link = find(&page, "압축 파일")?.r.clone();
    let after = free_act(browser, Act::Click { r: link }).await?;
    let seen = browser.take_downloads();
    check(
        after.url.ends_with("/dl-page") && seen.len() == 1 && seen[0].url == file,
        "내려받기 링크 누르기(주소를 알림, 화면은 그대로)",
        format!("{} {seen:?}", after.url),
    )?;
    // 파일 주소를 바로 열면 브라우저는 열지 못하고, 조금 뒤 넘겨준 끝의 주소를 알린다.
    let opened = browser.open(&format!("{FIXTURE}/dl/moved"), None).await;
    tokio::time::sleep(Duration::from_millis(1200)).await;
    let seen = browser.take_downloads();
    check(
        matches!(&opened, Err(BrowserError::Failed(why)) if why.contains("ERR_ABORTED"))
            && seen.len() == 1
            && seen[0].url == file,
        "파일 주소 바로 열기(중단됨, 넘겨준 끝의 주소를 알림)",
        format!("{opened:?} {seen:?}"),
    )?;
    // PDF는 뷰어로 열린다: 읽을 글도 요소도 없고 내려받기도 아니다(도구가 빈 화면을 보고 주소를 받아 본다).
    let pdf = browser.open(&format!("{FIXTURE}/dl/doc.pdf"), None).await?;
    let seen = browser.take_downloads();
    check(
        pdf.text.trim().is_empty() && pdf.elements.is_empty() && seen.is_empty(),
        "PDF 주소 열기(뷰어, 빈 화면)",
        format!("{:?} 요소 {} {seen:?}", pdf.text, pdf.elements.len()),
    )?;
    Ok(())
}

/// 페이지 스크립트와 파일 올리기(결정 0059, `LUCIA_BROWSER_CHECK_ONLY=control`로 이것만).
async fn control_section(browser: &SbxBrowser) -> anyhow::Result<()> {
    let page = browser.open(&format!("{FIXTURE}/labels"), None).await?;
    // 스크립트: 페이지의 실행 공간에서 돌고, 돌려준 값이 온다(await도 된다).
    let (value, view) = browser
        .eval("const n = await Promise.resolve(document.querySelectorAll('form').length); return {title: document.title, forms: n}")
        .await?;
    check(
        value.contains("시험 label")
            && value.contains("\"forms\": 3")
            && view.url.ends_with("/labels"),
        "페이지 스크립트(돌려준 값)",
        value.replace('\n', " "),
    )?;
    let failed = browser.eval("return nothing.here").await;
    check(
        matches!(&failed, Err(BrowserError::Failed(why)) if why.contains("스크립트 오류")),
        "페이지 스크립트의 오류가 까닭으로 옴",
        format!("{:?}", failed.map(|(v, _)| v)),
    )?;
    // 파일 올리기: 파일 칸에 넣은 파일을 페이지가 본다(한글 이름, 바이트 그대로).
    let file = page
        .elements
        .iter()
        .find(|e| e.kind == "file")
        .ok_or_else(|| anyhow::anyhow!("파일 칸 없음"))?
        .r
        .clone();
    browser
        .upload(&file, "시험 파일.txt", "안녕 lucia".as_bytes())
        .await?;
    let (seen, _) = browser
        .eval("const f = document.getElementById('fl').files[0]; return f ? f.name + '|' + f.size + '|' + await f.text() : 'none'")
        .await?;
    check(
        seen == "시험 파일.txt|12|안녕 lucia",
        "파일 올리기(페이지가 파일을 본다)",
        seen,
    )?;
    let not_file = page
        .elements
        .iter()
        .find(|e| e.kind == "text")
        .map(|e| e.r.clone())
        .unwrap_or_default();
    let refused = browser.upload(&not_file, "x.txt", b"x").await;
    check(
        refused.is_err(),
        "파일 칸이 아닌 곳에는 올리지 않음",
        format!("{:?}", refused.map(|v| v.url)),
    )?;
    Ok(())
}

async fn frames_section(browser: &SbxBrowser) -> anyhow::Result<()> {
    let fv = browser.open(&format!("{FIXTURE}/frames"), None).await?;
    let hosts: Vec<&str> = fv.frames.iter().map(|f| f.url.as_str()).collect();
    check(
        fv.frames.len() == 2
            && fv.frames.iter().all(|f| f.text.contains("위젯 글"))
            && fv.viewport.0 > 0,
        "iframe 글과 요소 좌표(같은 사이트·다른 사이트)",
        format!("{hosts:?}, 화면 {:?}", fv.viewport),
    )?;
    let cross_comment = frame_point(&fv, "localhost:8002", "댓글")?;
    let (g, el, after) = coord(browser, cross_comment).await?;
    check(
        g == Gate::Free
            && el
                .frame
                .as_deref()
                .is_some_and(|f| f.contains("localhost:8002")),
        "다른 사이트 iframe의 입력칸을 좌표로 누름(그 안의 요소로 판정)",
        format!("{} {:?} @ {:?}", el.kind, el.label, el.frame),
    )?;
    anyhow::ensure!(after.is_some());
    let (g, _) = focus_act(
        browser,
        Act::TypeText {
            text: "안녕".into(),
            submit: false,
        },
    )
    .await?;
    let typed = browser.view().await?;
    let value = typed
        .frames
        .iter()
        .filter(|f| f.url.contains("localhost:8002"))
        .flat_map(|f| f.elements.iter())
        .find(|(_, e)| e.label == "댓글")
        .and_then(|(_, e)| e.value.clone());
    check(
        g == Gate::Free && value.as_deref() == Some("안녕"),
        "초점 칸에 글 넣기(다른 사이트 iframe 안)",
        format!("{value:?}"),
    )?;
    let (g_enter, _) = focus_act(
        browser,
        Act::Key {
            key: "enter".into(),
        },
    )
    .await?;
    let (g_tab, _) = focus_act(browser, Act::Key { key: "tab".into() }).await?;
    let (g_btn_enter, f_btn) = focus_act(
        browser,
        Act::Key {
            key: "enter".into(),
        },
    )
    .await?;
    let (g_btn_space, _) = focus_act(
        browser,
        Act::Key {
            key: "space".into(),
        },
    )
    .await?;
    check(
        approval(&g_enter)
            && g_tab == Gate::Free
            && approval(&g_btn_enter)
            && approval(&g_btn_space),
        "iframe 안 POST 폼의 Enter, 제출 단추에 초점 뒤 Enter·Space는 승인",
        format!(
            "Enter {g_enter:?} · Tab {g_tab:?} → {:?} · Enter {g_btn_enter:?} · Space {g_btn_space:?}",
            f_btn.target().label
        ),
    )?;
    let fv = browser.view().await?;
    let mut graded = Vec::new();
    for (host, label) in [
        ("localhost:8002", "댓글 등록"),
        ("localhost:8002", "보내기"),
        ("127.0.0.1:8001", "댓글 등록"),
        ("127.0.0.1:8001", "보내기"),
    ] {
        let (g, el, _) = coord(browser, frame_point(&fv, host, label)?).await?;
        graded.push((host, label, g.clone(), el.kind.clone()));
        anyhow::ensure!(approval(&g), "{host} {label}: {g:?}");
    }
    check(
        true,
        "iframe 안 POST 제출·스크립트 단추는 승인(누르지 않음)",
        format!("{graded:?}"),
    )?;
    let login_id = frame_point(&fv, "localhost:8002", "아이디")?;
    let (g_id, _, _) = coord(browser, login_id).await?;
    let (_, _) = focus_act(browser, Act::Key { key: "tab".into() }).await?;
    let (g_pw, f_pw) = focus_act(
        browser,
        Act::TypeText {
            text: "hunter22".into(),
            submit: false,
        },
    )
    .await?;
    check(
        g_id == Gate::Free && matches!(g_pw, Gate::HumanOnly(_)),
        "iframe 안 비밀번호 칸은 사람이 대화 중일 때만(결정 0059)",
        format!("{} → {g_pw:?}", f_pw.target().kind),
    )?;
    let mut traps = Vec::new();
    for (y, what) in [
        (315, "투명 제출 단추"),
        (365, "pointer-events:none 미끼"),
        (415, "리스너만 단 div"),
    ] {
        let (g, el, _) = coord(browser, Point { x: 100, y }).await?;
        traps.push(format!("{what} → {} {:?} {g:?}", el.kind, el.label));
        anyhow::ensure!(approval(&g), "{what}: {g:?}");
    }
    check(
        true,
        "좌표 아래 실제 요소로 판정(투명 단추·미끼·리스너)",
        traps.join(" / "),
    )?;
    browser.open(&format!("{FIXTURE}/moving"), None).await?;
    let moving = Point { x: 100, y: 35 };
    let hit = browser.hit(moving).await?;
    let before = hit.target();
    tokio::time::sleep(Duration::from_secs(5)).await;
    let r = browser
        .act(
            &Act::ClickAt { at: moving },
            Some(&Expect {
                url: hit.url.clone(),
                element: before.clone(),
                approved: true,
            }),
        )
        .await;
    check(
        approval(&classify_hit(&hit)) && matches!(r, Err(BrowserError::Stale(_))),
        "승인을 기다리는 사이 바뀐 단추는 누르지 않음",
        format!("{:?} → {:?}", before.label, r.err()),
    )?;
    let fv = browser.open(&format!("{FIXTURE}/frames"), None).await?;
    let y0 = frame_point(&fv, "localhost:8002", "보내기")?.y;
    browser
        .act(
            &Act::Scroll {
                to: super::Scroll::Down,
                at: Some(Point { x: 600, y: 150 }),
            },
            None,
        )
        .await?;
    let scrolled = browser.view().await?;
    let y1 = frame_point(&scrolled, "localhost:8002", "보내기")
        .map(|p| p.y)
        .ok();
    check(
        y1.is_none_or(|y| y < y0),
        "iframe 안 휠 스크롤",
        format!("보내기 y {y0} → {y1:?}"),
    )?;
    // label은 연결된 컨트롤로 판정한다.
    browser.open(&format!("{FIXTURE}/labels"), None).await?;
    let (g_label, el_label, _) = coord(browser, Point { x: 60, y: 85 }).await?;
    let (g_file, el_file, _) = coord(browser, Point { x: 60, y: 135 }).await?;
    check(
        approval(&g_label) && matches!(g_file, Gate::Forbidden(_)),
        "label은 연결된 컨트롤로 판정(POST 단추 → 승인, 파일 칸 → 못 함)",
        format!(
            "{} {:?} {g_label:?} / {} {g_file:?}",
            el_label.kind, el_label.label, el_file.kind
        ),
    )?;
    // 폼 안의 체크·선택 상자에서 Enter(암묵적 제출)는 폼 판단.
    let (g_t, _, _) = coord(browser, Point { x: 60, y: 185 }).await?;
    focus_act(browser, Act::Key { key: "tab".into() }).await?;
    let (g_ck, f_ck) = focus_act(
        browser,
        Act::Key {
            key: "enter".into(),
        },
    )
    .await?;
    focus_act(browser, Act::Key { key: "tab".into() }).await?;
    let (g_sel, f_sel) = focus_act(
        browser,
        Act::Key {
            key: "enter".into(),
        },
    )
    .await?;
    check(
        g_t == Gate::Free && approval(&g_ck) && approval(&g_sel),
        "POST 폼 안 체크·선택 상자의 Enter는 승인",
        format!(
            "{} {g_ck:?} / {} {g_sel:?}",
            f_ck.target().kind,
            f_sel.target().kind
        ),
    )?;
    // 참고: Chrome이 체크 칸의 Enter로 폼을 제출하는지(GET 폼으로 본다, 실패로 치지 않는다).
    coord(browser, Point { x: 60, y: 375 }).await?;
    focus_act(browser, Act::Key { key: "tab".into() }).await?;
    let (_, f) = browser.focused().await?;
    let after = browser
        .act(
            &Act::Key {
                key: "enter".into(),
            },
            Some(&Expect {
                url: format!("{FIXTURE}/labels"),
                element: f.target(),
                approved: true,
            }),
        )
        .await?;
    println!(
        "  참고: 체크 칸에서 Enter → {}",
        if after.url.contains("/search") {
            "폼 제출됨(암묵적 제출)"
        } else {
            "제출 안 됨"
        }
    );
    // 스스로 다시 뜨는 iframe이 있어도 참조 클릭은 본 페이지 이동을 기다린다.
    let rv = browser.open(&format!("{FIXTURE}/reloading"), None).await?;
    let next = find(&rv, "다음 쪽")?.r.clone();
    let started = std::time::Instant::now();
    let page2 = free_act(browser, Act::Click { r: next }).await?;
    check(
        page2.text.contains("둘째") && started.elapsed() < Duration::from_secs(8),
        "다시 뜨는 iframe이 있어도 링크 이동을 기다림",
        format!("{:?} · {:.1}초", page2.url, started.elapsed().as_secs_f32()),
    )?;

    let n = posts(browser).await?;
    check(
        n.contains("POST 0"),
        "좌표·iframe 시험 동안 승인 없이 나간 POST 없음",
        n,
    )?;
    Ok(())
}

async fn posts(browser: &SbxBrowser) -> anyhow::Result<String> {
    let v = browser.open(&format!("{FIXTURE}/count"), None).await?;
    Ok(v.text)
}

pub async fn run(cfg: &Config, skip_site: bool) -> anyhow::Result<()> {
    let cancel = CancellationToken::new();
    let egress = Arc::new(Egress::default().with_dev_origin(FIXTURE));
    let proxy = Proxy::system(egress.clone());
    proxy
        .clone()
        .serve(cfg.browser.proxy_bind, cancel.clone())
        .await?;
    let browser = SbxBrowser::new(
        SbxConfig {
            sbx: cfg.sandbox.sbx.clone(),
            sandbox: cfg.browser.sandbox.clone(),
            idle: Duration::from_secs(600),
            proxy: format!(
                "socks5://{}:{}",
                cfg.mcp.host_alias,
                cfg.browser.proxy_bind.port()
            ),
        },
        egress.clone(),
    );
    let sites = select(&cfg.browser.sites).map_err(|e| anyhow::anyhow!(e))?;
    if !skip_site {
        let first = format!("{}/", sites[0].origin);
        egress.check_url(&first).map_err(|e| anyhow::anyhow!(e))?;
        let page = browser.open(&first, Some(&sites[0])).await?;
        check(
            !page.title.is_empty() && !page.text.is_empty(),
            "사이트 읽기(프록시 경유)",
            format!(
                "{} · 제목 {:?} · 글 {}자 · 요소 {}개 · 로그인 칸 {} · iframe {}개(모델에게 가는 화면 글 {}자)",
                page.url,
                page.title,
                page.text.chars().count(),
                page.element_total,
                page.login_form,
                page.frames.len(),
                super::tools::format_view(&page, false).chars().count()
            ),
        )?;
        let full = super::tools::format_view(&page, false).chars().count();
        let bare = super::tools::format_view(
            &PageView {
                frames: Vec::new(),
                ..page.clone()
            },
            false,
        )
        .chars()
        .count();
        check(
            true,
            "사이트 iframe 몫",
            format!(
                "{}자 · {:?}",
                full - bare,
                page.frames
                    .iter()
                    .map(|f| (
                        f.url.chars().take(60).collect::<String>(),
                        f.rect,
                        f.text.chars().count(),
                        f.elements.len()
                    ))
                    .collect::<Vec<_>>()
            ),
        )?;
    }

    let sandbox = cfg.browser.sandbox.as_str();
    sbx(
        cfg,
        &[
            "exec",
            "-i",
            sandbox,
            "sh",
            "-c",
            "cat > /tmp/lucia-fixture-login.py",
        ],
        Some(FIXTURE_SERVER),
    )
    .await?;
    sbx(
        cfg,
        &[
            "exec",
            sandbox,
            "sh",
            "-c",
            // pkill -f는 이 셸 자신의 명령줄에도 맞으므로 PID 파일로 다룬다.
            "[ -f /tmp/lucia-fixture.pid ] && kill $(cat /tmp/lucia-fixture.pid) 2>/dev/null; \
             setsid nohup python3 /tmp/lucia-fixture-login.py >/tmp/lucia-fixture.log 2>&1 </dev/null & \
             echo $! > /tmp/lucia-fixture.pid; sleep 1",
        ],
        None,
    )
    .await?;
    // 로그인 화면 살펴보기(개발용): LUCIA_BROWSER_CHECK_ONLY=probe LUCIA_PROBE_URL=<로그인 페이지>.
    // 가짜 아이디·비밀번호로 한 번 넣어 보고, 제출 뒤 화면(주소·제목·비밀번호 칸·글 앞부분)을 시간에 따라 보인다.
    // 실제 계정은 쓰지 않는다.
    if std::env::var("LUCIA_BROWSER_CHECK_ONLY").as_deref() == Ok("probe") {
        let r = probe_login(&browser, &egress).await;
        browser.shutdown().await;
        cancel.cancel();
        return r;
    }
    // 로그인 세션 빌려 읽기(결정 0037): 공개 쿠키 되울림 페이지로 계정 없이 본다. 바깥 사이트를 쓰므로
    // --skip-site면 건너뛴다(LUCIA_BROWSER_CHECK_ONLY=session이면 이것만).
    let only = std::env::var("LUCIA_BROWSER_CHECK_ONLY").unwrap_or_default();
    if !skip_site || only == "session" {
        let r = session_section(&browser, &egress, &proxy).await;
        if only == "session" || r.is_err() {
            browser.shutdown().await;
            cancel.cancel();
            return r;
        }
    }
    // 스크립트와 파일 올리기(결정 0059): LUCIA_BROWSER_CHECK_ONLY=control 이면 이것만 돈다.
    if only == "control" {
        let r = control_section(&browser).await;
        browser.shutdown().await;
        cancel.cancel();
        return r;
    }
    // 내려받기(결정 0057): LUCIA_BROWSER_CHECK_ONLY=downloads 면 이것만 돈다.
    if only == "downloads" {
        let r = downloads_section(&browser).await;
        browser.shutdown().await;
        cancel.cancel();
        return r;
    }
    // 개발 편의: LUCIA_BROWSER_CHECK_ONLY=frames 면 좌표·iframe 부분만 돈다.
    if only == "frames" {
        let r = frames_section(&browser).await;
        browser.shutdown().await;
        cancel.cancel();
        return r;
    }
    let site = fixture_site();

    // ----- 로그인 -----
    let _ = browser.open(&format!("{FIXTURE}/logout"), None).await;
    let r = browser.login(&site, "", "x").await?;
    check(
        matches!(&r, LoginResult::Rejected(why) if why.contains("아이디를 입력하세요")),
        "빈 아이디(대화상자 자동 닫기)",
        format!("{r:?}"),
    )?;
    let r = browser.login(&site, FIXTURE_USER, "wrong pw").await?;
    check(
        matches!(&r, LoginResult::Rejected(why) if why.contains("다릅니다") && !why.contains("wrong pw")),
        "틀린 비밀번호(사이트 문구, 입력값 없음)",
        format!("{r:?}"),
    )?;
    let r = browser.login(&site, FIXTURE_USER, FIXTURE_PASSWORD).await?;
    check(
        r == LoginResult::LoggedIn,
        "맞는 비밀번호(공백 포함, 로그인 유지 체크)",
        format!("{r:?}"),
    )?;
    let r = browser.login(&site, FIXTURE_USER, FIXTURE_PASSWORD).await?;
    check(
        r == LoginResult::AlreadyLoggedIn,
        "다시 로그인",
        format!("{r:?}"),
    )?;
    let home = browser.open(&format!("{FIXTURE}/"), None).await?;
    check(
        home.text.contains("로그인됨") && !home.login_form,
        "로그인된 페이지 읽기",
        format!("{:?}", home.text),
    )?;
    browser.shutdown().await;
    let again = browser.open(&format!("{FIXTURE}/"), None).await?;
    check(
        again.text.contains("로그인됨"),
        "브라우저를 닫았다 다시 열어도 로그인 유지",
        format!("{:?}", again.text),
    )?;

    // ----- 로그인 게시판 감시(결정 0036): 세션 쿠키 꺼내기, 감시 탭 읽기(대화용 탭은 그대로) -----
    let jar = browser.cookies(&format!("{FIXTURE}/")).await?;
    check(
        jar.cookies
            .iter()
            .any(|(k, v)| k == "lucia_fixture" && v == "ok")
            && jar.user_agent.contains("Chrome"),
        "로그인 세션 쿠키 꺼내기",
        format!(
            "쿠키 {}개 · 사용자 에이전트 {}자",
            jar.cookies.len(),
            jar.user_agent.len()
        ),
    )?;
    let before = browser.view().await?.url;
    let (html, arrived) = browser.read_page(&format!("{FIXTURE}/page2")).await?;
    let after = browser.view().await?.url;
    check(
        html.contains("둘째") && arrived.ends_with("/page2") && before == after,
        "감시 탭에서 읽기(대화용 탭은 그대로)",
        format!("{arrived} · 대화용 탭 {before} → {after}"),
    )?;
    let (html, _) = browser.read_page(&format!("{FIXTURE}/")).await?;
    check(
        html.contains("로그인됨"),
        "감시 탭도 로그인된 세션(같은 탭을 다시 씀)",
        html.chars().take(80).collect::<String>(),
    )?;

    let generic = fixture_generic();
    let _ = browser.open(&format!("{FIXTURE}/logout"), None).await;
    let r = browser.login(&generic, FIXTURE_USER, "wrong pw").await?;
    check(
        matches!(&r, LoginResult::Rejected(why) if !why.contains("wrong pw")),
        "일반 프로필: 틀린 비밀번호",
        format!("{r:?}"),
    )?;
    let r = browser
        .login(&generic, FIXTURE_USER, FIXTURE_PASSWORD)
        .await?;
    check(
        r == LoginResult::LoggedIn,
        "일반 프로필: 칸 찾기 → 로그인(유지 체크 포함)",
        format!("{r:?}"),
    )?;

    // 이미지 로그인 단추(form.elements에 없다)와 제출 검사 스크립트, 단추 없이 Enter: 넣은 비밀번호를 그대로 둔 채
    // 제출해야 한다(2026-11-18 운영: Enter 전에 칸을 비워 "비밀번호를 입력하세요"로 실패했다).
    for (path, what) in [
        (
            "/login11",
            "일반 프로필: 이미지 로그인 단추 + 제출 검사 스크립트",
        ),
        (
            "/login12",
            "일반 프로필: 단추 없이 Enter(넣은 비밀번호를 지우지 않음)",
        ),
    ] {
        let _ = browser.open(&format!("{FIXTURE}/logout"), None).await;
        let r = browser
            .login(&fixture_generic_at(path), FIXTURE_USER, FIXTURE_PASSWORD)
            .await?;
        check(r == LoginResult::LoggedIn, what, format!("{r:?}"))?;
    }

    let _ = browser.open(&format!("{FIXTURE}/logout"), None).await;
    let r = browser
        .login(
            &fixture_generic_at("/login13"),
            FIXTURE_USER,
            FIXTURE_PASSWORD,
        )
        .await?;
    check(
        matches!(&r, LoginResult::NeedsHuman { why } if why.contains("추가 보안")),
        "일반 프로필: 비밀번호 뒤 사람 확인(캡차)은 성공이 아니라 사람에게 넘김",
        format!("{r:?}"),
    )?;

    // 에디터의 숨은 편집 iframe: 초점을 편집 칸으로 읽고, Enter는 줄바꿈이라 묻지 않는다.
    browser.open(&format!("{FIXTURE}/editor"), None).await?;
    let (url, focus) = browser.focused().await?;
    let gate = classify_focus(
        &Act::Key {
            key: "enter".into(),
        },
        &url,
        &focus,
    );
    check(
        matches!(&focus, Focus::On { element } if element.kind == "editable") && gate == Gate::Free,
        "에디터의 숨은 편집 iframe 초점 읽기 → Enter는 그냥",
        format!("{focus:?} → {gate:?}"),
    )?;

    // ----- 단계형 로그인(결정 0017) -----
    let _ = browser.open(&format!("{FIXTURE}/logout"), None).await;
    let r = browser
        .login(
            &fixture_generic_at("/login3"),
            FIXTURE_USER,
            FIXTURE_PASSWORD,
        )
        .await?;
    check(
        r == LoginResult::LoggedIn,
        "단계형(한 화면): 아이디 → 다음 → 비밀번호 칸이 나오면 넣기",
        format!("{r:?}"),
    )?;
    let r = browser
        .login(
            &fixture_generic_at("/login3"),
            FIXTURE_USER,
            FIXTURE_PASSWORD,
        )
        .await?;
    let search = browser.view().await?;
    check(
        r == LoginResult::AlreadyLoggedIn
            && search.url.ends_with("/act")
            && !search
                .elements
                .iter()
                .any(|e| e.value.as_deref().is_some_and(|v| !v.is_empty())),
        "이미 로그인: 넘어간 페이지의 검색 칸은 건드리지 않음",
        format!("{r:?} {}", search.url),
    )?;
    let _ = browser.open(&format!("{FIXTURE}/logout"), None).await;
    let r = browser
        .login(
            &fixture_generic_at("/login4"),
            FIXTURE_USER,
            FIXTURE_PASSWORD,
        )
        .await?;
    check(
        r == LoginResult::LoggedIn,
        "단계형(두 페이지): 아이디 제출 뒤 다음 페이지의 비밀번호",
        format!("{r:?}"),
    )?;
    let _ = browser.open(&format!("{FIXTURE}/logout"), None).await;
    let r = browser
        .login(
            &fixture_generic_at("/login5"),
            FIXTURE_USER,
            FIXTURE_PASSWORD,
        )
        .await;
    check(
        matches!(&r, Err(BrowserError::NotAllowed(why)) if why.contains("다른 곳")),
        "단계형: 다음 단계가 다른 곳으로 넘어가면 비밀번호를 넣지 않음",
        format!("{r:?}"),
    )?;
    let r = browser
        .login(
            &fixture_generic_at("/login6"),
            FIXTURE_USER,
            FIXTURE_PASSWORD,
        )
        .await;
    let left = browser.view().await?;
    check(
        matches!(&r, Err(BrowserError::Failed(why)) if why.contains("비밀번호 칸이 나오지 않았다"))
            && !left
                .elements
                .iter()
                .any(|e| e.value.as_deref() == Some(FIXTURE_USER)),
        "단계형: 비밀번호 칸이 안 나오면 시간 안에 멈추고 칸을 비움",
        format!("{r:?}"),
    )?;
    // ----- 인증 코드(결정 0017) -----
    let _ = browser.open(&format!("{FIXTURE}/logout"), None).await;
    let otp_site = fixture_generic_at("/login8");
    let r = browser
        .login(&otp_site, FIXTURE_USER, FIXTURE_PASSWORD)
        .await?;
    check(
        matches!(&r, LoginResult::NeedsCode { hint } if hint.contains("인증")),
        "인증 코드: 비밀번호 뒤 코드 칸을 찾고 그 단계에 둠",
        format!("{r:?}"),
    )?;
    let r = browser.submit_code(&otp_site, "000000").await?;
    check(
        matches!(&r, LoginResult::Rejected(why) if !why.contains("000000")),
        "인증 코드: 틀린 코드는 사이트 문구로 거절(코드는 문구에 없음)",
        format!("{r:?}"),
    )?;
    let r = browser.submit_code(&otp_site, "424242").await?;
    let home = browser.open(&format!("{FIXTURE}/"), None).await?;
    check(
        r == LoginResult::LoggedIn && home.text.contains("로그인됨"),
        "인증 코드: 맞는 코드로 로그인",
        format!("{r:?}"),
    )?;
    let r = browser.submit_code(&otp_site, "424242").await;
    check(
        r.is_err(),
        "인증 코드: 코드 칸이 없으면 넣지 않음",
        format!("{r:?}"),
    )?;

    // 뒤 검사를 위해 다시 로그인해 둔다.
    let _ = browser
        .login(&generic, FIXTURE_USER, FIXTURE_PASSWORD)
        .await?;

    // ----- 화면 넘겨받기(결정 0017) -----
    {
        use super::HandInput;
        let _ = browser.open(&format!("{FIXTURE}/popup"), None).await?;
        browser.set_handover(true).await?;
        let tabs = browser.handover_tabs().await?;
        let main = tabs[0].id.clone();
        let frame = browser.handover_frame(&main).await?;
        check(
            frame.jpeg.starts_with(&[0xFF, 0xD8]),
            "넘겨받기: 화면을 JPEG로 받음",
            format!("{} 바이트", frame.jpeg.len()),
        )?;
        // 시험 페이지의 단추는 왼쪽 위 200×40에 있다.
        browser
            .handover_input(&main, &HandInput::Click { x: 100, y: 20 })
            .await?;
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let tabs = browser.handover_tabs().await?;
        check(
            tabs.len() == 2 && tabs.iter().any(|t| t.url.ends_with("/page2")),
            "넘겨받기: 누른 자리로 새 창이 열리고 닫지 않고 탭으로 남김",
            format!("{tabs:?}"),
        )?;
        let popup = tabs.iter().find(|t| t.id != main).unwrap().id.clone();
        check(
            browser.handover_frame(&popup).await?.jpeg.len() > 100,
            "넘겨받기: 새 창 화면도 받음",
            String::new(),
        )?;
        browser.set_handover(false).await?;
        let tabs = browser.handover_tabs().await?;
        check(
            tabs.len() == 1,
            "넘겨받기를 끝내면 새 창을 닫음",
            format!("{tabs:?}"),
        )?;
        browser.set_handover(true).await?;
        let r = browser
            .handover_input(&main, &HandInput::Text("x".into()))
            .await;
        browser.set_handover(false).await?;
        let _ = browser
            .handover_input(&main, &HandInput::Key("enter".into()))
            .await
            .err()
            .ok_or_else(|| anyhow::anyhow!("넘겨받지 않은 상태의 입력이 통과했다"))?;
        check(r.is_ok(), "넘겨받지 않으면 입력을 받지 않음", String::new())?;
    }

    // ----- 화면 동작 -----
    let view = browser.open(&format!("{FIXTURE}/act"), None).await?;
    let blocked_img = proxy
        .recent_blocks(20)
        .iter()
        .any(|d| d.host == "10.1.2.3.nip.io");
    check(
        blocked_img,
        "하위 요청: 사설 주소로 풀리는 이미지를 프록시가 막음",
        format!("{:?}", proxy.recent_blocks(3)),
    )?;
    let url = view.url.clone();
    let gate_of = |label: &str, act: fn(String) -> Act| -> anyhow::Result<Gate> {
        let el = find(&view, label)?;
        Ok(classify(&act(el.r.clone()), el, &url))
    };
    let click = |r: String| Act::Click { r };
    let grades = [
        ("다음 쪽", gate_of("다음 쪽", click)?),
        ("펼쳐 보기", gate_of("펼쳐 보기", click)?),
        ("찾기", gate_of("찾기", click)?),
        ("스크립트 보내기", gate_of("스크립트 보내기", click)?),
        ("글 등록", gate_of("글 등록", click)?),
        ("로그아웃", gate_of("로그아웃", click)?),
    ];
    check(
        matches!(grades[0].1, Gate::Free)
            && matches!(grades[1].1, Gate::Free)
            && matches!(grades[2].1, Gate::Free)
            && matches!(grades[3].1, Gate::Approval(_) | Gate::Publish(_))
            && matches!(grades[4].1, Gate::Publish(_))
            && matches!(grades[5].1, Gate::HumanOnly(_)),
        "요소 등급(링크·펼치기·GET은 그냥, 스크립트·POST는 승인, 로그아웃은 사람이 대화 중일 때만)",
        format!("{grades:?}"),
    )?;

    let more = find(&view, "펼쳐 보기")?.r.clone();
    let after = free_act(&browser, Act::Click { r: more }).await?;
    check(
        after.text.contains("펼친 글"),
        "펼치기(실제 마우스 클릭)",
        format!("{:?}", after.text.lines().take(4).collect::<Vec<_>>()),
    )?;

    // 승인 없이 confirm을 띄우는 스크립트 단추를 눌렀다고 해도 확인은 취소된다(POST 없음).
    let js = find(&after, "스크립트 보내기")?.clone();
    let (u, el) = browser.inspect(&js.r).await?;
    let pressed = browser
        .act(
            &Act::Click { r: js.r.clone() },
            Some(&Expect {
                url: u,
                element: el,
                approved: false,
            }),
        )
        .await?;
    check(
        pressed.dialogs.iter().any(|d| d.contains("취소함")),
        "승인 밖의 확인(confirm) 대화상자는 취소",
        format!("{:?}", pressed.dialogs),
    )?;

    let search = find(&pressed, "검색어")?.r.clone();
    let found = free_act(
        &browser,
        Act::Type {
            r: search,
            text: "고양이".into(),
            submit: true,
        },
    )
    .await?;
    check(
        found.text.contains("검색: 고양이"),
        "검색 칸에 넣고 Enter(GET 폼)",
        format!("{} {:?}", found.url, found.text),
    )?;
    let back = browser.act(&Act::Back, None).await?;
    check(back.url.ends_with("/act"), "뒤로", back.url.clone())?;

    let covered = find(&back, "가려진 단추")?.r.clone();
    let (u, el) = browser.inspect(&covered).await?;
    let r = browser
        .act(
            &Act::Click { r: covered },
            Some(&Expect {
                url: u,
                element: el,
                approved: false,
            }),
        )
        .await;
    check(
        matches!(&r, Err(BrowserError::Failed(why)) if why.contains("가려")),
        "가려진 요소는 누르지 않음",
        format!("{:?}", r.as_ref().err()),
    )?;

    let stale = find(&back, "글 등록")?.r.clone();
    let next = find(&back, "다음 쪽")?.r.clone();
    let page2 = free_act(&browser, Act::Click { r: next }).await?;
    let r = browser.inspect(&stale).await;
    check(
        page2.text.contains("둘째") && matches!(r, Err(BrowserError::Stale(_))),
        "링크 이동 뒤 낡은 참조는 쓰지 않음",
        format!("{:?}", r.err()),
    )?;

    let n = posts(&browser).await?;
    check(n.contains("POST 0"), "승인 없이 나간 POST 없음", n)?;

    frames_section(&browser).await?;
    downloads_section(&browser).await?;
    control_section(&browser).await?;

    // 승인된 POST: 제목을 넣고(그냥) 등록을 누른다(승인 흐름처럼 같은 요소·같은 값을 다시 확인).
    let form = browser.open(&format!("{FIXTURE}/act"), None).await?;
    let title = find(&form, "제목")?.r.clone();
    let typed = free_act(
        &browser,
        Act::Type {
            r: title,
            text: "안녕".into(),
            submit: false,
        },
    )
    .await?;
    let submit = find(&typed, "글 등록")?.r.clone();
    let (u, el) = browser.inspect(&submit).await?;
    let fields = el
        .form
        .as_ref()
        .map(|f| f.fields.clone())
        .unwrap_or_default();
    let done = browser
        .act(
            &Act::Click { r: submit },
            Some(&Expect {
                url: u,
                element: el,
                approved: true,
            }),
        )
        .await?;
    check(
        done.text.contains("POST 1"),
        "승인된 POST 폼 제출(폼 값 확인 뒤)",
        format!("폼 값 {fields:?} → {:?}", done.text.lines().last()),
    )?;

    let shot = browser.screenshot(false).await?;
    check(
        shot.jpeg.starts_with(&[0xFF, 0xD8, 0xFF]) && shot.jpeg.len() > 1000,
        "스크린샷",
        format!("{} 바이트", shot.jpeg.len()),
    )?;
    let full = browser.screenshot(true).await?;
    check(
        full.jpeg.len() > 1000,
        "전체 페이지 스크린샷",
        format!("{} 바이트", full.jpeg.len()),
    )?;

    let r = browser.open(&format!("{FIXTURE}/redir"), None).await;
    check(
        r.is_err()
            && proxy
                .recent_blocks(20)
                .iter()
                .any(|d| d.host == "192.168.0.250.nip.io"),
        "사설 주소로 풀리는 이름으로 넘겨주기는 막힘",
        format!("{:?}", r.err()),
    )?;

    browser.shutdown().await;
    let _ = sbx(
        cfg,
        &[
            "exec",
            sandbox,
            "sh",
            "-c",
            "kill $(cat /tmp/lucia-fixture.pid) 2>/dev/null; rm -f /tmp/lucia-fixture.pid",
        ],
        None,
    )
    .await;
    cancel.cancel();
    println!("브라우저 점검 끝(실제 계정 로그인은 하지 않았다)");
    Ok(())
}
