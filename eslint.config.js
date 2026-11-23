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
  // 入口模块的「顶层副作用早于模块绑定初始化」门禁（GitHub issue #5）。
  //
  // companion-ui 的两个入口在模块顶层直接挂载 React。这类文件里，顶层调用一旦写在
  // `let` / `const` / `class` 声明之前，就可能在 async 函数里撞上 TDZ，而
  // `void boot(...)` 把异常吞成 unhandled rejection——页面静默失败、`#desktop-rail`
  // 永远为空。0.2.x 到 0.3.0 的「看不到会话悬浮栏」正是 `let stopAttention` 写在
  // `void boot(container)` 之后导致的（见 docs/companion.md 的「入口模块初始化顺序」）。
  //
  // 只对这两个入口文件开启（不放开到全仓库）：历史代码里「先用后声明」的写法很多，
  // 全量开启会成片报错，反而逼人加 disable。入口文件本来就该保持“无顶层顺序依赖”，
  // 这条约束在这里成本最低、收益最直接。
  {
    files: ["companion-ui/src/desktop/rail.tsx", "companion-ui/src/desktop/settings.tsx"],
    rules: {
      // 用 TS 版规则而不是核心 `no-use-before-define`：后者不认 TS 的类型引用，
      // 在 `.tsx` 上容易把类型位置误报成运行时引用。
      "@typescript-eslint/no-use-before-define": [
        "error",
        { functions: true, classes: true, variables: true, enums: true, typedefs: true },
      ],
    },
  },
);
