// 前端 lint 门禁：只覆盖主界面与内置悬浮窗两个 Vite 工程的源码。
//
// 规则范围刻意收窄：
// - `typescript-eslint` 的 recommended：捕捉未使用变量、any、无效类型等真实问题
//   （tsc 的 noUnusedLocals 已覆盖一部分，lint 在 CI 里作为第二道门）；
// - `react-hooks` 的两条经典规则：hooks 调用顺序（error，会让运行时报错）
//   与依赖数组（warn；历史代码里的豁免按文件最小化处理，不整体关闭）。
//
// 不引入格式化规则（prettier 不在此仓库），避免与编辑器/现有风格打架。
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";
import globals from "globals";

export default tseslint.config(
  {
    ignores: [
      "dist/**",
      "public/**",
      "companion-ui/dist-embed/**",
      "companion-ui/dist/**",
      "node_modules/**",
      "src-tauri/**",
    ],
  },
  ...tseslint.configs.recommended,
  {
    files: ["src/**/*.{ts,tsx}", "companion-ui/src/**/*.{ts,tsx}"],
    languageOptions: {
      globals: { ...globals.browser },
    },
    plugins: { "react-hooks": reactHooks },
    rules: {
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "warn",
      // 以 `_` 开头的参数/变量是「签名占位」约定（如 fetch mock 的 _url/_init）；
      // 只豁免这一种写法，其余未使用变量仍然报错。
      "@typescript-eslint/no-unused-vars": [
        "error",
        { argsIgnorePattern: "^_", varsIgnorePattern: "^_", caughtErrorsIgnorePattern: "^_" },
      ],
    },
  },
);
