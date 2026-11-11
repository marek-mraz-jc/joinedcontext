import type { StyleSpecification } from "maplibre-gl";
import { currentTokens } from "./tokens";

export const NO_BASEMAP = "No basemap is configured";
/** What a map says when its rows carry no location; the run's check reads it as a map left empty. */
export const NO_LOCATIONS = "No locations in this data.";

/** Why a map could not start (T-6737): its box says so instead of staying empty. */
export type MapFailure = "webgl" | "start" | "lost";
/** What the box says per failure; never the library's own text, which can carry a URL or a token. */
export const MAP_FAILED: Record<MapFailure, string> = {
  webgl:
    "This browser cannot draw the map: it needs WebGL 2, which is switched off or not supported here (often by a setting such as hardware acceleration). The table lists the same rows.",
  start: "The map could not load. The table lists the same rows.",
  lost: "The map stopped drawing because the browser took back its graphics memory. The table lists the same rows.",
};
export const MAP_RETRY = "Try the map again";
/** How long a lost graphics context may take to come back before the map counts as stopped. */
const CONTEXT_BACK_MS = 5000;

interface Startable {
  on(type: string, listener: (event: { sourceId?: string }) => void): unknown;
  remove(): void;
}

/**
 * Builds a map and watches its start (T-6737): a constructor that throws (MapLibre does when it
 * gets no WebGL 2 context), a failure without a source before the first view (the style, the
 * sprite, the glyphs), or a lost graphics context not back within CONTEXT_BACK_MS ends it and calls
 * `fail` once with the kind. One source's tile failing is a gap, not a failure. Register this
 * before the caller's own `load` listener. `dispose` removes the map and stops the watch.
 */
export function startMap<M extends Startable>(create: () => M, fail: (kind: MapFailure) => void): { map: M; dispose: () => void } | null {
  let map: M;
  try {
    map = create();
  } catch {
    fail(hasWebgl2() ? "start" : "webgl");
    return null;
  }
  let loaded = false;
  let done = false;
  let lost: ReturnType<typeof setTimeout> | undefined;
  const dispose = (): void => {
    if (done) return;
    done = true;
    clearTimeout(lost);
    map.remove();
  };
  const stop = (kind: MapFailure): void => {
    if (done) return;
    dispose();
    fail(kind);
  };
  map.on("load", () => {
    loaded = true;
  });
  map.on("error", (event) => {
    if (!loaded && !event?.sourceId) stop("start");
  });
  map.on("webglcontextlost", () => {
    clearTimeout(lost);
    lost = setTimeout(() => stop("lost"), CONTEXT_BACK_MS);
  });
  map.on("webglcontextrestored", () => clearTimeout(lost));
  return { map, dispose };
}

function hasWebgl2(): boolean {
  try {
    return Boolean(document.createElement("canvas").getContext("webgl2"));
  } catch {
    return false;
  }
}

/**
 * Resolves the basemap style: the provided URL if non-empty, or a plain background layer.
 */
export function styleFor(basemap?: string): string | StyleSpecification {
  if (basemap && basemap.trim() !== "") {
    return basemap.trim();
  }
  return {
    version: 8 as const,
    sources: {},
    layers: [
      {
        id: "background",
        type: "background",
        // No basemap: the page's own line colour, so the empty map sits in the app's look (SDK-25).
        paint: { "background-color": currentTokens().color.line },
      },
    ],
  };
}

/**
 * The worker the Portal inlined as base64 in `#kit-worker`, handed to the library by URL.
 *
 * The preview frame is sandboxed without an origin, and in Chromium such a document may start
 * a worker from a `data:` URL and from nothing else: not from `blob:`, not from the page's own
 * host. The library, for its part, wraps every URL that is not same-origin in a `blob:` module
 * that imports it, so a `data:` URL alone never reaches `new Worker`. The wrapper below hands
 * the library the `data:` URL at that moment instead. A browser that refuses `data:` workers
 * (the probe says) gets a `blob:` URL, which the library uses as it is. Without the element
 * (vite dev, the built application) the library finds its worker on its own: in vite dev
 * beside its script, in a build through the template's `map-worker.ts`, which bundles it. Resolves
 * once the choice is made, the first call making it; the map is built after it. The library is
 * imported only here and only when there is a worker to hand it, so an application without a map
 * never loads it.
 */
export function mapWorkerReady(doc: Document = document): Promise<boolean> {
  ready ??= inlineWorker(doc);
  return ready;
}

let ready: Promise<boolean> | undefined;

async function inlineWorker(doc: Document): Promise<boolean> {
  const text = doc.getElementById("kit-worker")?.textContent?.trim();
  if (!text) return false;
  const { setWorkerUrl } = await import("maplibre-gl");
  const asData = `data:text/javascript;base64,${text}`;
  const asBlob = (): string => {
    const bytes = Uint8Array.from(atob(text), (c) => c.charCodeAt(0));
    return URL.createObjectURL(new Blob([bytes], { type: "text/javascript" }));
  };
  return new Promise((resolve) => {
    const done = (url: string): void => {
      if (url.startsWith("data:")) {
        const Native = window.Worker;
        window.Worker = class extends Native {
          constructor(script: string | URL, options?: WorkerOptions) {
            super(String(script).startsWith("blob:") ? url : script, options);
          }
        };
      }
      setWorkerUrl(url);
      resolve(true);
    };
    try {
      const probe = new Worker("data:text/javascript,self.postMessage(1)");
      const timer = setTimeout(() => {
        probe.terminate();
        done(asBlob());
      }, 1500);
      probe.onmessage = () => {
        clearTimeout(timer);
        probe.terminate();
        done(asData);
      };
      probe.onerror = () => {
        clearTimeout(timer);
        probe.terminate();
        done(asBlob());
      };
    } catch {
      done(asBlob());
    }
  });
}
