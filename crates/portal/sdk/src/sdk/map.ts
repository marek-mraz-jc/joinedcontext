import type { StyleSpecification } from "maplibre-gl";
import { currentTokens } from "./tokens";

export const NO_BASEMAP = "No basemap is configured";
/** What a map says when its rows carry no location; the run's check reads it as a map left empty. */
export const NO_LOCATIONS = "No locations in this data.";

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
