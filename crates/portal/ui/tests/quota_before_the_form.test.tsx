/**
 * A project at its quota says so before the person types (T-1594; PF-73, PF-75, UI-01).
 *
 * `spaces_view.test.tsx::blocks a new space once the quota is used up` holds this for Context
 * Spaces, and for two of the three dimensions a person meets in a form there was nothing: a
 * project at its `residentPipelines` or `publicEndpoints` limit let the person choose a class or an
 * audience, fill the form, check it and submit — and only then met the refusal
 * (`src/quotas.rs::the_next_one_over_the_limit_is_refused_and_the_message_names_both_numbers`).
 *
 * Why the control is not disabled here, unlike on the spaces page: a scheduled pipeline does not
 * count against `residentPipelines`, and an endpoint for the project or the organization does not
 * count against `publicEndpoints`. Disabling `New pipeline` would refuse what the server accepts.
 * The page says which dimension is used up and what it costs, and leaves the choice.
 */
import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";

const PROJECT = "banskabystrica";
const IDENTITY = { subject: "b7c1e0f4", username: "jana.kovacova", roles: ["portal-approver"] };

/** The project as its own route answers it: what it holds of each dimension, and what it may. */
function project(usage: Record<string, { used: number; limit?: number }>) {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Project",
    metadata: { name: PROJECT, namespace: "org" },
    spec: { title: "Banská Bystrica" },
    status: { usage },
  };
}

function renderAt(route: string, usage: Record<string, { used: number; limit?: number }>) {
  const json = (body: unknown, status = 200) =>
    Promise.resolve(
      new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } }),
    );
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const path = new URL((input as Request).url).pathname;
      if (path.endsWith("/auth/me")) {
        return json(IDENTITY);
      }
      if (path === `/api/v1/projects/${PROJECT}`) {
        return json(project(usage));
      }
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
    }),
  );
  window.history.pushState({}, "", route);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <App />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the page says which quota is used up before a form is opened (T-1594)", () => {
  it("names the resident-pipeline quota on the pipelines page", async () => {
    renderAt(`/projects/${PROJECT}/pipelines`, { residentPipelines: { used: 4, limit: 4 } });

    await waitFor(() =>
      expect(screen.getByText(/quota of 4 Resident pipelines is used up/)).toBeInTheDocument(),
    );
    // The control stays, in the header and in the empty state alike: a scheduled pipeline is still
    // allowed, and disabling it would refuse what the server accepts.
    for (const control of screen.getAllByRole("button", { name: en.pipelines.add })) {
      expect(control).toBeEnabled();
    }
  });

  it("names the public-endpoint quota on the endpoints page", async () => {
    renderAt(`/projects/${PROJECT}/endpoints`, { publicEndpoints: { used: 2, limit: 2 } });

    await waitFor(() =>
      expect(screen.getByText(/quota of 2 Public endpoints is used up/)).toBeInTheDocument(),
    );
    for (const control of screen.getAllByRole("button", { name: en.endpoints.add })) {
      expect(control).toBeEnabled();
    }
  });

  it("says nothing while the project is inside its quota, or has no limit at all", async () => {
    renderAt(`/projects/${PROJECT}/pipelines`, { residentPipelines: { used: 1, limit: 4 } });
    await waitFor(() =>
      expect(screen.getAllByRole("button", { name: en.pipelines.add }).length).toBeGreaterThan(0),
    );
    expect(screen.queryByText(/is used up/)).not.toBeInTheDocument();

    vi.unstubAllGlobals();
    renderAt(`/projects/${PROJECT}/endpoints`, { publicEndpoints: { used: 9 } });
    await waitFor(() =>
      expect(screen.getAllByRole("button", { name: en.endpoints.add }).length).toBeGreaterThan(0),
    );
    expect(screen.queryByText(/is used up/)).not.toBeInTheDocument();
  });
});
