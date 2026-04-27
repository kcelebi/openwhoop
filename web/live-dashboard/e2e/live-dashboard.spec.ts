import { expect, test } from "@playwright/test";

function heartRateJson(opts: {
  bpm: number;
  unix_ms?: number;
  received_at_ms?: number;
}) {
  const { bpm, unix_ms = 1_700_000_000_000 + bpm, received_at_ms = Date.now() } = opts;
  return JSON.stringify({
    type: "heart_rate",
    unix_ms,
    bpm,
    time_local: "2099-01-01 12:00:00",
    rr_count: 0,
    received_at_ms,
  });
}

test.describe("live dashboard (mock WebSocket)", () => {
  test("BPM value updates when server sends a new heart_rate frame", async ({ page }) => {
    await page.routeWebSocket("/ws", (ws) => {
      queueMicrotask(() => {
        ws.send(heartRateJson({ bpm: 70, received_at_ms: 1000 }));
      });
      setTimeout(() => {
        ws.send(heartRateJson({ bpm: 88, received_at_ms: 2000 }));
      }, 400);
    });

    await page.goto("/");
    await expect(page.getByTestId("ws-state")).toHaveText("Connected", { timeout: 15_000 });
    await expect(page.getByTestId("live-bpm")).toHaveText("70");
    await expect(page.getByTestId("live-bpm")).toHaveText("88", { timeout: 10_000 });
  });

  test("host received time updates for repeated identical BPM (live stream tick)", async ({
    page,
  }) => {
    await page.routeWebSocket("/ws", (ws) => {
      queueMicrotask(() => {
        ws.send(heartRateJson({ bpm: 72, unix_ms: 111, received_at_ms: 500_001 }));
      });
      setTimeout(() => {
        ws.send(heartRateJson({ bpm: 72, unix_ms: 111, received_at_ms: 500_002 }));
      }, 350);
    });

    await page.goto("/");
    await expect(page.getByTestId("ws-state")).toHaveText("Connected", { timeout: 15_000 });
    await expect(page.getByTestId("live-bpm")).toHaveText("72");
    const first = await page.getByTestId("live-received-at").getAttribute("data-received-at");
    await expect
      .poll(
        async () => page.getByTestId("live-received-at").getAttribute("data-received-at"),
        { timeout: 10_000 },
      )
      .not.toBe(first);
  });

  test("after reload, BPM appears once mock sends heart_rate on the new socket", async ({
    page,
  }) => {
    await page.routeWebSocket("/ws", (ws) => {
      queueMicrotask(() => {
        ws.send(heartRateJson({ bpm: 61 }));
      });
    });

    await page.goto("/");
    await expect(page.getByTestId("live-bpm")).toHaveText("61", { timeout: 15_000 });

    await page.reload();
    await expect(page.getByTestId("ws-state")).toHaveText("Connected", { timeout: 15_000 });
    await expect(page.getByTestId("live-bpm")).toHaveText("61", { timeout: 15_000 });
  });
});
