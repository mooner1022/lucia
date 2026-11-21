// 브라우저 띄우기(검사와 자체 시험이 같이 쓴다).
//
// 이 호스트 같은 곳에서는 Chromium이 시스템 라이브러리(libasound)나 한글 글꼴이 없어 못 뜨거나 글자가 네모로 나온다.
// 권한 없이 쓰는 우회 두 가지를 환경 변수로 받는다.
//   LUCIA_E2E_LIBS   libasound.so.2 등이 든 디렉터리. 기본값 ~/.local/opt/e2e-libs/root/usr/lib/x86_64-linux-gnu(있을 때만).
//                    브라우저 프로세스의 LD_LIBRARY_PATH 앞에 붙는다.
//   LUCIA_E2E_FONTS  ttf/otf 글꼴이 든 디렉터리. 기본값 ~/.local/opt/e2e-libs/fonts, 없으면 e2e/out/fonts(있을 때만). 임시 fontconfig 파일로 브라우저에만 더한다.
//                    한글 글꼴이 하나도 없으면 한글이 네모로 그려져 스크린샷을 눈으로 볼 수 없고, 글줄 너비도 근사치가 된다.
//                    (프로젝트 글꼴 목록의 맨 앞은 Pretendard다. OFL 라이선스라 내려받아 두어도 된다.)

import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readdirSync, writeFileSync } from 'node:fs';
import { homedir, tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { chromium } from 'playwright';

const here = dirname(fileURLToPath(import.meta.url));
const DEFAULT_LIBS = join(homedir(), '.local/opt/e2e-libs/root/usr/lib/x86_64-linux-gnu');
const DEFAULT_FONTS = join(homedir(), '.local/opt/e2e-libs/fonts');

function hasFonts(dir) {
  try {
    return readdirSync(dir).some((f) => /\.(ttf|otf|ttc)$/i.test(f));
  } catch {
    return false;
  }
}

/** 브라우저 프로세스에 줄 환경과 안내 글(한글 글꼴이 없을 때의 경고 등)을 만든다. */
function browserEnv() {
  const env = { ...process.env };
  const notes = [];

  const libs = process.env.LUCIA_E2E_LIBS ?? (existsSync(DEFAULT_LIBS) ? DEFAULT_LIBS : '');
  if (libs) env.LD_LIBRARY_PATH = libs + (env.LD_LIBRARY_PATH ? `:${env.LD_LIBRARY_PATH}` : '');

  const fonts = resolve(process.env.LUCIA_E2E_FONTS ?? (hasFonts(DEFAULT_FONTS) ? DEFAULT_FONTS : join(here, 'out/fonts')));
  if (hasFonts(fonts)) {
    // 시스템 설정을 그대로 두고 글꼴 디렉터리만 더한 fontconfig를 임시로 만든다.
    const dir = join(tmpdir(), `lucia-e2e-fontconfig-${process.getuid?.() ?? 0}`);
    mkdirSync(dir, { recursive: true });
    const conf = join(dir, 'fonts.conf');
    writeFileSync(
      conf,
      `<?xml version="1.0"?>\n<!DOCTYPE fontconfig SYSTEM "fonts.dtd">\n<fontconfig>\n` +
        `  <include ignore_missing="yes">/etc/fonts/fonts.conf</include>\n` +
        `  <dir>${fonts}</dir>\n  <cachedir>${join(dir, 'cache')}</cachedir>\n</fontconfig>\n`,
    );
    env.FONTCONFIG_FILE = conf;
  }

  try {
    const korean = execFileSync('fc-list', [':charset=ac00', 'family'], { env, encoding: 'utf8', timeout: 20000 }).trim();
    if (!korean) notes.push('한글 글꼴이 없습니다: 한글이 네모로 그려지고 글줄 너비가 근사치입니다(LUCIA_E2E_FONTS로 ttf/otf 디렉터리를 준다).');
  } catch {
    // fc-list가 없으면 확인하지 않는다.
  }
  return { env, notes };
}

/** Chromium을 띄운다. 못 뜨면 까닭과 우회 방법을 담은 오류를 던진다. */
export async function launchBrowser() {
  const { env, notes } = browserEnv();
  try {
    const browser = await chromium.launch({ env });
    return { browser, notes };
  } catch (error) {
    const text = String(error?.message ?? error);
    const lib = text.match(/error while loading shared libraries: (\S+)/);
    const hint = lib
      ? `\n시스템 라이브러리 ${lib[1]}가 없습니다. 그것이 든 디렉터리를 LUCIA_E2E_LIBS로 준다(권한이 필요 없는 방법: apt-get download 한 deb를 dpkg -x로 풀기).`
      : '\n`npx playwright install chromium`을 했는지 확인한다.';
    throw new Error(`Chromium을 띄우지 못했습니다.${hint}\n${text.split('\n').slice(0, 3).join('\n')}`);
  }
}
