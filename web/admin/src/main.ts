import { mount } from 'svelte';
import App from './App.svelte';
import { start } from './lib/app.svelte';
import { initTheme } from './lib/theme.svelte';
// 글꼴(결정 0062). 빌드에 넣어 같은 출처에서 준다(CSP font-src 'self'). 둘 다 SIL OFL 1.1이다(허가서: public/licenses/).
// 낱자 묶음(unicode-range)으로 잘게 나뉘어 있어, 브라우저는 화면에 나온 글자가 든 조각만 받는다.
//  - Pretendard Variable: 본문·라벨·숫자
//  - 고운바탕(Gowun Batang) 400·700: 화면 제목, 이름, 로그인, 맺음 글귀(--font-display)
// 여기서 직접 들여와야 vite.config.ts의 woff2Only가 걸린다(CSS 안의 @import는 그 손질을 거치지 않는다).
import 'pretendard/dist/web/variable/pretendardvariable-dynamic-subset.css';
import '@fontsource/gowun-batang/400.css';
import '@fontsource/gowun-batang/700.css';
import './ui/tokens.css';
import './ui/base.css';

initTheme();
mount(App, { target: document.getElementById('app')! });
start();
