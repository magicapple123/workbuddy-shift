import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';

// 独立于 vite.config.js：那份带 react/tailwind 插件、demo 两个构建入口与
// build-time 的 VITE_DEMO_MODE 注入，测试用不上。
//
// 两件事必须在这里说清楚：
// 1. `environment: 'happy-dom'`——本工程的测试对象是**入口模块**：它直接操作
//    `document`、`localStorage`、`matchMedia`、`ResizeObserver`，并且用
//    `document.getElementById('desktop-rail')` 找挂载点。node 环境下这些全都没有，
//    测出来的东西跟真实悬浮窗无关（见 src/desktop/rail.smoke.test.tsx）。
// 2. `include` 只收冒烟测试：本工程此前没有测试，规则宁窄不宽——新增测试先按
//    `*.smoke.test.ts(x)` 命名，确认环境合适后再谈放开。
export default defineConfig({
  resolve: {
    // 与 vite.config.js / tsconfig.json 的 paths 保持一致。
    alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) },
  },
  test: {
    environment: 'happy-dom',
    include: ['src/**/*.smoke.test.ts', 'src/**/*.smoke.test.tsx'],
  },
});
