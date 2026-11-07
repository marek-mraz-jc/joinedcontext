import { setWorkerUrl } from "maplibre-gl";
import mapWorker from "maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url";
import { startApp } from "@joinedcontext/sdk";
import "@joinedcontext/sdk/style.css";
import App from "./App";
import "./app.css";

// A built bundle has no MapLibre worker beside the library's chunk, so the map would never draw:
// Vite bundles the worker with its shared module and this hands the library its address. The
// Portal's preview inlines its own worker instead (the SDK's mapWorkerReady).
setWorkerUrl(mapWorker);

startApp(App);
