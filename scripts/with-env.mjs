#!/usr/bin/env node
/**
 * 跨平台的环境变量前缀。
 *
 * `VAR=1 vite build` 只在 POSIX shell 成立，Windows 的 cmd.exe 会把它当成命令名。
 * 这里解析 `KEY=VALUE ... -- <命令> [参数...]`，带上环境变量转发执行，两种平台行为一致。
 *
 * 例：node scripts/with-env.mjs VITE_DEMO_MODE=1 VITE_PAGES_DEMO=1 -- pnpm exec vite build
 */
import { spawnSync } from 'node:child_process';

const argv = process.argv.slice(2);
const separator = argv.indexOf('--');
if (separator < 0) {
  console.error('用法: node scripts/with-env.mjs KEY=VALUE ... -- <命令> [参数...]');
  process.exit(2);
}

const env = { ...process.env };
for (const pair of argv.slice(0, separator)) {
  const eq = pair.indexOf('=');
  if (eq <= 0) {
    console.error(`无效的环境变量: ${pair}`);
    process.exit(2);
  }
  env[pair.slice(0, eq)] = pair.slice(eq + 1);
}

const [command, ...args] = argv.slice(separator + 1);
if (!command) {
  console.error('缺少要执行的命令');
  process.exit(2);
}

const result = spawnSync(command, args, {
  stdio: 'inherit',
  env,
  // Windows 下 pnpm/npm 都是 .cmd，必须经 shell 解析。
  shell: process.platform === 'win32',
});
process.exit(result.status ?? 1);
