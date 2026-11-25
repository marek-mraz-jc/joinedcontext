// T-7706: SourcePage: unticking a page and a document, folding an opened branch, and the passages and links
// previews of a page that holds none.
import { cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { jsonResponse, list, renderRoute } from "./pageHarness";

const BASE = "/api/v1/projects/banskabystrica/knowledge/sources/bb-web";
const GRANTS = {
  project: "banskabystrica",
  bootstrap: false,
  grants: [{ role: "r", binding: "b", scope: "project:banskabystrica", rule: { kinds: ["KnowledgeSource", "Project"], verbs: ["read", "propose"] } }],
};

const page = (id: number, url: string, extra: Record<string, unknown> = {}) => ({
  id,
  url,
  depth: 0,
  parentId: null,
  status: "fetched",
  included: true,
  excludedBy: null,
  language: "sk",
  fetchedAt: "2026-10-06T03:01:00Z",
  children: 0,
  documents: 0,
  passages: 3,
  ...extra,
});

const ROOT = "https://www.banskabystrica.sk/";
const DOC = "https://www.banskabystrica.sk/vzn.pdf";

function answer(path: string): Response | undefined {
  if (path === `${BASE}/pages`) return jsonResponse({ items: [page(1, ROOT, { children: 1 })] });
  if (path === `${BASE}/documents`) {
    return jsonResponse({
      items: [{ id: 9, url: DOC, pageId: 1, mime: "application/pdf", bytes: 1000, pages: 2, offDomain: false, status: "fetched", included: true, excludedBy: null, passages: 0, title: "VZN", modified: "2026-01-15T00:00:00Z" }],
    });
  }
  if (path === `${BASE}/passages`) return jsonResponse({ items: [] });
  if (path === `${BASE}/pages/1/links`) return jsonResponse({ items: [] });
  if (path.endsWith("/knowledge/sources")) return jsonResponse(list([]));
  return undefined;
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => cleanup());

describe("SourcePage selection and folding", () => {
  it("unticks a page and a document that were ticked, and folds a branch it opened", async () => {
    await renderRoute({ path: "/projects/banskabystrica/knowledge/bb-web", permissions: GRANTS, answer });
    const box = await screen.findByRole("checkbox", { name: `Select ${ROOT}` });
    await userEvent.click(box);
    expect(screen.getByText("1 selected")).toBeInTheDocument();
    await userEvent.click(box);
    expect(screen.getByText("Nothing selected")).toBeInTheDocument();

    await userEvent.click(await screen.findByRole("button", { name: `Show the 1 pages below ${ROOT}` }));
    const hide = await screen.findByRole("button", { name: `Hide the 1 pages below ${ROOT}` });
    await userEvent.click(hide);
    expect(await screen.findByRole("button", { name: `Show the 1 pages below ${ROOT}` })).toHaveAttribute("aria-expanded", "false");

    await userEvent.click(await screen.findByRole("tab", { name: "Documents" }));
    const doc = await screen.findByRole("checkbox", { name: `Select ${DOC}` });
    await userEvent.click(doc);
    expect(screen.getByText("1 selected")).toBeInTheDocument();
    await userEvent.click(doc);
    expect(screen.getByText("Nothing selected")).toBeInTheDocument();
  });
});

describe("SourcePage previews with nothing to show", () => {
  it("says a page holds no passage and links nothing", async () => {
    await renderRoute({ path: "/projects/banskabystrica/knowledge/bb-web", permissions: GRANTS, answer });
    await userEvent.click(await screen.findByRole("button", { name: `Passages of ${ROOT}` }));
    expect(await within(await screen.findByRole("dialog")).findByText(en.knowledge.source.noPassages)).toBeInTheDocument();
    await userEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    await userEvent.click(await screen.findByRole("button", { name: `Links of ${ROOT}` }));
    expect(await within(await screen.findByRole("dialog")).findByText(en.knowledge.source.noLinks)).toBeInTheDocument();
  });
});
