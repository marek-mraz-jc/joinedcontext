import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, it, vi } from "vitest";
import i18n from "../src/i18n";
import { KindList } from "../src/routes/ResourceListPage";
import { json, list, renderPage } from "./page_contract";

describe("probe", () => {
  beforeEach(async () => { await i18n.changeLanguage("en"); });
  afterEach(() => vi.unstubAllGlobals());
  it("opens", async () => {
    const seen: string[] = [];
    renderPage(<KindList project="helsinki" plural="blueprints" />, {
      answer: async (url, request) => {
        seen.push(`${request.method} ${url.pathname}${url.search}`);
        if (url.pathname.endsWith("/permissions/me")) return json({ project: "helsinki", bootstrap: true, grants: [] });
        if (url.pathname.endsWith("/blueprints")) return json(list([{ apiVersion: "joinedcontext.com/v1alpha1", kind: "Blueprint", metadata: { name: "bp", namespace: "helsinki" }, spec: { version: "1.0.0" } }]));
        return undefined;
      },
    });
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: /More actions/ }));
    await user.click(await screen.findByRole("menuitem", { name: "Edit" }));
    const d = await screen.findByRole("dialog");
    await waitFor(() => { if (/Reading/.test(d.textContent ?? "")) throw new Error("reading"); });
    (await import("node:fs")).writeFileSync("/tmp/claude-1001/-workspace/a002a5fb-b0db-4b5d-944c-294dfffccc73/scratchpad/out.txt", d.textContent + "\n" + JSON.stringify(within(d).getAllByRole("button").map((b) => b.textContent)) + "\n" + JSON.stringify(seen));
    
    
  });
});
