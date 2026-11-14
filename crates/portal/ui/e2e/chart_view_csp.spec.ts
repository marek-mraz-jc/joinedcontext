import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";
import type { Page } from "@playwright/test";

// T-6887: the chart view (crates/shared/jc-chart/src/chart_view.html) with the chart library it
// inlines, run as an MCP Apps host runs it: a sandboxed frame under the specification's default CSP,
// which allows no eval. The page draws through Vega's interpreter, so the browser refuses nothing,
// and a hostile title and hostile points run nothing, ask for nothing and open no link.

const HERE = dirname(fileURLToPath(import.meta.url));
const SRC = resolve(HERE, "../../../shared/jc-chart/src");
const VENDOR = resolve(HERE, "../../../../vendor/viz/vendor");
const LIBRARY: [string, string][] = [
  ["__JC_VEGA__", "vega-6.4.0.min.js"],
  ["__JC_VEGA_LITE__", "vega-lite-6.5.0.min.js"],
  ["__JC_VEGA_INTERPRETER__", "vega-interpreter-2.3.2.js"],
];
// The MCP Apps default (SEP-1865) when a resource declares no domains: no unsafe-eval.
const CSP = "default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; media-src 'self' data:; connect-src 'none'";
const PAGE = LIBRARY.reduce(
  (page, [mark, file]) => page.replace(mark, () => readFileSync(resolve(VENDOR, file), "utf8")),
  readFileSync(resolve(SRC, "chart_view.html"), "utf8")
    .replace("__JC_THEME__", readFileSync(resolve(SRC, "theme.json"), "utf8").trimEnd())
    .replace("__PORTAL_ORIGIN__", "null"),
).replace('<meta charset="utf-8">', `<meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="${CSP}">`);

const HOSTILE = "</script><img src=x onerror=alert(1)>";
const EXPRESSION = "datum.x + '}' + (alert(1))";
const LINK = "javascript:alert(1)";
const T0 = 1794528000000;

function answer(spec: Record<string, unknown>, points: unknown[]) {
  return {
    content: [{ type: "image", data: "iVBORw0KGgo=", mimeType: "image/png" }],
    structuredContent: { rows: points.length, sources: [{ endpoint: "air" }], categories: { x: [{ label: "#1", value: HOSTILE }] }, untrusted: true },
    _meta: {
      "joinedcontext.com/chartSpec": { width: 800, height: 450, ...spec },
      "joinedcontext.com/chartPoints": points,
      "joinedcontext.com/pointsTruncated": false,
    },
  };
}

// A host page: the view in a sandboxed srcdoc frame; it answers ui/initialize, posts the answer and
// keeps every message the view sends.
async function host(page: Page, result: unknown) {
  const refused: string[] = [];
  const requests: string[] = [];
  page.on("console", (m) => { if (m.type() === "error" || m.type() === "warning") refused.push(m.text()); });
  page.on("pageerror", (e) => refused.push(e.message));
  page.on("dialog", (d) => { refused.push(`dialog ${d.message()}`); void d.dismiss(); });
  page.on("request", (r) => { if (!/^(about|data):/.test(r.url())) requests.push(r.url()); });
  await page.setContent(`<!doctype html><html><body><iframe id="f" sandbox="allow-scripts" style="width:100%;height:600px"></iframe><script>
    window.sent = [];
    const f = document.getElementById("f");
    window.addEventListener("message", (e) => {
      if (e.source !== f.contentWindow) return;
      const m = e.data; window.sent.push(m.method);
      if (m.method === "ui/initialize") f.contentWindow.postMessage({ jsonrpc: "2.0", id: m.id, result: { protocolVersion: "2026-01-26", hostContext: { theme: "light" } } }, "*");
      else if (m.method === "ui/notifications/initialized") f.contentWindow.postMessage({ jsonrpc: "2.0", method: "ui/notifications/tool-result", params: window.RESULT }, "*");
    });
  </script></body></html>`);
  await page.evaluate(([html, r]) => {
    (window as unknown as { RESULT: unknown }).RESULT = r;
    (document.getElementById("f") as HTMLIFrameElement).srcdoc = html as string;
  }, [PAGE, result]);
  const frame = page.frameLocator("#f");
  return { frame, refused, requests };
}

test("the_chart_view_runs_under_the_default_host_csp_without_a_refusal", async ({ page }) => {
  const points = Array.from({ length: 1000 }, (_, i) => ({ x: T0 + i * 3_600_000, y: 20 + (i % 24) / 2, color: i % 2 }));
  const started = Date.now();
  const { frame, refused, requests } = await host(page, answer(
    { mark: "line", title: "PM10", x: { scale: "temporal", title: "observedAt" }, y: { scale: "quantitative", title: "pm10" }, color: { scale: "nominal", title: "station" } },
    points,
  ));
  await expect(frame.locator(".canvas svg g.mark-line path")).toHaveCount(2);
  test.info().annotations.push({ type: "first draw ms", description: String(Date.now() - started) });
  await expect(frame.locator(".fallback")).toHaveCount(0);
  expect(refused).toEqual([]);
  expect(requests).toEqual([]);
});

test("a hostile title and hostile points run nothing under the host CSP", async ({ page }) => {
  const { frame, refused, requests } = await host(page, answer(
    { mark: "bar", title: HOSTILE, x: { scale: "nominal", title: EXPRESSION }, y: { scale: "quantitative", title: LINK }, color: { scale: "nominal", title: HOSTILE } },
    [{ x: 0, y: 21, color: 0, observedAt: HOSTILE }, { x: EXPRESSION, y: LINK, color: HOSTILE, size: { signal: EXPRESSION } }],
  ));
  await expect(frame.locator(".canvas svg g.mark-rect path")).toHaveCount(1);
  await expect(frame.locator("h1")).toHaveText(HOSTILE);
  await frame.locator(".plot").focus();
  await frame.locator(".plot").press("ArrowRight");
  await expect(frame.locator(".tip")).toContainText(`${EXPRESSION}: #1 ${HOSTILE}`);
  await frame.locator(".plot").hover();
  for (const text of [HOSTILE, EXPRESSION, LINK]) {
    expect(await frame.locator(".canvas").innerHTML()).not.toContain(text);
  }
  await expect(frame.locator("img, a, iframe, object")).toHaveCount(0);
  expect(refused).toEqual([]);
  expect(requests).toEqual([]);
  const sent = await page.evaluate(() => (window as unknown as { sent: string[] }).sent);
  expect(new Set(sent)).toEqual(new Set(["ui/initialize", "ui/notifications/initialized", "ui/notifications/size-changed"]));
});
