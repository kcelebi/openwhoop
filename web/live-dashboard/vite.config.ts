import { createLogger, defineConfig } from "vite";
import react from "@vitejs/plugin-react";

/**
 * ECONNREFUSED → nothing is listening on 3848 (Studio not started or wrong port).
 * EPIPE / ECONNRESET → normal when a WebSocket leg closes (reload, reconnect, browser
 * throttling); Studio may still be running. Do not conflate those with "server down".
 */
const STUDIO_BACKEND_HINT =
  "[vite] Nothing is accepting connections on 127.0.0.1:3848 — start Studio, then refresh:\n" +
  "    cargo run -p openwhoop -- live-server --whoop <your-device-id>";

function isWsProxyMsg(msg: string): boolean {
  return (
    msg.includes("ws proxy error") || msg.includes("ws proxy socket error")
  );
}

const baseLogger = createLogger();
let lastRefusedHintMs = 0;
const logger = {
  ...baseLogger,
  error(msg: string, options?: Parameters<typeof baseLogger.error>[1]) {
    if (typeof msg === "string" && isWsProxyMsg(msg)) {
      // Teardown noise while the backend is healthy — suppress stacks only.
      if (msg.includes("EPIPE") || msg.includes("ECONNRESET")) {
        return;
      }
      if (msg.includes("ECONNREFUSED")) {
        const now = Date.now();
        if (now - lastRefusedHintMs > 15_000) {
          lastRefusedHintMs = now;
          baseLogger.warn(`\n${STUDIO_BACKEND_HINT}\n`, { timestamp: true });
        }
        return;
      }
    }
    baseLogger.error(msg, options);
  },
};

export default defineConfig({
  customLogger: logger,
  plugins: [react()],
  server: {
    // Bind IPv4 so tools that probe 127.0.0.1 (Playwright webServer, Rust live-server) match `localhost`.
    host: "127.0.0.1",
    port: 5173,
    proxy: {
      // Use http + ws:true so the dev server performs a proper Upgrade; ws:// targets often break.
      "/ws": {
        target: "http://127.0.0.1:3848",
        ws: true,
        changeOrigin: true,
      },
      "/api": {
        target: "http://127.0.0.1:3848",
        changeOrigin: true,
      },
    },
  },
});
