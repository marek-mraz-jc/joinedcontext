/**
 * Administration → Open data pages: Retry after a failed list, the way back from the editor, an
 * emptied text, and a preview the server refused.
 */
import { cleanup, fireEvent, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { Effective } from "../src/api/permissions";
import { OpendataPages } from "../src/pages/organization/OpendataPages";
import { json, problem, renderPage } from "./page_contract";

const ADMIN = {
  project: "org",
  bootstrap: false,
  grants: [{ role: "org-admin", binding: "admins", rule: { kinds: ["RoleBinding", "Organization"], verbs: ["read", "propose", "approve", "delete"] } }],
} as unknown as Effective;

const LIST = {
  organization: "bbsk",
  languages: ["sk"],
  maxBytes: 16384,
  pages: [{ page: "connector", lang: "sk", title: "Konektor", builtIn: "# Konektor\n\nVstavaný text." }],
};

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(cleanup);

function renderPages(list: () => Response, preview: () => Response = () => json({ title: "Konektor", html: "<p>ukážka</p>" })) {
  const reads: string[] = [];
  const previews: unknown[] = [];
  renderPage(<OpendataPages />, {
    path: "/organization/opendata-pages",
    answer: async (url, request) => {
      if (url.pathname.endsWith("/permissions/me")) return json(ADMIN);
      if (url.pathname === "/api/v1/organization/opendata/pages") {
        reads.push(url.pathname);
        return list();
      }
      if (url.pathname === "/api/v1/organization/opendata/pages/preview") {
        previews.push(await request.clone().json());
        return preview();
      }
      if (url.pathname.startsWith("/api/v1/projects/org/changes")) return json({ apiVersion: "v1", kind: "ChangeList", items: [] });
      return undefined;
    },
  });
  return { reads, previews };
}

describe("open data pages: failures and the editor's edges", () => {
  it("says the list could not be read and shows the pages after Retry", async () => {
    let healthy = false;
    const { reads } = renderPages(() => (healthy ? json(LIST) : problem(503, "The page store is restarting.")));
    expect(await screen.findByText(/The page store is restarting/)).toBeInTheDocument();
    healthy = true;
    await userEvent.setup().click(screen.getByRole("button", { name: en.app.error.retry }));
    expect(await screen.findByTestId("opendata-pages-list")).toBeInTheDocument();
    expect(reads).toHaveLength(2);
  });

  it("goes back from the editor to the list of pages", async () => {
    renderPages(() => json(LIST));
    const user = userEvent.setup();
    await user.click(await screen.findByRole("button", { name: /Edit Konektor \(sk\)/ }));
    expect(await screen.findByTestId("opendata-page-editor")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: en.opendataPages.back }));
    expect(screen.queryByTestId("opendata-page-editor")).toBeNull();
    expect(screen.getByTestId("opendata-pages-list")).toBeInTheDocument();
  });

  it("shows no preview of an emptied text and asks the server for none", async () => {
    const { previews } = renderPages(() => json(LIST));
    await userEvent.setup().click(await screen.findByRole("button", { name: /Edit Konektor \(sk\)/ }));
    expect(await screen.findByText("ukážka")).toBeInTheDocument();
    const asked = previews.length;
    fireEvent.change(screen.getByRole("textbox", { name: /Page text in Markdown/ }), { target: { value: "   " } });
    expect(screen.getByText(en.opendataPages.previewEmpty)).toBeInTheDocument();
    expect(screen.queryByText("ukážka")).toBeNull();
    expect(previews).toHaveLength(asked);
  });

  it("says why the preview could not be drawn", async () => {
    const { previews } = renderPages(
      () => json(LIST),
      () => problem(422, "the text holds a table the renderer cannot read"),
    );
    await userEvent.setup().click(await screen.findByRole("button", { name: /Edit Konektor \(sk\)/ }));
    expect(await screen.findByText(/the text holds a table the renderer cannot read/)).toBeInTheDocument();
    expect(previews[0]).toEqual({ markdown: "# Konektor\n\nVstavaný text." });
  });
});
