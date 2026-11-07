/**
 * T-6404: the sweep runs several specs at once, and every sign-in opens a fresh browser context
 * that loads the Portal's ~60 bundle chunks again. The edge allows 300 requests a minute per
 * address on the Portal host (deployment components/portal/apisix-plugins.yaml, route portal-ui),
 * and the swarm shares one address, so four workers signing in together would meet its 429. The
 * chunks are content-hashed (`/assets/index-<hash>.js`): a worker fetches each one once through
 * the edge and hands the same bytes to its later contexts. A refused or failed chunk is not kept.
 *
 * With SWEEP_OUT set, each worker also counts the requests its signed-in contexts sent to the edge
 * per minute (chunks it answered itself left out) into SWEEP_OUT/requests-<pid>.json, which
 * sweep.sh adds up, so a run shows how close it came to the edge's limits.
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import type { BrowserContext, Request } from "@playwright/test";

export interface HeldAsset {
  status: number;
  headers: Record<string, string>;
  body: Buffer;
}

/** The headers of a fetched answer that still hold for its decoded body. */
export function replayHeaders(headers: Record<string, string>): Record<string, string> {
  const dropped = new Set(["content-encoding", "content-length", "transfer-encoding"]);
  return Object.fromEntries(Object.entries(headers).filter(([name]) => !dropped.has(name.toLowerCase())));
}

/** Whether an answer may be handed to another context: only a whole 200. */
export function keepable(asset: HeldAsset): boolean {
  return asset.status === 200;
}

/** The minute (UTC, `YYYY-MM-DDTHH:MM`) a request was sent in. */
export function minuteOf(ms: number): string {
  return new Date(ms).toISOString().slice(0, 16);
}

const held = new Map<string, Promise<HeldAsset>>();
const perMinute = new Map<string, number>();
const answeredHere = new WeakSet<Request>();
let written = false;

function portalHost(): string {
  return new URL(process.env.PORTAL_URL ?? "https://portal.dev.joinedcontext.com").host;
}

function writeCounts(dir: string): void {
  if (!perMinute.size) return;
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, `requests-${process.pid}.json`), JSON.stringify(Object.fromEntries(perMinute)));
}

/** Shares the bundle's chunks between this worker's contexts and counts what reaches the edge. */
export async function shareEdge(context: BrowserContext): Promise<void> {
  const portal = portalHost();
  const domain = portal.replace(/^portal\./, "");
  const out = process.env.SWEEP_OUT;
  if (out) {
    if (!written) {
      written = true;
      process.on("exit", () => writeCounts(out));
    }
    context.on("requestfinished", (request) => {
      const host = new URL(request.url()).host;
      if (answeredHere.has(request) || !(host === domain || host.endsWith(`.${domain}`))) return;
      const minute = minuteOf(Date.now());
      perMinute.set(minute, (perMinute.get(minute) ?? 0) + 1);
    });
  }
  // WALK_UI_DIST serves /assets/ from a local build (portal.ts withLocalUi): nothing to share then.
  if (process.env.WALK_UI_DIST) return;
  await context.route(
    (url) => url.host === portal && url.pathname.startsWith("/assets/"),
    async (route) => {
      const url = route.request().url();
      let asset = held.get(url);
      if (!asset) {
        asset = route.fetch().then(async (answer) => ({ status: answer.status(), headers: answer.headers(), body: await answer.body() }));
        held.set(url, asset);
        // The context that fetched it closed or the edge refused it: the next context asks again.
        asset.then((got) => keepable(got) || held.delete(url), () => held.delete(url));
      } else {
        answeredHere.add(route.request());
      }
      let got: HeldAsset;
      try {
        got = await asset;
      } catch {
        return route.continue();
      }
      return route.fulfill({ status: got.status, headers: replayHeaders(got.headers), body: got.body });
    },
  );
}
