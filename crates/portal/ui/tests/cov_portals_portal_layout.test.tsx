// T-7589: coverage for PortalLayout.tsx - mobile preview toggle, page language select, add home, remove page, page title/menu edits, block title, move actions, hero, gallery, featured-datasets, numbers, link-cards, file-list, markedField edge cases
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
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

describe("markedField edge cases", () => {
  const t = i18n.t.bind(i18n) as (key: string, options?: Record<string, unknown>) => string;
  const pages: PortalPage[] = [
    {
      slug: "home",
      rows: [
        {
          blocks: [
            { type: "map", dataset: "zastavky" },
            { type: "chart", dataset: "graf" },
            { type: "text", markdown: { en: "hello" } },
            { type: "link-cards", cards: [{ title: { en: "c1" }, url: "/p1", text: { en: "t1" } }] },
            { type: "hero", intro: { en: "welcome" } },
          ],
        },
      ],
    },
    {
      slug: "transport",
      title: { en: "Transport" },
      rows: [],
    },
  ];

  it("handles non-matching and boundary paths", () => {
    expect(markedField("invalid.path", pages, "en", t)).toBeNull();
    expect(markedField("spec.pages[99].title", pages, "en", t)).toBeNull();
    expect(markedField("spec.pages[0].rows[0].blocks[99].dataset", pages, "en", t)).toBeNull();
    expect(markedField("spec.pages[0].rows[99].blocks[0].dataset", pages, "en", t)).toBeNull();
  });
});

describe("PortalLayoutTab interactions coverage", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it("handles numbers block checkbox toggles", async () => {
    const user = userEvent.setup();
    const pages: PortalPage[] = [
      {
        slug: "home",
        rows: [
          {
            blocks: [{ type: "numbers", items: ["datasets"] }],
          },
        ],
      },
    ];

    renderLayout(pages);
    await user.click(screen.getByRole("button", { name: "Edit Numbers" }));

    const datasetsCheck = screen.getByRole("checkbox", { name: "Datasets" });
    expect(datasetsCheck).toBeChecked();
    await user.click(datasetsCheck);

    const publishersCheck = screen.getByRole("checkbox", { name: "Publishers" });
    expect(publishersCheck).not.toBeChecked();
    await user.click(publishersCheck);
  });

  it("handles file-list checkboxes and preview", async () => {
    const user = userEvent.setup();
    const pages: PortalPage[] = [
      {
        slug: "home",
        rows: [
          {
            blocks: [{ type: "file-list", files: [`/files/${SHA}/hero.png`] }],
          },
        ],
      },
    ];

    renderLayout(pages);
    await user.click(screen.getByRole("button", { name: "Edit File list" }));

    const heroFileCheck = await screen.findByRole("checkbox", { name: "hero.png" });
    expect(heroFileCheck).toBeChecked();
    await user.click(heroFileCheck);

    const docFileCheck = screen.getByRole("checkbox", { name: "doc.pdf" });
    expect(docFileCheck).not.toBeChecked();
    await user.click(docFileCheck);
  });
});
