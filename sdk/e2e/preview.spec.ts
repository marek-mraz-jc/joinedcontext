import { readFileSync } from "node:fs";
import { expect, test } from "@playwright/test";
import type { Row } from "../src/ngsi";
import type { JcRequest } from "../src/sdk/transport";
import { stubTransport } from "../src/sdk/testing";

const OUT = process.env.JC_PREVIEW_OUT;
const HOST = "http://portal.test/host";
const PREVIEW = "http://portal.test/preview";

const ROWS: Row[] = [
  { id: "urn:ngsi-ld:Station:hel.fi:mobility:1", type: "Station", name: "Kamppi", bikes: 4, status: "open", location: { type: "Point", coordinates: [24.93, 60.17] } },
  { id: "urn:ngsi-ld:Station:hel.fi:mobility:2", type: "Station", name: "Kallio", bikes: 7, status: "closed", location: { type: "Point", coordinates: [24.95, 60.18] } },
];

// The Portal page in miniature: a frame without allow-same-origin, and a bridge that answers
// what the frame posts (SDK-06, AP-63).
const HOST_PAGE = `<!doctype html><html><body>
<iframe sandbox="allow-scripts" src="${PREVIEW}" style="width:1200px;height:900px"></iframe>
<script>
addEventListener("message", async (event) => {
  const frame = document.querySelector("iframe");
  if (event.source !== frame.contentWindow || event.data?.kind === undefined) return;
  if (event.data.kind === "jc-error") { window.frameErrors = [...(window.frameErrors ?? []), event.data]; return; }
  if (event.data.kind !== "jc-request") return;
  const { id, method, path, body } = event.data;
  const answer = await window.bridge({ method, path, body });
  frame.contentWindow.postMessage({ kind: "jc-response", id, ...answer }, "*");
});
</script></body></html>`;

test.skip(!OUT, "set JC_PREVIEW_OUT to where the Portal's preview test wrote template.html");

test("the template app renders from the Portal's preview document in a sandboxed frame", async ({ page }) => {
  const html = readFileSync(`${OUT}/template.html`, "utf8");
  const csp = readFileSync(`${OUT}/template.csp`, "utf8");
  const transport = stubTransport({
    entities: ROWS,
    access: { permissions: [{ resource: { type: "*" }, actions: ["queryEntity", "retrieveEntity"], attributes: "*" }], prohibitions: [] },
    functions: { summary: () => ({ types: [{ type: "Station", count: 2, averages: { bikes: 5.5 } }] }) },
  });
  const problems: string[] = [];
  page.on("console", (message) => {
    if (message.type() === "error") problems.push(message.text());
  });
  page.on("pageerror", (error) => problems.push(error.message));
  await page.exposeFunction("bridge", (request: JcRequest) => transport(request));
  await page.route(HOST, (route) => route.fulfill({ contentType: "text/html", body: HOST_PAGE }));
  await page.route(PREVIEW, (route) =>
    route.fulfill({ contentType: "text/html; charset=utf-8", headers: { "content-security-policy": csp }, body: html }),
  );

  await page.goto(HOST);
  const frame = page.frameLocator("iframe");

  await expect(frame.getByRole("heading", { name: "bikes" })).toBeVisible({ timeout: 20_000 });
  // The server's summary is a chart since T-2921 (AP-138): the figure under its caption, drawn.
  const summary = frame.locator("figure", { hasText: "Entities per type" });
  await expect(summary.locator("canvas")).toHaveCount(1);
  await frame.getByRole("button", { name: "Station" }).click();
  await expect(frame.getByRole("region", { name: "Station" }).getByText("Kallio")).toBeVisible();
  // The map draws the stations: its worker came in as a data: URL, which only a frame without
  // an origin makes necessary. More than two colours means the points are on the background.
  const preview = page.mainFrame().childFrames()[0];
  await expect
    .poll(() =>
      preview.evaluate(() => {
        const canvas = document.querySelector<HTMLCanvasElement>("canvas.maplibregl-canvas");
        if (!canvas) return 0;
        const copy = document.createElement("canvas");
        copy.width = canvas.width;
        copy.height = canvas.height;
        const context = copy.getContext("2d", { willReadFrequently: true })!;
        context.drawImage(canvas, 0, 0);
        const { data } = context.getImageData(0, 0, copy.width, copy.height);
        const colours = new Set<number>();
        for (let i = 0; i < data.length; i += 4 * 97) colours.add((data[i] << 16) | (data[i + 1] << 8) | data[i + 2]);
        return colours.size;
      }),
    { timeout: 15_000 })
    .toBeGreaterThan(2);
  // Everything the app read went through the bridge, on the configured endpoint only.
  expect(transport.calls.length).toBeGreaterThan(0);
  expect(transport.calls.every((call) => call.path.startsWith("/api/endpoint/bikes/") || call.path === "/functions/summary")).toBe(true);
  expect(await page.evaluate(() => (window as unknown as { frameErrors?: unknown[] }).frameErrors ?? [])).toEqual([]);
  expect(problems.filter((p) => /Content Security Policy|Refused|Failed to resolve module/i.test(p))).toEqual([]);
});
