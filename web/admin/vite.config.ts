import { svelte } from '@sveltejs/vite-plugin-svelte';
import type { Plugin } from 'vite';
import { defineConfig } from 'vitest/config';

// 글꼴 꾸러미(@fontsource)의 스타일은 woff2와 woff를 함께 가리킨다. woff2만 남긴다(바이너리에 넣는 파일을 반으로 줄인다).
function woff2Only(): Plugin {
  return {
    name: 'lucia-woff2-only',
    enforce: 'pre',
    transform(code, id) {
      if (!/@fontsource\/.*\.css(\?|$)/.test(id)) return null;
      return { code: code.replace(/,\s*url\([^)]*\.woff\)\s*format\(['"]woff['"]\)/g, ''), map: null };
    },
  };
}

// 개발 서버에서 화면만 바로바로 고쳐 볼 때: 시험 데이터 서버(e2e/run.sh 머리글의 cargo test … serve_ui_fixtures)를 띄워 두고
//   LUCIA_UI_DEV_PROXY=http://127.0.0.1:47890 npm run dev
// 로 띄우면, 화면(HTML·소스)은 Vite가 주고 데이터(JSON)·폼·그림은 그 서버로 넘긴다. 운영 데몬을 가리키지 않는다.
const devProxy = (globalThis as { process?: { env: Record<string, string | undefined> } }).process?.env.LUCIA_UI_DEV_PROXY;

// 관리 화면(결정 0061): 정적 빌드를 luciad가 바이너리에 넣어 내준다. 인라인 스크립트·스타일을 만들지 않는다(CSP).
export default defineConfig(({ mode }) => ({
  plugins: [woff2Only(), svelte()],
  // 시험(jsdom)에서는 Svelte의 브라우저용 코드를 쓴다.
  resolve: mode === 'test' ? { conditions: ['browser'] } : {},
  build: {
    outDir: 'dist',
    assetsDir: 'assets',
    emptyOutDir: true,
    sourcemap: false,
    // 작은 그림도 파일로 둔다(data: 주소를 쓰지 않는다).
    assetsInlineLimit: 0,
    modulePreload: { polyfill: false },
  },
  server: {
    port: 5178,
    strictPort: true,
    proxy: devProxy
      ? {
          '/': {
            target: devProxy,
            // 그 서버는 Host와 Origin이 제 주소일 때만 답한다.
            changeOrigin: true,
            headers: { origin: devProxy },
            bypass(req) {
              const url = req.url ?? '/';
              const own = /^\/(@|src\/|node_modules\/|favicon\.svg)/.test(url);
              const html = req.method === 'GET' && (req.headers.accept ?? '').includes('text/html');
              return own || html ? url : undefined;
            },
          },
        }
      : undefined,
  },
  test: {
    environment: 'jsdom',
    include: ['tests/**/*.test.ts'],
    testTimeout: 20000,
  },
}));
