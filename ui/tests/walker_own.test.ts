import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { OWN_ROUTES } from "./pageChecks";

// T-6010: the viewer walk flagged "New AI connector" on /ai-connectors as a write a viewer may not
// make. An AI connector is the person's own (AG-142): the server keeps it inside the person's rights
// (connectors.rs `reachable`), so the page is one of the person's own things, as /connectors is.
describe("pages of the person's own things", () => {
  it("count the AI connectors page as the person's own", () => {
    expect(OWN_ROUTES.has("/ai-connectors")).toBe(true);
    expect(OWN_ROUTES.has("/connectors")).toBe(true);
  });

  it("name only routes the router declares, none inside a project", () => {
    const router = readFileSync(join(dirname(fileURLToPath(import.meta.url)), "..", "src", "router.tsx"), "utf8");
    for (const route of OWN_ROUTES) {
      expect(router).toContain(`path: "${route}"`);
      expect(route.startsWith("/projects")).toBe(false);
    }
  });
});
