import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "node:path";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;
// GitHub Pages serves only the public demo from the repository subpath.
// Normal WebUI and Tauri builds intentionally keep Vite's root base.
// @ts-expect-error process is a nodejs global
const base = process.env.VITE_PAGES_DEMO === "1" ? "/workbuddy-shift/" : "/";

// https://vite.dev/config/
export default defineConfig(async () => ({
  base,
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },

  // Vite options tailored for Tauri development and only applied in `tauri dev` or `tauri build`
  //
  // 1. prevent Vite from obscuring rust errors
  clearScreen: false,
  build: {
    rollupOptions: {
      output: {
        /**
         * 把体积大、变动少的依赖拆成独立 chunk：路由切换不再重复下载，
         * 只是换页时按需加载对应业务代码。
         *
         * 判定顺序有意义：`recharts` 含 "react"、`react-router` 也含 "react"，
         * 必须排在 react 之前，否则会被并进 react chunk。
         */
        manualChunks(id: string) {
          // 本地模块单独成块：否则 Rollup 会用它的名字命名整个图表动态块。
          if (id.includes("stacked-bar-visuals")) return "stacked-bar-visuals";
          if (!id.includes("node_modules")) return undefined;
          if (id.includes("react-router")) return "router";
          if (id.includes("@radix-ui") || id.includes("lucide-react")) return "ui";
          if (id.includes("react") || id.includes("scheduler")) return "react";
          return undefined;
        },
      },
    },
  },
  // 2. tauri expects a fixed port, fail if that port is not available
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. tell Vite to ignore watching `src-tauri`
      ignored: ["**/src-tauri/**"],
    },
  },
}));
