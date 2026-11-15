import { setWorkerUrl } from "maplibre-gl";
import mapWorker from "maplibre-gl/dist/maplibre-gl-worker.mjs?worker&url";
import { startApp } from "@joinedcontext/sdk";
import "@joinedcontext/sdk/style.css";
import App from "./App";
import "./components/components.css";
import "./app.css";
import { startTokens } from "./theme";

// A built bundle has no MapLibre worker beside the library's chunk: vite bundles it and this
// hands the library its address (as helsinki-events does).
setWorkerUrl(mapWorker);

startApp(App, { tokens: startTokens() });
