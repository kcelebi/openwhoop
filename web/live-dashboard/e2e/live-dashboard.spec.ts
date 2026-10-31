import { expect, test } from "@playwright/test";

function heartRatePoints(bpm: number, unixMs = Date.now()) {
  return [{ t_ms: unixMs, bpm }];
}

test.describe("live dashboard (mock REST backend — Milestone 1 polling)", () => {
  test("BPM value updates via REST polling", async ({ page }) => {
    let bpm = 70;
    await page.route("**/api/insights/heart-rate-series**", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          range: "1h",
          count: 1,
          points: heartRatePoints(bpm),
        }),
      }),
    );

    await page.goto("/");
    await expect(page.getByTestId("live-bpm")).toHaveText("70");

    bpm = 88;
    await expect
      .poll(async () => (await page.getByTestId("live-bpm").textContent()) ?? "")
      .toEqual("88");
  });

  test("repeated samples do not appear as fresh data", async ({ page }) => {
    const sampleTime = Date.now();
    await page.route("**/api/insights/heart-rate-series**", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          range: "1h",
          count: 1,
          points: heartRatePoints(72, sampleTime),
        }),
      }),
    );

    await page.goto("/");
    await expect(page.getByTestId("live-bpm")).toHaveText("72");
    const first = await page.getByTestId("live-received-at").getAttribute("data-received-at");
    await page.waitForTimeout(2_500);
    await expect(page.getByTestId("live-received-at")).toHaveAttribute("data-received-at", first!);
  });

  test("after reload, BPM appears once mock responds", async ({ page }) => {
    await page.route("**/api/insights/heart-rate-series**", (route) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({
          range: "1h",
          count: 1,
          points: heartRatePoints(61),
        }),
      }),
    );

    await page.goto("/");
    await expect(page.getByTestId("live-bpm")).toHaveText("61");

    await page.reload();
    await expect(page.getByTestId("live-bpm")).toHaveText("61");
  });
});
