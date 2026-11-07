import { setWorkerUrl } from "maplibre-gl";
import mapWorker from "maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url";
import { startApp } from "@joinedcontext/sdk";
import "@joinedcontext/sdk/style.css";
import App from "./App";
import "./components/components.css";
import "./app.css";
import tokens from "./design-tokens.json";
import { prefersDark, themed } from "./theme";

// A built bundle has no MapLibre worker beside the library's chunk, so the map would never draw:
// vite bundles the worker with its shared module into one file and this hands the library its
// address. The Portal's preview replaces it with its inline worker (the SDK's mapWorkerReady).
setWorkerUrl(mapWorker);

const dark = prefersDark();
document.documentElement.dataset.theme = dark ? "dark" : "light";
startApp(App, { tokens: themed(tokens, dark) });
