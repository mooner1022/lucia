// 화면 배치 검사(실제 브라우저 안에서 잰다). check.mjs(실제 화면)와 selftest.mjs(검출기 자체 시험)가 같이 쓴다.
//
// 이 파일이 잡으려는 것은 화면이 "격자가 깨진다", "모바일에서 글이 겹친다"로 보이는 모든 경우다.
//   overflow-x      문서가 가로로 뷰포트보다 넓다(가로 스크롤이 생긴다)
//   outside         보이는 요소가 뷰포트 좌우 밖으로 나간다
//   overlap         서로 다른 글 조각이 겹쳐 그려진다
//   control-overlap 단추·링크·입력 같은 조작 요소가 서로 겹친다
//   clipped         overflow가 잘라서 글이 잘려 보인다(말줄임표는 의도된 줄임이라 뺀다)
//   tiny-target     터치 화면에서 조작 요소가 24x24 CSS px보다 작다(경고, 실패로 치지 않는다)
// "console"(콘솔 오류·실패한 요청·CSP 위반)은 페이지 안에서 잴 수 없어 check.mjs의 collectDiagnostics가 모은다.
//
// 각 발견은 { check, selector, text, rect, other?, detail? }이다. rect는 문서 좌표(스크롤을 더한 값)의 {x,y,w,h}.

export const WARNING_CHECKS = new Set(['tiny-target']);

/** 이 발견이 실패로 치는 것인가(경고가 아닌가). */
export function isFailure(finding) {
  return !WARNING_CHECKS.has(finding.check);
}

/**
 * 페이지 안에서 도는 함수. 바깥 변수를 쓰지 않는다(Playwright가 소스 글로 보낸다).
 * options: { touch: 터치 화면인가, width: 기대하는 뷰포트 너비(없으면 innerWidth), ignoreFloating: false면 고정·붙는 상자도 겹침으로 센다 }
 */
function inPage(options) {
  const MAX = { outside: 20, overlap: 30, 'control-overlap': 20, clipped: 20, 'tiny-target': 30 };
  const root = document.documentElement;
  const vw = options.width || window.innerWidth;
  const sx = window.scrollX;
  const sy = window.scrollY;
  const findings = [];
  const counts = {};

  // ---------- 바탕 도구 ----------
  const styles = new WeakMap();
  const style = (el) => {
    let s = styles.get(el);
    if (!s) {
      s = getComputedStyle(el);
      styles.set(el, s);
    }
    return s;
  };

  const goodClass = (c) => c && !/^svelte-/.test(c);
  function part(el) {
    let s = el.tagName.toLowerCase();
    if (el.id) return `${s}#${el.id}`;
    const cls = [...el.classList].filter(goodClass).slice(0, 2);
    if (cls.length > 0) return `${s}.${cls.join('.')}`;
    const parent = el.parentElement;
    if (parent) {
      const same = [...parent.children].filter((c) => c.tagName === el.tagName);
      if (same.length > 1) s += `:nth-of-type(${same.indexOf(el) + 1})`;
    }
    return s;
  }
  function selectorOf(el) {
    const parts = [];
    let cur = el;
    for (let i = 0; cur && cur.nodeType === 1 && i < 4; i++) {
      parts.unshift(part(cur));
      if (cur.id || cur === document.body) break;
      cur = cur.parentElement;
    }
    return parts.join(' > ');
  }
  function textOf(el) {
    const raw = el.innerText || el.textContent || el.getAttribute('aria-label') || el.getAttribute('placeholder') || el.value || '';
    return String(raw).replace(/\s+/g, ' ').trim().slice(0, 60);
  }
  const round = (n) => Math.round(n);
  const docRect = (r) => ({ x: round(r.x0 + sx), y: round(r.y0 + sy), w: round(r.x1 - r.x0), h: round(r.y1 - r.y0) });
  const fromDOMRect = (r) => ({ x0: r.left, y0: r.top, x1: r.right, y1: r.bottom });
  function intersect(a, b) {
    const r = { x0: Math.max(a.x0, b.x0), y0: Math.max(a.y0, b.y0), x1: Math.min(a.x1, b.x1), y1: Math.min(a.y1, b.y1) };
    return r.x1 > r.x0 && r.y1 > r.y0 ? r : null;
  }
  function add(check, el, rect, extra = {}) {
    counts[check] = (counts[check] ?? 0) + 1;
    if (counts[check] > (MAX[check] ?? 50)) return;
    findings.push({ check, selector: selectorOf(el), text: textOf(el), rect: docRect(rect), ...extra });
  }

  // ---------- 보이는가 ----------
  const VIS = { checkOpacity: true, checkVisibilityCSS: true, opacityProperty: true, visibilityProperty: true };
  const firstSummary = (details) => [...details.children].find((c) => c.tagName === 'SUMMARY');
  // 구조로 숨은 것: 시각적으로만 숨긴 글(.sr-only, clip-path 전체), 닫힌 details의 속, 열리지 않은 dialog.
  function hiddenByStructure(el) {
    for (let cur = el; cur && cur !== root; cur = cur.parentElement) {
      if (cur.classList.contains('sr-only')) return true;
      if (/^inset\(50%/.test(style(cur).clipPath)) return true;
      if (cur.tagName === 'DIALOG' && !cur.open) return true;
      const parent = cur.parentElement;
      if (parent && parent.tagName === 'DETAILS' && !parent.open && cur !== firstSummary(parent)) return true;
    }
    return false;
  }
  const visibleCache = new WeakMap();
  function visible(el) {
    let v = visibleCache.get(el);
    if (v === undefined) {
      v = el.checkVisibility(VIS) && !hiddenByStructure(el);
      visibleCache.set(el, v);
    }
    return v;
  }

  // ---------- 잘림(조상의 overflow) ----------
  const isContainingBlock = (el) => {
    const cs = style(el);
    return (
      cs.position !== 'static' ||
      cs.transform !== 'none' ||
      cs.filter !== 'none' ||
      cs.perspective !== 'none' ||
      /paint|layout|strict|content/.test(cs.contain) ||
      cs.willChange.includes('transform')
    );
  };
  const clipsContent = (el) => {
    const cs = style(el);
    if (cs.display === 'inline' || cs.display === 'contents') return false;
    return [cs.overflowX, cs.overflowY].some((v) => v === 'hidden' || v === 'scroll' || v === 'auto' || v === 'clip');
  };
  /**
   * 사각형을 조상의 스크롤 상자(overflow가 hidden|scroll|auto|clip)로 잘라 남는 부분을 돌려준다(없으면 null).
   * start가 글을 담은 요소면 includeSelf=true(자기 overflow도 그 글을 자른다), 요소 자신의 상자면 false.
   * 절대 위치 요소는 위치 기준 조상까지, 고정 위치 요소는 어느 조상에도 잘리지 않는다. stop은 거기서 멈춘다(자기 자신은 안 자른다).
   */
  function clipRect(rect, start, includeSelf, stop) {
    let cur = start;
    let first = true;
    while (cur && cur !== root && cur !== document.body && cur !== stop) {
      if ((includeSelf || !first) && clipsContent(cur)) {
        const b = cur.getBoundingClientRect();
        const x0 = b.left + cur.clientLeft;
        const y0 = b.top + cur.clientTop;
        rect = intersect(rect, { x0, y0, x1: x0 + cur.clientWidth, y1: y0 + cur.clientHeight });
        if (!rect) return null;
      }
      first = false;
      const pos = style(cur).position;
      if (pos === 'fixed') break;
      let next = cur.parentElement;
      if (pos === 'absolute') while (next && next !== root && !isContainingBlock(next)) next = next.parentElement;
      cur = next;
    }
    return rect;
  }

  // 화면에 떠 있는 상자(고정·붙는 막대, 열린 dialog): 스크롤하는 본문과 겹쳐 보여도 정상이다.
  const floatCache = new WeakMap();
  function floatOf(el) {
    if (!el) return null;
    if (floatCache.has(el)) return floatCache.get(el);
    const pos = style(el).position;
    const own = pos === 'fixed' || pos === 'sticky' || (el.tagName === 'DIALOG' && el.open);
    const result = own ? el : floatOf(el.parentElement);
    floatCache.set(el, result);
    return result;
  }
  const sameFloat = (a, b) => options.ignoreFloating === false || a.float === b.float;

  // 가로로 훑는 쌍 찾기: y로 정렬한 뒤 아직 끝나지 않은 것하고만 비교한다.
  function sweepPairs(items, test) {
    const sorted = items.slice().sort((a, b) => a.r.y0 - b.r.y0 || a.r.x0 - b.r.x0);
    let active = [];
    for (const item of sorted) {
      active = active.filter((a) => a.r.y1 > item.r.y0);
      for (const a of active) test(a, item);
      active.push(item);
    }
  }

  // ---------- 글 조각 모으기 ----------
  const textRange = document.createRange();
  function textRects(node, el, stop) {
    textRange.selectNodeContents(node);
    const out = [];
    for (const rc of textRange.getClientRects()) {
      if (rc.width < 2 || rc.height < 2) continue;
      const r = clipRect(fromDOMRect(rc), el, true, stop);
      if (r && r.x1 - r.x0 >= 2 && r.y1 - r.y0 >= 2) out.push(r);
    }
    return out;
  }
  const SKIP_TEXT = 'script,style,noscript,template,textarea,select,option';
  const texts = [];
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    if (!/\S/.test(node.data)) continue;
    const el = node.parentElement;
    if (!el || el.closest(SKIP_TEXT) || !visible(el)) continue;
    for (const r of textRects(node, el, null)) texts.push({ node, el, r, float: floatOf(el) });
  }

  // ---------- 조작 요소 모으기 ----------
  const controls = [];
  for (const el of document.querySelectorAll('a, button, input:not([type=hidden]), select, textarea, summary, label')) {
    if (!visible(el)) continue;
    for (const rc of el.getClientRects()) {
      if (rc.width < 1 || rc.height < 1) continue;
      const r = clipRect(fromDOMRect(rc), el, false, null);
      if (r) controls.push({ el, r, float: floatOf(el) });
    }
  }

  // ---------- 1) outside, overflow-x ----------
  const offenders = [];
  for (const el of document.body.querySelectorAll('*')) {
    if (el instanceof SVGElement && el.tagName.toLowerCase() !== 'svg') continue;
    if (!visible(el)) continue;
    const b = el.getBoundingClientRect();
    if (b.width < 1 || b.height < 1) continue;
    const r = clipRect(fromDOMRect(b), el, false, null);
    if (r && (r.x0 < -1 || r.x1 > vw + 1)) offenders.push({ el, r });
  }
  // 가장 안쪽만 센다: 자손 중에 같은 문제가 있으면 그 조상은 따라 밀린 것이다.
  const hasOffender = new Set();
  for (const { el } of offenders) for (let p = el.parentElement; p; p = p.parentElement) hasOffender.add(p);
  const innermost = offenders.filter(({ el }) => !hasOffender.has(el));
  // 모바일에서는 본문이 넘치면 레이아웃 뷰포트가 같이 넓어져 고정 막대도 따라 넓어진다. 그것은 원인이 아니라 결과이므로
  // 본문 쪽 원인이 있으면 고정·붙는 상자의 것은 뺀다(본문에 원인이 없을 때만 보고한다).
  const solid = innermost.filter(({ el }) => !floatOf(el));
  const deepest = solid.length > 0 ? solid : innermost;

  if (root.scrollWidth > vw + 1) {
    const widest = deepest.filter(({ r }) => r.x1 > vw + 1).sort((a, b) => b.r.x1 - a.r.x1).slice(0, 5);
    const detail = `scrollWidth ${root.scrollWidth} > 뷰포트 ${vw}`;
    if (widest.length === 0) add('overflow-x', document.body, { x0: 0, y0: 0, x1: root.scrollWidth, y1: 1 }, { detail });
    for (const { el, r } of widest) add('overflow-x', el, r, { detail });
  }
  for (const { el, r } of deepest) {
    add('outside', el, r, { detail: r.x0 < -1 ? `왼쪽 ${round(r.x0)}px` : `오른쪽 ${round(r.x1 - vw)}px 넘침` });
  }

  // ---------- 2) overlap ----------
  const seenText = new Set();
  sweepPairs(texts, (a, b) => {
    if (a.node === b.node || !sameFloat(a, b)) return;
    const iw = Math.min(a.r.x1, b.r.x1) - Math.max(a.r.x0, b.r.x0);
    const ih = Math.min(a.r.y1, b.r.y1) - Math.max(a.r.y0, b.r.y0);
    if (iw < 3 || ih < 3) return;
    if (ih / Math.min(a.r.y1 - a.r.y0, b.r.y1 - b.r.y0) <= 0.2) return;
    const key = `${selectorOf(a.el)}|${a.node.data.trim().slice(0, 20)}|${selectorOf(b.el)}|${b.node.data.trim().slice(0, 20)}`;
    if (seenText.has(key)) return;
    seenText.add(key);
    const other = { selector: selectorOf(b.el), text: b.node.data.replace(/\s+/g, ' ').trim().slice(0, 60), rect: docRect(b.r) };
    counts.overlap = (counts.overlap ?? 0) + 1;
    if (counts.overlap > MAX.overlap) return;
    findings.push({
      check: 'overlap',
      selector: selectorOf(a.el),
      text: a.node.data.replace(/\s+/g, ' ').trim().slice(0, 60),
      rect: docRect(a.r),
      other,
      detail: `겹침 ${round(iw)}x${round(ih)}px`,
    });
  });

  // ---------- 3) control-overlap ----------
  const seenControl = new Set();
  const ids = new Map();
  const idOf = (el) => {
    if (!ids.has(el)) ids.set(el, ids.size);
    return ids.get(el);
  };
  sweepPairs(controls, (a, b) => {
    if (a.el === b.el || !sameFloat(a, b)) return;
    if (a.el.contains(b.el) || b.el.contains(a.el)) return;
    if ((a.el.tagName === 'LABEL' && a.el.control === b.el) || (b.el.tagName === 'LABEL' && b.el.control === a.el)) return;
    const iw = Math.min(a.r.x1, b.r.x1) - Math.max(a.r.x0, b.r.x0);
    const ih = Math.min(a.r.y1, b.r.y1) - Math.max(a.r.y0, b.r.y0);
    if (iw < 4 || ih < 4) return;
    const key = [idOf(a.el), idOf(b.el)].sort().join('|');
    if (seenControl.has(key)) return;
    seenControl.add(key);
    add('control-overlap', a.el, a.r, {
      other: { selector: selectorOf(b.el), text: textOf(b.el), rect: docRect(b.r) },
      detail: `겹침 ${round(iw)}x${round(ih)}px`,
    });
  });

  // ---------- 4) clipped ----------
  const NO_VALUE_BOX = /^(INPUT|TEXTAREA|SELECT|SCRIPT|STYLE)$/;
  for (const el of document.body.querySelectorAll('*')) {
    if (NO_VALUE_BOX.test(el.tagName) || el instanceof SVGElement) continue;
    const cs = style(el);
    const clipX = cs.overflowX === 'hidden' || cs.overflowX === 'clip';
    const clipY = cs.overflowY === 'hidden' || cs.overflowY === 'clip';
    if (!clipX && !clipY) continue;
    if (cs.display === 'inline' || cs.display === 'contents') continue;
    if (el.clientWidth <= 1 || el.clientHeight <= 1) continue; // 보이지 않게 줄인 상자(sr-only 따위)
    // 말줄임표·줄 수 제한은 의도해서 줄인 것이다.
    const lineClamp = cs.getPropertyValue('-webkit-line-clamp');
    const clamped = lineClamp !== '' && lineClamp !== 'none';
    const overX = clipX && cs.textOverflow !== 'ellipsis' && el.scrollWidth > el.clientWidth + 1;
    const overY = clipY && !clamped && el.scrollHeight > el.clientHeight + 1;
    if (!overX && !overY) continue;
    if (!visible(el)) continue;
    // 정말 글이 잘리는지: 안쪽 글 조각이 상자 가장자리 밖으로 나가 있는가(그림이 넘치는 것은 세지 않는다).
    const b = el.getBoundingClientRect();
    const box = { x0: b.left + el.clientLeft, y0: b.top + el.clientTop, x1: b.left + el.clientLeft + el.clientWidth, y1: b.top + el.clientTop + el.clientHeight };
    let cut = null;
    const inner = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
    for (let node = inner.nextNode(); node && !cut; node = inner.nextNode()) {
      if (!/\S/.test(node.data)) continue;
      const parent = node.parentElement;
      if (!parent || parent.closest(SKIP_TEXT) || !visible(parent)) continue;
      for (const r of textRects(node, parent, el)) {
        if ((overX && (r.x1 > box.x1 + 1 || r.x0 < box.x0 - 1)) || (overY && (r.y1 > box.y1 + 1 || r.y0 < box.y0 - 1))) {
          cut = r;
          break;
        }
      }
    }
    if (!cut) continue;
    const parts = [];
    if (overX) parts.push(`가로 ${el.scrollWidth} > ${el.clientWidth}`);
    if (overY) parts.push(`세로 ${el.scrollHeight} > ${el.clientHeight}`);
    add('clipped', el, fromDOMRect(b), { detail: parts.join(', ') });
  }

  // ---------- 5) tiny-target(터치 화면만) ----------
  if (options.touch) {
    const seen = new Set();
    for (const { el, r } of controls) {
      if (el.tagName === 'LABEL' || seen.has(el)) continue;
      seen.add(el);
      if (el instanceof HTMLInputElement && el.labels && el.labels.length > 0) continue; // 글(label)까지가 누르는 자리다
      if (el.tagName === 'A' && style(el).display === 'inline') {
        // 문장 속 링크는 크기 기준에서 뺀다.
        const around = (el.parentElement?.textContent ?? '').trim().length;
        if (around > (el.textContent ?? '').trim().length + 2) continue;
      }
      const b = el.getBoundingClientRect();
      if (b.width < 24 || b.height < 24) add('tiny-target', el, r, { detail: `${round(b.width)}x${round(b.height)}px` });
    }
  }

  // 한도를 넘어 줄인 것은 한 줄로 알린다.
  for (const [check, n] of Object.entries(counts)) {
    const limit = MAX[check] ?? 50;
    if (n > limit) findings.push({ check, selector: '(생략)', text: `…외 ${n - limit}건`, rect: null });
  }
  return findings;
}

/** 지금 화면 한 번 잰다(details는 있는 그대로). */
export async function runChecks(page, options = {}) {
  return page.evaluate(inPage, { touch: false, ...options });
}

async function frames(page) {
  await page.evaluate(() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done))));
}

/**
 * 두 번 잰다: 있는 그대로, 그리고 모든 <details>를 펼친 뒤(펼침 속의 내용도 깨지면 안 된다).
 * 펼친 뒤에야 나온 발견에는 phase: 'details-open'을 붙인다. dialog는 열지 않는다.
 * afterBase(선택)는 첫 검사 뒤, 펼치기 전에 부른다.
 */
export async function runAllChecks(page, options = {}, afterBase = null) {
  const base = await runChecks(page, options);
  // afterBase: 펼치기 전(있는 그대로)의 모습이 필요한 일(스크린샷)을 여기서 한다.
  if (afterBase) await afterBase();
  const closed = await page.evaluate(() => {
    const list = [...document.querySelectorAll('details:not([open])')];
    for (const d of list) d.open = true;
    return list.length;
  });
  if (closed === 0) return base;
  await frames(page);
  const key = (f) => `${f.check}|${f.selector}|${f.text}`;
  const seen = new Set(base.map(key));
  const extra = (await runChecks(page, options)).filter((f) => !seen.has(key(f))).map((f) => ({ ...f, phase: 'details-open' }));
  return [...base, ...extra];
}

/**
 * 콘솔 쪽 검사(console): console.error, 잡히지 않은 오류, 같은 출처의 실패한 요청, CSP 위반.
 * context에 addInitScript를 걸어 CSP 위반을 window에 모으므로, 문서를 열기 전에 installProbes(context)를 부른다.
 * 쓰기: const diag = watchConsole(page, base); diag.begin({ url, status }); …이동·검사… const findings = await diag.end();
 */
export async function installProbes(context) {
  await context.addInitScript(() => {
    window.__cspViolations = [];
    document.addEventListener('securitypolicyviolation', (e) => {
      window.__cspViolations.push(`${e.violatedDirective}: ${e.blockedURI || 'inline'} (${e.sourceFile || ''}:${e.lineNumber || 0})`);
    });
  });
}

export function watchConsole(page, base) {
  const origin = new URL(base).origin;
  let current = null;
  const log = (kind, text) => {
    if (current) current.push({ check: 'console', selector: kind, text: String(text).replace(/\s+/g, ' ').slice(0, 300), rect: null });
  };
  // 그 화면이 일부러 4xx로 답하는 경우(예: 없는 항목)의 JSON 답과, 브라우저가 그것에 대해 찍는 콘솔 글은 오류가 아니다.
  const expected = (url) => current && current.expect.status >= 400 && url === origin + current.expect.url;

  page.on('console', (msg) => {
    if (msg.type() !== 'error') return;
    const text = msg.text();
    if (/Failed to load resource/.test(text) && expected(msg.location().url)) return;
    log(/Content Security Policy/i.test(text) ? 'csp' : 'console.error', text);
  });
  page.on('pageerror', (error) => log('pageerror', error?.stack?.split('\n').slice(0, 2).join(' ') ?? error));
  page.on('response', (response) => {
    const url = response.url();
    if (!url.startsWith(origin) || response.status() < 400) return;
    if (expected(url) && response.status() === current.expect.status) return;
    log(`http ${response.status()}`, url.slice(origin.length));
  });
  page.on('requestfailed', (request) => {
    if (!request.url().startsWith(origin)) return;
    const why = request.failure()?.errorText ?? '';
    if (/ERR_ABORTED/.test(why)) return; // 이동하며 취소된 요청
    log('requestfailed', `${request.url().slice(origin.length)} ${why}`);
  });

  return {
    begin(expect) {
      current = [];
      current.expect = expect;
    },
    async end() {
      const csp = await page.evaluate(() => window.__cspViolations ?? []).catch(() => []);
      // 콘솔에도 같은 위반이 찍히면(브라우저가 둘 다 낸다) 한 번만 센다.
      if (!(current ?? []).some((f) => f.selector === 'csp')) for (const v of csp) log('csp', v);
      const out = current ?? [];
      current = null;
      return out;
    },
  };
}
