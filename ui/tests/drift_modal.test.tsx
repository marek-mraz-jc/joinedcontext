/**
 * T-0213: the two resolutions UI-26 puts on the screen, and only those (CC-21, CC-38).
 *
 * A drifted seed entity is the one thing that can drift here — configuration is read from the
 * repository, so it is what is running (CC-72) — and the operator gets exactly two buttons:
 * revert, which writes the repository back into the space, and adopt, which proposes what is
 * live as the declared value and is the only way an out-of-band change becomes durable.
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { DriftResolutionModal } from "../src/components/drift/DriftResolutionModal";
import type { DriftedEntity } from "../src/components/drift/DriftResolutionModal";

const PROJECT = "helsinki";

const MODIFIED: DriftedEntity = {
  space: "helsinki",
  id: "urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:station-1",
  drift: "MODIFIED",
  diff: [{ path: "airQualityIndex.value", declared: 42, live: 7 }],
  resolutions: ["revert", "adopt"],
  source: "projects/helsinki/spaces/helsinki/entities/seed/stations.json",
};

const MISSING: DriftedEntity = {
  ...MODIFIED,
  id: "urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:station-2",
  drift: "MISSING",
  diff: [],
  resolutions: ["revert"],
};

/** The path the client builds: `{id}` is the entity's URN, percent-encoded (API/01 §20). */
function route(entity: DriftedEntity, resolution: string): string {
  return `POST /api/v1/projects/${PROJECT}/drift/${entity.space}/${encodeURIComponent(entity.id)}/${resolution}`;
}

function show(entity: DriftedEntity, answer: (path: string) => Response) {
  const calls: string[] = [];
  const fetchMock = vi.fn((input: RequestInfo | URL) => {
    const request = input as Request;
    const path = new URL(request.url).pathname;
    calls.push(`${request.method} ${path}`);
    return Promise.resolve(answer(path));
  });
  vi.stubGlobal("fetch", fetchMock);
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <DriftResolutionModal
          project={PROJECT}
          entity={entity}
          open
          onOpenChange={() => undefined}
        />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return calls;
}

const ok = (status = 204) => new Response(null, { status });

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("resolving a drifted entity", () => {
  it("shows what differs, and where it is declared", async () => {
    show(MODIFIED, () => ok());

    expect(await screen.findByText("airQualityIndex.value")).toBeInTheDocument();
    expect(screen.getByText("42")).toBeInTheDocument();
    expect(screen.getByText("7")).toBeInTheDocument();
    // The file, so the person can read the change they are about to resolve.
    expect(screen.getByText(/stations\.json/)).toBeInTheDocument();
  });

  it("reverts through the route that writes the repository back", async () => {
    const calls = show(MODIFIED, () => ok());
    await userEvent.click(screen.getByRole("button", { name: /revert/i }));

    expect(calls).toContain(route(MODIFIED, "revert"));
  });

  it("adopts through the route that proposes the live value, and says a change was opened", async () => {
    const calls = show(MODIFIED, () =>
      new Response(JSON.stringify({ kind: "Change" }), {
        status: 202,
        headers: { "content-type": "application/json" },
      }),
    );
    await userEvent.click(screen.getByRole("button", { name: /adopt/i }));

    expect(calls).toContain(route(MODIFIED, "adopt"));
    // An adopt is a proposal, not a write: the dialog stays open and says an approver sees it.
    expect(await screen.findByRole("status")).toHaveTextContent(/approver/i);
  });

  it("offers no adopt where there is nothing live, and says why", async () => {
    show(MISSING, () => ok());

    expect(screen.getByRole("button", { name: /revert/i })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /adopt/i })).not.toBeInTheDocument();
    // Absent with its reason on the line, not a disabled button somebody hovers to find out.
    expect(screen.getByText(/explicit deletion/i)).toBeInTheDocument();
  });

  it("says what the platform refused rather than looking like it worked", async () => {
    show(MODIFIED, () =>
      new Response(
        JSON.stringify({ detail: "no space surface is configured" }),
        { status: 503, headers: { "content-type": "application/problem+json" } },
      ),
    );
    await userEvent.click(screen.getByRole("button", { name: /revert/i }));

    expect(await screen.findByRole("alert")).toHaveTextContent(/no space surface/i);
  });
});
