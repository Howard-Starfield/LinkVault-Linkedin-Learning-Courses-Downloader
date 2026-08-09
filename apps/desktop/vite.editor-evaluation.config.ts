import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  server: {
    strictPort: true,
    host: "127.0.0.1",
    port: 1421
  },
  build: {
    outDir: "dist-editor-evaluation",
    rollupOptions: {
      input: "editor-evaluation.html"
    }
  }
});
