// T-7706: a number or boolean kept as it is in a manifest's YAML, a bigint read as text, and a class two models both declare offered once.
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import type { Manifest } from "../src/api/manifest";
import { manifestYaml } from "../src/components/grid/ManifestYamlDialog";
import { TypePicker } from "../src/components/pickers/TypePicker";
import { textOf } from "../src/pages/apps/QueryResultCard";

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("a manifest shown as YAML", () => {
  it("keeps numbers, booleans and empty values as they are, and masks only a secret's text", () => {
    const manifest = {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "DataSource",
      metadata: { name: "feed", namespace: "helsinki" },
      spec: { retries: 3, enabled: true, note: null, password: "hunter2", secretRef: { name: "feed-secret", key: "token" } },
      status: { phase: "Live" },
    } as unknown as Manifest;
    const yaml = manifestYaml(manifest);
    expect(yaml).toContain("retries: 3");
    expect(yaml).toContain("enabled: true");
    expect(yaml).toContain("note: null");
    expect(yaml).toMatch(/password: .?\*{8}.?/);
    expect(yaml).toContain("name: feed-secret");
    expect(yaml).not.toContain("hunter2");
    expect(yaml).not.toContain("status");
  });
});

describe("a query result's cell", () => {
  it("reads a value that is none of text, number, list or record as plain text", () => {
    expect(textOf(10n)).toBe("10");
    expect(textOf(Symbol.for("x"))).toBe("Symbol(x)");
  });
});

describe("the entity types a project's models offer", () => {
  it("offers a class two models both declare once", async () => {
    const user = userEvent.setup();
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const url = new URL(typeof input === "string" || input instanceof URL ? String(input) : input.url, window.location.origin);
        if (url.pathname === "/api/v1/organization/datamodels") {
          return Response.json({
            apiVersion: "joinedcontext.com/v1alpha1",
            kind: "List",
            items: [
              { name: "mobility", level: "project", project: "helsinki", version: "1.0.0", lifecycle: "published", classes: ["Road"] },
              { name: "streets", level: "project", project: "helsinki", version: "2.0.0", lifecycle: "published", classes: ["Road", "Bridge"] },
            ],
            smartDataModels: [],
          });
        }
        return new Response("not found", { status: 404 });
      }),
    );
    render(
      <I18nextProvider i18n={i18n}>
        <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
          <TypePicker label="Type" project="helsinki" value={[]} onChange={() => undefined} />
        </QueryClientProvider>
      </I18nextProvider>,
    );
    await user.click(screen.getByRole("combobox", { name: "Type" }));
    await waitFor(() => expect(screen.getAllByRole("option").length).toBeGreaterThan(0));
    const names = screen.getAllByRole("option").map((option) => option.textContent ?? "");
    expect(names.filter((text) => text.includes("Road"))).toHaveLength(1);
    expect(names.some((text) => text.includes("Bridge"))).toBe(true);
  });
});
