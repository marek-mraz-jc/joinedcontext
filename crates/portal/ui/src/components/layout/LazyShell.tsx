import { lazy } from "react";

/**
 * What the router draws a signed-in page in (T-5122): the shell loads with the first such page,
 * so a visitor of the public catalogue never downloads the assistant, the menus or the palette.
 * Vitest maps this module to `Shell` itself (vite.config.ts), so a test reads the frame at once.
 */
export const Shell = lazy(() => import("./Shell").then((module) => ({ default: module.Shell })));
