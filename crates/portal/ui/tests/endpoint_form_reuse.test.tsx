import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { findFormPage } from "./formPage";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

const SPACES = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "ContextSpace",
      metadata: { name: "ovzdusie", namespace: "banskabystrica" },
      spec: {},
      status: { phase: "Live" },
    },
  ],
};

const DATAMODELS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "DataModel",
      metadata: { name: "mobility-model", namespace: "banskabystrica" },
      spec: {
        contextSpaceRef: "ovzdusie",
        linkml: "id: m\nname: m\nclasses:\n  Car:\n    slots: [id, type, speed]\nslots:\n  id: {range: string}\n  type: {range: string}\n  speed: {range: integer}\n",
      },
    },
  ],
};

const PROJECTIONS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "ModelProjection",
      metadata: { name: "shared-cars", namespace: "banskabystrica" },
      spec: {
        contextSpaceRef: "ovzdusie",
        classes: [{ name: "Car", slots: ["speed"] }],
      },
    },
  ],
};

const ENDPOINTS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Endpoint",
      metadata: { name: "first-endpoint", namespace: "banskabystrica" },
      spec: {
        contextSpaceRef: "ovzdusie",
        slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa",
        audience: "project-list",
        enabledRepresentations: ["ngsi-ld"],
        projectionRef: { kind: "ModelProjection", name: "shared-cars" },
      },
    },
  ],
};

describe("endpoint form projection reuse (T-0564)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/endpoints");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("selecting existing projection displays sharing endpoints and locks tree until detach", async () => {
    const fetchMock = vi.fn((input: RequestInfo | URL) => {
      const request = input as Request;
      const url = typeof input === "string" ? new URL(input) : new URL(request.url);
      const json = (body: unknown, status = 200) =>
        Promise.resolve(
          new Response(JSON.stringify(body), {
            status,
            headers: { "Content-Type": "application/json" },
          }),
        );

      if (url.pathname.endsWith("/auth/me")) return json(IDENTITY);
      if (url.pathname.endsWith("/spaces")) return json(SPACES);
      if (url.pathname.endsWith("/datamodels")) return json(DATAMODELS);
      if (url.pathname.endsWith("/projections")) return json(PROJECTIONS);
      if (url.pathname.endsWith("/endpoints")) return json(ENDPOINTS);
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
    });
    vi.stubGlobal("fetch", fetchMock);

    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(
      <QueryClientProvider client={client}>
        <I18nextProvider i18n={i18n}>
          <App />
        </I18nextProvider>
      </QueryClientProvider>,
    );

    await userEvent.click(await screen.findByRole("button", { name: en.endpoints.add }));
    const dialog = await findFormPage();

    await waitFor(() => expect(dialog.querySelector("#projection-reuse")).not.toBeNull());
    const reuseSelect = dialog.querySelector("#projection-reuse") as HTMLSelectElement;
    await userEvent.selectOptions(reuseSelect, "shared-cars");

    // Shows which endpoints reference it
    expect(await within(dialog).findByText(/first-endpoint/)).toBeInTheDocument();

    // Checkbox is read-only / disabled
    const carCheckbox = within(dialog).getByLabelText("Car");
    expect(carCheckbox).toBeDisabled();

    // Click detach
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.picker.detach }));

    // Now unlocked
    expect(carCheckbox).toBeEnabled();
  });
});
