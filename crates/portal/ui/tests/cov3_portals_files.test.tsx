// Files tab, remaining branches: replacing a file, a transport failure with no message, grid search over every column.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import en from "../src/locales/en.json";
import i18n from "../src/i18n";
import { PortalFilesTab } from "../src/pages/portals/PortalFiles";

const REPORT = {
  id: "f-2",
  name: "report.pdf",
  contentType: "application/pdf",
  size: 3 * 1024 * 1024,
  sha256: "b".repeat(64),
  path: `/files/${"b".repeat(64)}/report.pdf`,
  uploadedBy: "Ana",
  uploadedAt: "2026-11-19T19:00:00Z",
  usedBy: [] as string[],
};
const LIMITS = { files: 1, bytes: 3 * 1024 * 1024, maxFiles: 500, maxBytes: 200 * 1024 * 1024, maxFileBytes: 10 * 1024 * 1024 };

function renderFiles() {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <PortalFilesTab name="bbsk-mobility" spec={{ organizationRef: "bbsk", path: "mobility" }} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

function upload(input: HTMLElement, file: File) {
  fireEvent.change(input, { target: { files: [file] } });
}

function stub(onWrite: (request: Request) => Response | Promise<Response>) {
  const writes: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(input, init);
      if (request.method === "GET") return new Response(JSON.stringify({ items: [REPORT], limits: LIMITS }), { status: 200, headers: { "Content-Type": "application/json" } });
      writes.push(`${request.method} ${new URL(request.url).pathname}`);
      return onWrite(request);
    }),
  );
  return writes;
}

describe("PortalFiles, more branches", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it("replaces the contents of an unused file and says so", async () => {
    const writes = stub(() => new Response(JSON.stringify(REPORT), { status: 200, headers: { "Content-Type": "application/json" } }));
    renderFiles();
    const picker = await screen.findByLabelText("Replace the contents of report.pdf");
    upload(picker, new File(["%PDF"], "new.pdf", { type: "application/pdf" }));
    expect(await screen.findByText("report.pdf was replaced. Its address changed with its contents.")).toBeInTheDocument();
    expect(writes).toEqual(["PUT /api/v1/projects/org/portals/bbsk-mobility/files/f-2"]);
  });

  it("falls back to the generic text when the transport fails without a message", async () => {
    stub(() => {
      throw new Error("");
    });
    renderFiles();
    const picker = await screen.findByLabelText("Replace the contents of report.pdf");
    upload(picker, new File(["%PDF"], "new.pdf", { type: "application/pdf" }));
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(`new.pdf: ${i18n.t("app.error.generic")}`);
  });

  it("names the HTTP status when a refused replacement carries no problem text", async () => {
    stub(() => new Response("{}", { status: 500, headers: { "Content-Type": "application/json" } }));
    renderFiles();
    upload(await screen.findByLabelText("Replace the contents of report.pdf"), new File(["%PDF"], "new.pdf"));
    expect(await screen.findByRole("alert")).toHaveTextContent("new.pdf: HTTP 500");
  });

  it("searches every column, so a word in none of them leaves the grid empty", async () => {
    const user = userEvent.setup();
    stub(() => new Response(null, { status: 204 }));
    renderFiles();
    const table = await screen.findByRole("table", { name: "Files" });
    expect(within(table).getByText("report.pdf")).toBeInTheDocument();
    await user.type(screen.getByRole("searchbox"), "zzz-nothing");
    expect(await screen.findByText("No rows match the search and filters.")).toBeInTheDocument();
    expect(screen.queryByText("report.pdf")).toBeNull();
  });

  it("shows the address column once it is chosen in Arrange columns", async () => {
    const user = userEvent.setup();
    stub(() => new Response(null, { status: 204 }));
    renderFiles();
    const table = await screen.findByRole("table", { name: "Files" });
    expect(within(table).queryByText(REPORT.path)).toBeNull();
    await user.click(screen.getByRole("button", { name: en.entityGrid.arrangeColumns }));
    const panel = screen.getByRole("dialog", { name: en.entityGrid.arrangeColumns });
    await user.click(within(panel).getByRole("checkbox", { name: `${en.entityGrid.showColumn}: Address` }));
    expect(within(await screen.findByRole("table", { name: "Files" })).getByText(REPORT.path)).toBeInTheDocument();
  });
});
