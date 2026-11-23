import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ExportModal } from "../src/components/export/ExportModal";

describe("export modal when the request itself fails", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it("says what went wrong, saves nothing and keeps the dialog open", async () => {
    const asked: string[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn((input: RequestInfo | URL) => {
        const request = input as Request;
        asked.push(`${request.method} ${new URL(request.url).pathname}`);
        return Promise.reject(new TypeError("Failed to fetch"));
      }),
    );
    const onOpenChange = vi.fn();
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <ExportModal project="helsinki" target={{ plural: "endpoints", name: "public-air" }} open onOpenChange={onOpenChange} />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    await userEvent.click(screen.getByRole("button", { name: en.export.download }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Failed to fetch");
    expect(asked).toContain("GET /api/v1/projects/helsinki/export");
    expect(onOpenChange).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog")).toBeInTheDocument();
  });
});
