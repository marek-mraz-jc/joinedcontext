/**
 * T-2867: drift is seen and resolved on the space's own page (UI-25, UI-26, CC-21). The modal
 * of T-0213 was mounted by no page, so a steward whose seed entity drifted had no screen that
 * showed it; here the router opens the space and the section reaches the modal.
 */
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { asDriftList } from "../src/pages/spaces/SpaceDrift";

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

const SPACE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ContextSpace",
  metadata: { name: "ovzdusie", namespace: "banskabystrica", title: { en: "Air quality" } },
  spec: {},
  status: { phase: "Live" },
};

const OURS = {
  space: "ovzdusie",
  id: "urn:ngsi-ld:AirQualityObserved:banskabystrica.sk:ovzdusie:station-1",
  drift: "MODIFIED",
  diff: [{ path: "pm10.value", declared: 12, live: 40 }],
  resolutions: ["revert", "adopt"],
  source: "projects/banskabystrica/spaces/ovzdusie/entities/seed/stations.json",
};
const ELSEWHERE = { ...OURS, space: "doprava", id: "urn:ngsi-ld:Road:banskabystrica.sk:doprava:r1" };
const SCANNED = { observedAt: "2026-09-25T08:00:00Z" };

type Answer = { status: number; body: unknown };

function renderSpace(drift: Answer, grants?: unknown[]) {
  const calls: string[] = [];
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input instanceof Request ? input : new Request(new URL(String(input), window.location.origin));
    const path = new URL(request.url).pathname;
    calls.push(`${request.method} ${path}`);
    const json = (body: unknown, status = 200) =>
      Promise.resolve(
        status === 204
          ? new Response(null, { status })
          : new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } }),
      );
    if (path.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    if (path.endsWith("/permissions/me")) {
      return json(grants === undefined ? { bootstrap: true, grants: [] } : { grants });
    }
    if (path.endsWith("/spaces/ovzdusie")) {
      return json(SPACE);
    }
    if (path === "/api/v1/projects/banskabystrica/drift") {
      return json(drift.body, drift.status);
    }
    if (path.endsWith("/revert")) {
      return json(null, 204);
    }
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
  return calls;
}

function list(items: unknown[], metadata: unknown = SCANNED) {
  return { apiVersion: "joinedcontext.com/v1alpha1", kind: "List", metadata, items };
}

async function section() {
  const heading = await screen.findByRole("heading", { name: en.drift.section.title });
  return heading.closest("section") as HTMLElement;
}

describe("the drift list as the page reads it", () => {
  it("keeps the scan instant and the well-formed entries, and drops the rest", () => {
    expect(asDriftList(list([OURS, { id: 3 }, { ...OURS, drift: "GONE" }, null]))).toEqual({
      observedAt: SCANNED.observedAt,
      items: [OURS],
    });
  });

  it("reads a list with no scan, or no list at all, as no scan", () => {
    expect(asDriftList(list([], {}))).toEqual({ observedAt: undefined, items: [] });
    expect(asDriftList(undefined)).toEqual({ observedAt: undefined, items: [] });
    expect(asDriftList({ metadata: { observedAt: 7 }, items: "x" })).toEqual({ observedAt: undefined, items: [] });
  });
});

describe("drift on the space page (T-2867)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/spaces/ovzdusie?tab=quality");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it("lists this space's drifted entities only, and Resolve opens the modal that reverts", async () => {
    // An entry of another shape is not shown rather than guessed at.
    const calls = renderSpace({ status: 200, body: list([OURS, ELSEWHERE, { id: 3 }]) });
    const drift = await section();
    const table = await within(drift).findByRole("table", {
      name: "Entities of ovzdusie that differ from the repository",
    });
    expect(within(table).getByText(OURS.id)).toBeInTheDocument();
    expect(within(table).queryByText(ELSEWHERE.id)).toBeNull();
    expect(within(table).getByText("pm10.value")).toBeInTheDocument();
    expect(within(drift).getByText("1 entity differs from the repository.")).toBeInTheDocument();

    await userEvent.click(within(table).getByRole("button", { name: `Resolve the drift of ${OURS.id}` }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("40")).toBeInTheDocument();
    await userEvent.click(within(dialog).getByRole("button", { name: en.drift.modal.revert }));

    const revert = `POST /api/v1/projects/banskabystrica/drift/ovzdusie/${encodeURIComponent(OURS.id)}/revert`;
    await vi.waitFor(() => expect(calls).toContain(revert));
    // The list is read again after a resolution: the next scan is the answer.
    await vi.waitFor(() =>
      expect(calls.filter((call) => call === "GET /api/v1/projects/banskabystrica/drift").length).toBeGreaterThan(1),
    );
  });

  it("says no scan has run, rather than that nothing drifted", async () => {
    renderSpace({ status: 200, body: list([], {}) });
    const drift = await section();
    expect(await within(drift).findByText(en.drift.section.notYet)).toBeInTheDocument();
    expect(within(drift).queryByText(en.drift.section.clean)).toBeNull();
  });

  it("says the space is clean when the scan found nothing of it", async () => {
    renderSpace({ status: 200, body: list([ELSEWHERE]) });
    const drift = await section();
    expect(await within(drift).findByText(en.drift.section.clean)).toBeInTheDocument();
    expect(within(drift).queryByRole("table")).toBeNull();
  });

  it("offers no Resolve to a person who may not propose an Entity, and says why", async () => {
    renderSpace({ status: 200, body: list([OURS]) }, [{ rule: { kinds: ["Entity"], verbs: ["read"] } }]);
    const drift = await section();
    expect(await within(drift).findByText(en.drift.section.mayNotResolve)).toBeInTheDocument();
    expect(within(drift).queryByRole("button", { name: /Resolve/ })).toBeNull();
  });

  it("shows a failed read with a retry, never an empty or clean list", async () => {
    renderSpace({ status: 503, body: { title: "Service Unavailable", detail: "the scan store is down" } });
    const drift = await section();
    expect(await within(drift).findByRole("button", { name: en.app.error.retry })).toBeInTheDocument();
    expect(within(drift).queryByText(en.drift.section.clean)).toBeNull();
    expect(within(drift).queryByText(en.drift.section.notYet)).toBeNull();
  });
});
