// The map's worker, for the built application only. MapLibre looks for its worker beside its own
// script, and a Vite build has no such file, so the map would never draw; this bundles the worker
// with the library's shared chunk and hands the library its address. index.html loads it before
// the application. The Portal's preview builds its own document and inlines the worker instead
// (the SDK's mapWorkerReady), so it never loads this file.
import { setWorkerUrl } from "maplibre-gl";
import mapWorker from "maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url";

setWorkerUrl(mapWorker);
