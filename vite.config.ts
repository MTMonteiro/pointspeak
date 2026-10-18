import { resolve } from "node:path";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// Paralelismo dos testes no runner: RUNNER_TEST_MAX_WORKERS (número ou percentual).
const maxWorkers = process.env.RUNNER_TEST_MAX_WORKERS ?? "50%";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  // O Tauri espera uma porta fixa e não limpa a tela do CLI.
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**", "**/target/**"] },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    rollupOptions: {
      // Uma página por janela do Tauri.
      input: {
        hud: resolve(import.meta.dirname, "hud.html"),
        settings: resolve(import.meta.dirname, "settings.html"),
      },
    },
  },
  test: {
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
    include: ["src/**/*.test.{ts,tsx}"],
    maxWorkers: /^\d+$/.test(maxWorkers) ? Number(maxWorkers) : maxWorkers,
    forceRerunTriggers: [
      "**/package.json/**",
      "**/vite.config.ts/**",
      "**/src/test/**",
    ],
  },
});
