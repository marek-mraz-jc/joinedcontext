/** T-7589: coverage for PortalLayout components, block editors, preview, and page mutations. */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import en from "../src/locales/en.json";
import i18n from "../src/i18n";
import { markedField, PortalLayoutTab } from "../src/pages/portals/PortalLayout";
import type { PortalPage } from "../src/pages/portals/portalPages";

const SHA = "a".repeat(64);
const FILES = {
  items: [
    { id: "f-1", name: "hero.png", contentType: "image/png", size: 2048, sha256: SHA, path: `/files/${SHA}/hero.png`, uploadedBy: "Ana", uploadedAt: "2026-11-19T19:00:00Z", usedBy: [] },
    { id: "f-2", name: "doc.pdf", contentType: "application/pdf", size: 4096, sha256: "b".repeat(64), path: `/files/${"b".repeat(64)}/doc.pdf`, uploadedBy: "Ana", uploadedAt: "2026-11-19T19:00:00Z", usedBy: [] },
  ],
  limits: { files: 2, bytes: 6144, maxFiles: 500, maxBytes: 200 * 1024 * 1024, maxFileBytes: 10 * 1024 * 1024 },
};
const DATASETS = {
  total: 2,
  items: [
    { name: "zastavky", title: "Zastávky MHD", source: "endpoints", hidden: false },
    { name: "linky", title: "Linky MHD", source: "endpoints", hidden: false },
  ],
};

function renderLayout(pages: PortalPage[], languages = ["en", "sk"]) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL(typeof input === "string" ? input : input instanceof URL ? input.href : input.url);
      if (url.pathname.endsWith("/files")) return new Response(JSON.stringify(FILES), { status: 200, headers: { "Content-Type": "application/json" } });
      if (url.pathname.endsWith("/datasets")) return new Response(JSON.stringify(DATASETS), { status: 200, headers: { "Content-Type": "application/json" } });
      if (url.pathname.includes("/changes/history")) return new Response(JSON.stringify({ items: [] }), { status: 200, headers: { "Content-Type": "application/json" } });
      return new Response("{}", { status: 404 });
    }),
  );

  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <PortalLayoutTab
          name="bbsk-mobility"
          spec={{
            organizationRef: "bbsk",
            path: "mobility",
            languages,
            pages,
          }}
        />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("PortalLayoutTab coverage pass 2", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("handles markedField paths for page title and slug", () => {
    const t = i18n.t.bind(i18n) as (key: string, options?: Record<string, unknown>) => string;
    const pages: PortalPage[] = [
      { slug: "home", title: { en: "Home" }, rows: [] },
      { slug: "doprava", title: { en: "Transport" }, rows: [] },
    ];

    const titleField = markedField("spec.pages[1].title", pages, "en", t);
    expect(titleField).toEqual({
      page: 1,
      name: `Transport: ${en.portals.layout.pageTitle}`,
      target: "layout-page-title",
    });

    const slugField = markedField("spec.pages[1].slug", pages, "en", t);
    expect(slugField).toEqual({
      page: 1,
      name: `Transport: ${en.portals.layout.slug}`,
      target: "",
    });
  });

  it("toggles mobile preview button and language selector in PageBar", async () => {
    const user = userEvent.setup();
    const pages: PortalPage[] = [{ slug: "home", rows: [] }];

    renderLayout(pages, ["en", "sk"]);

    const previewToggle = screen.getByRole("button", { name: en.portals.layout.toPreview });
    await user.click(previewToggle);
    expect(screen.getByRole("button", { name: en.portals.layout.hidePreview })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: en.portals.layout.hidePreview }));
    expect(screen.getByRole("button", { name: en.portals.layout.toPreview })).toBeInTheDocument();

    const langSelect = screen.getByLabelText(en.portals.layout.language);
    fireEvent.change(langSelect, { target: { value: "sk" } });
  });

  it("handles link-cards block editing, card addition and removal", async () => {
    const user = userEvent.setup();
    const pages: PortalPage[] = [
      {
        slug: "home",
        rows: [
          {
            blocks: [
              {
                type: "link-cards",
                cards: [{ title: { en: "Card 1" }, url: "/p1", text: { en: "Description 1" } }],
              },
            ],
          },
        ],
      },
    ];

    renderLayout(pages);

    expect(screen.getAllByText("Card 1").length).toBeGreaterThan(0);

    const editBtn = screen.getByRole("button", { name: "Edit Link cards" });
    await user.click(editBtn);

    const titleInput = screen.getByDisplayValue("Card 1");
    fireEvent.change(titleInput, { target: { value: "Updated Card" } });

    const urlInput = screen.getByDisplayValue("/p1");
    fireEvent.change(urlInput, { target: { value: "/p1-updated" } });

    const textInput = screen.getByDisplayValue("Description 1");
    fireEvent.change(textInput, { target: { value: "Updated Text" } });

    const addCardBtn = screen.getByRole("button", { name: en.portals.layout.addCard });
    await user.click(addCardBtn);

    const removeBtns = screen.getAllByRole("button", { name: en.portals.layout.removeCard });
    expect(removeBtns.length).toBe(2);
    await user.click(removeBtns[1]);
  });
});
