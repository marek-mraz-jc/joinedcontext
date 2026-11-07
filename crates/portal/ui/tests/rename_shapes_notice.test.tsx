/**
 * The one click of AP-124 on the Apps page (T-2940): a project that still holds `static` or
 * `fullstack` Apps is told which and what they become, and one button proposes the one Change in
 * the person's name; nobody without propose on App gets a working button, and nothing shows
 * when there is nothing to rename.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/apps/RenameShapesNotice.tsx.
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Change, Manifest } from "../src/api/manifest";
import { oldShapeApps, RenameShapesNotice } from "../src/pages/apps/RenameShapesNotice";
import { expectNoViolations } from "./checks";
import { renderPage } from "./page_contract";

const V = "joinedcontext.com/v1alpha1";
const app = (name: string, kind: string) =>
  ({ apiVersion: V, kind: "App", metadata: { name, namespace: "helsinki" }, spec: { kind } }) as unknown as Manifest;
const APPS = [app("air-map", "static"), app("bus-desk", "fullstack"), app("bikes", "ui")];
const CHANGE = {
  apiVersion: V,
  kind: "Change",
  metadata: { name: "chg-00000042", namespace: "helsinki" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { create: 0, update: 2, delete: 0 } },
};

function renderNotice({ apps = APPS, verbs = ["read", "propose"], answer = 202 } = {}) {
  const sent: string[] = [];
  const proposed = vi.fn<(change: Change) => void>();
  renderPage(<RenameShapesNotice project="helsinki" apps={apps} proposed={false} onProposed={proposed} />, {
    path: "/projects/helsinki/apps",
    answer: async (url, request) => {
      if (url.pathname.endsWith("/permissions/me")) {
        return new Response(JSON.stringify({ project: "helsinki", bootstrap: false, grants: [{ rule: { kinds: ["App"], verbs } }] }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        });
      }
      if (request.method === "POST") {
        sent.push(url.pathname);
        return answer === 202
          ? new Response(JSON.stringify(CHANGE), { status: 202, headers: { "Content-Type": "application/json" } })
          : new Response(JSON.stringify({ type: "about:blank", title: "Conflict", status: 409, detail: "nothing to rename (AP-124)" }), {
              status: 409,
              headers: { "Content-Type": "application/problem+json" },
            });
      }
      return undefined;
    },
  });
  return { sent, proposed };
}

const button = () => screen.findByRole("button", { name: en.apps.renameShapes.action });

describe("the rename notice", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it("finds exactly the Apps with an old name and what each becomes", () => {
    expect(oldShapeApps(APPS)).toEqual([
      { name: "air-map", from: "static", to: "ui" },
      { name: "bus-desk", from: "fullstack", to: "ui-rust" },
    ]);
    expect(oldShapeApps([app("bikes", "ui"), app("odd", "")])).toEqual([]);
  });

  it("names the Apps and proposes one Change with one click", async () => {
    const { sent, proposed } = renderNotice();
    expect(await screen.findByText("air-map: static becomes ui")).toBeInTheDocument();
    expect(screen.getByText("bus-desk: fullstack becomes ui-rust")).toBeInTheDocument();
    expect(screen.queryByText(/bikes/)).toBeNull();
    const propose = await button();
    await waitFor(() => expect(propose).not.toHaveAttribute("aria-disabled"));
    await expectNoViolations(document.body);
    await userEvent.click(propose);
    await waitFor(() => expect(proposed).toHaveBeenCalledTimes(1));
    expect(sent).toEqual(["/api/v1/projects/helsinki/apps/rename-shapes"]);
    expect(proposed.mock.calls[0][0].metadata.name).toBe("chg-00000042");
    // Once proposed, the button is gone: the page's own notice follows the Change.
    await waitFor(() => expect(screen.queryByRole("button", { name: en.apps.renameShapes.action })).toBeNull());
  });

  it("shows nothing when there is nothing to rename", async () => {
    renderNotice({ apps: [app("bikes", "ui")] });
    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(screen.queryByRole("button", { name: en.apps.renameShapes.action })).toBeNull();
  });

  it("keeps the button in place, disabled with the reason, for someone who may not propose an App", async () => {
    const { sent } = renderNotice({ verbs: ["read"] });
    const propose = await button();
    await waitFor(() => expect(propose).toHaveAttribute("aria-disabled", "true"));
    await userEvent.click(propose);
    expect(sent).toEqual([]);
  });

  it("says a refusal in words and keeps the button", async () => {
    renderNotice({ answer: 409 });
    const propose = await button();
    await waitFor(() => expect(propose).not.toHaveAttribute("aria-disabled"));
    await userEvent.click(propose);
    // The page shows the server's sentence without its requirement id.
    expect(await screen.findByRole("alert")).toHaveTextContent("nothing to rename");
    expect(await button()).toBeInTheDocument();
  });
});
