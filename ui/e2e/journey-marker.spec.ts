import { createServer } from "node:http";
import type { IncomingHttpHeaders, Server } from "node:http";
import type { AddressInfo } from "node:net";
import { gzipSync } from "node:zlib";
import { expect, test } from "@playwright/test";
import { journeyMarkerOnPortalOnly } from "./live/portal";

// The live config marks every request as a journey (`X-JC-Run-Origin`, AG-93). A Portal page that
// reads another origin (the API host's MCP route on /connect) failed that origin's CORS preflight
// on the marker, and the walker filed a console error no person meets (T-5025).

async function listen(
  handle: (headers: IncomingHttpHeaders, method: string, res: import("node:http").ServerResponse, path: string) => void,
): Promise<{ server: Server; origin: string }> {
  const server = createServer((req, res) => handle(req.headers, req.method ?? "", res, req.url ?? ""));
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  return { server, origin: `http://127.0.0.1:${(server.address() as AddressInfo).port}` };
}

test("the journey marker reaches the Portal's host and no other origin", async ({ browser }) => {
  const portalSaw: (string | undefined)[] = [];
  const otherSaw: (string | undefined)[] = [];
  // Another origin that allows a cross-origin read with content-type, as api.<domain> does.
  const other = await listen((headers, method, res) => {
    res.setHeader("access-control-allow-origin", "*");
    res.setHeader("access-control-allow-headers", "content-type");
    if (method === "OPTIONS") return res.writeHead(204).end();
    otherSaw.push(headers["x-jc-run-origin"] as string | undefined);
    res.writeHead(200, { "content-type": "text/plain" }).end("read");
  });
  const portal = await listen((headers, _method, res) => {
    portalSaw.push(headers["x-jc-run-origin"] as string | undefined);
    res.writeHead(200, { "content-type": "text/html" }).end("<!doctype html><title>Portal</title>");
  });
  const context = await browser.newContext({ extraHTTPHeaders: { "X-JC-Run-Origin": "journey" } });
  try {
    await journeyMarkerOnPortalOnly(context, new URL(portal.origin).host);
    const page = await context.newPage();
    await page.goto(portal.origin);
    const read = await page.evaluate(
      (url) => fetch(url, { headers: { "content-type": "text/plain" } }).then((answer) => answer.text(), (error: Error) => `blocked: ${error.message}`),
      `${other.origin}/api/mcp/helsinki/otvorene-data`,
    );
    expect(read, "the other origin's read is not blocked by the marker").toBe("read");
    expect(otherSaw).toEqual([undefined]);
    expect(portalSaw).toEqual(["journey"]);
  } finally {
    await context.close();
    portal.server.close();
    other.server.close();
  }
});

// Routing turns Chromium's HTTP cache off, so the walker fetched the whole immutable bundle on every
// page it opened and emptied the edge's per-IP budget, which a person's cached browser never does:
// a chunk answered 429 and the walk filed a Portal that did not boot (T-5744).
test("an immutable asset is fetched once a context, as a browser's cache does", async ({ browser }) => {
  const saw: string[] = [];
  const portal = await listen((_headers, _method, res, path) => {
    saw.push(path);
    if (path === "/") return res.writeHead(200, { "content-type": "text/html" }).end('<!doctype html><script src="/assets/app-1.js"></script><script src="/assets/busy-1.js"></script>');
    // Compressed, as the Portal answers its bundle (T-5598).
    if (path === "/assets/app-1.js")
      return res
        .writeHead(200, { "content-type": "text/javascript", "content-encoding": "gzip", "cache-control": "public, max-age=31536000, immutable" })
        .end(gzipSync("window.loaded = (window.loaded ?? 0) + 1;"));
    // A refusal is no file: the next load asks again, as a browser does.
    res.writeHead(429, { "content-type": "text/plain", "retry-after": "1" }).end("slow down");
  });
  const context = await browser.newContext();
  try {
    await journeyMarkerOnPortalOnly(context, new URL(portal.origin).host);
    const page = await context.newPage();
    for (let load = 0; load < 3; load++) {
      await page.goto(portal.origin);
      expect(await page.evaluate(() => (window as unknown as { loaded?: number }).loaded)).toBe(1);
    }
    expect(saw).toEqual(["/", "/assets/app-1.js", "/assets/busy-1.js", "/", "/assets/busy-1.js", "/", "/assets/busy-1.js"]);
  } finally {
    await context.close();
    portal.server.close();
  }
});

test.describe("a stranger's context under the live config's marker", () => {
  test.use({ extraHTTPHeaders: { "X-JC-Run-Origin": "journey" } });

  // The App probe's anonymous visit stands for a stranger, who sends no marker (T-5416).
  test("carries the marker by default and none when it clears the headers", async ({ browser }) => {
    const saw: (string | undefined)[] = [];
    const server = await listen((headers, _method, res) => {
      saw.push(headers["x-jc-run-origin"] as string | undefined);
      res.writeHead(200, { "content-type": "text/html" }).end("<!doctype html><title>App</title>");
    });
    try {
      for (const options of [{}, { extraHTTPHeaders: {} }]) {
        const context = await browser.newContext(options);
        await (await context.newPage()).goto(server.origin);
        await context.close();
      }
      expect(saw).toEqual(["journey", undefined]);
    } finally {
      server.server.close();
    }
  });
});
