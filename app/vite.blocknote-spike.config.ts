import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";
import { resolve } from "node:path";

export default defineConfig({
  plugins: [react()],
  build: {
    emptyOutDir: true,
    minify: "oxc",
    outDir: ".data/redesign/blocknote-spike-dist",
    rollupOptions: {
      input: resolve(import.meta.dirname, "blocknote-spike.html"),
    },
    target: "safari13",
  },
});
