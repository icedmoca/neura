import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

const backendTarget = process.env.VITE_BACKEND_TARGET ?? "http://127.0.0.1:8000";

export default defineConfig({
  plugins: [react()],
  server: {
    host: process.env.VITE_DEV_HOST || "127.0.0.1",
    port: Number(process.env.VITE_DEV_PORT) || 5173,
    strictPort: true,
    proxy: {
      "/backend": {
        target: backendTarget,
        changeOrigin: true,
        rewrite: (p) => p.replace(/^\/backend/, "") || "/",
      },
    },
  },
});
