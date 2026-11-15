import { defineConfig } from "vite";

// The Tauri window loads the dev server on a fixed port and the built files from dist/.
export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    watch: { ignored: ["**/src-tauri/**", "**/target/**"] },
  },
  build: {
    target: "es2021",
    outDir: "dist",
    emptyOutDir: true,
  },
});
