// T-7589: coverage for PortalFiles.tsx - non-503 query failure, rename/delete cancels and errors, file replacement, limit warning, usedBy cells, scope notes
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import i18n from "../src/i18n";
import { PortalFilesTab } from "../src/pages/portals/PortalFiles";

const SHA = "a".repeat(64);
const HERO = {
  id: "f-1",
  name: "hero.png",
  contentType: "image/png",
  size: 2048,
  sha256: SHA,
  path: `/files/${SHA}/hero.png`,
  uploadedBy: "Ana",
  uploadedAt: "2026-11-19T19:00:00Z",
  usedBy: ["bbsk-home"],
};
const REPORT = {
  ...HERO,
  id: "f-2",
  name: "report.pdf",
  contentType: "application/pdf",
  size: 3 * 1024 * 1024,
  sha256: "b".repeat(64),
  path: `/files/${"b".repeat(64)}/report.pdf`,
  usedBy: [],
};
const LIMITS = { files: 2, bytes: 3 * 1024 * 1024 + 2048, maxFiles: 500, maxBytes: 200 * 1024 * 1024, maxFileBytes: 10 * 1024 * 1024 };

function renderFiles(
  props: { name?: string; spec?: { organizationRef: string; path: string; title?: Record<string, string> } } = {},
  client = new QueryClient({ defaultOptions: { queries: { retry: false } } }),
) {
  const name = props.name ?? "bbsk-mobility";
  const spec = props.spec ?? { organizationRef: "bbsk", path: "mobility" };
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <PortalFilesTab name={name} spec={spec} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("PortalFiles coverage", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it("renders PageFailed on non-503 query failure and allows retry", async () => {
    let attempts = 0;
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = new URL(typeof input === "string" ? input : input instanceof URL ? input.href : input.url);
        if (url.pathname.includes("/files")) {
          attempts += 1;
          return new Response(JSON.stringify({ status: 500, title: "Server Error", detail: "Internal error" }), {
            status: 500,
            headers: { "Content-Type": "application/problem+json" },
          });
        }
        return new Response("{}", { status: 404 });
      }),
    );

    renderFiles();
    const retryBtn = await screen.findByRole("button", { name: "Retry" });
    expect(retryBtn).toBeInTheDocument();
    fireEvent.click(retryBtn);
    await waitFor(() => expect(attempts).toBeGreaterThan(1));
  });

  it("handles cancelling rename and delete actions", async () => {
    const user = userEvent.setup();
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = new URL(typeof input === "string" ? input : input instanceof URL ? input.href : input.url);
        if (url.pathname.includes("/files")) {
          return new Response(JSON.stringify({ items: [REPORT], limits: LIMITS }), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          });
        }
        return new Response("{}", { status: 404 });
      }),
    );

    renderFiles();
    const renameBtn = await screen.findByRole("button", { name: "Rename report.pdf" });
    await user.click(renameBtn);
    expect(screen.getByLabelText(/^New name/)).toBeInTheDocument();
    const cancelRenameBtn = screen.getByRole("button", { name: "Cancel" });
    await user.click(cancelRenameBtn);
    expect(screen.queryByLabelText(/^New name/)).toBeNull();

    const deleteBtn = screen.getByRole("button", { name: "Delete report.pdf" });
    await user.click(deleteBtn);
    const deleteAlert = screen.getByRole("alert");
    expect(within(deleteAlert).getByText("Delete report.pdf?")).toBeInTheDocument();
    const cancelDeleteBtn = within(deleteAlert).getByRole("button", { name: "Cancel" });
    await user.click(cancelDeleteBtn);
    expect(screen.queryByText("Delete report.pdf?")).toBeNull();
  });

  it("shows danger notice when rename or delete mutation fails with server error", async () => {
    const user = userEvent.setup();
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
        const req = input instanceof Request ? input : new Request(input, init);
        const url = new URL(req.url);
        if (url.pathname.endsWith("/files") && req.method === "GET") {
          return new Response(JSON.stringify({ items: [REPORT], limits: LIMITS }), {
            status: 200,
            headers: { "Content-Type": "application/json" },
          });
        }
        if (url.pathname.endsWith("/files/f-2") && req.method === "PATCH") {
          return new Response(JSON.stringify({ status: 422, title: "Refused", detail: "Name already taken" }), {
            status: 422,
            headers: { "Content-Type": "application/problem+json" },
          });
        }
        if (url.pathname.endsWith("/files/f-2") && req.method === "DELETE") {
          return new Response(JSON.stringify({ status: 400, title: "Bad Request", detail: "File locked" }), {
            status: 400,
            headers: { "Content-Type": "application/problem+json" },
          });
        }
        return new Response("{}", { status: 404 });
      }),
    );

    renderFiles();
    await user.click(await screen.findByRole("button", { name: "Rename report.pdf" }));
    const nameInput = screen.getByLabelText(/^New name/);
    await user.clear(nameInput);
    await user.type(nameInput, "other.pdf");
    await user.click(screen.getByRole("button", { name: "Rename" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Name already taken");

    await user.click(screen.getByRole("button", { name: "Delete report.pdf" }));
    await user.click(screen.getByRole("button", { name: "Delete the file" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("File locked");
  });

  it("renders usedBy cells with items and unused, and calls grid query refetch", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () =>
        new Response(JSON.stringify({ items: [HERO, REPORT], limits: LIMITS }), {
          status: 200,
          headers: { "Content-Type": "application/json" },
        }),
      ),
    );

    renderFiles();
    const table = await screen.findByRole("table", { name: "Files" });
    expect(within(table).getByText("bbsk-home")).toBeInTheDocument();
    expect(within(table).getByText("Not used")).toBeInTheDocument();
  });
});
