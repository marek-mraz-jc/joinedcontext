import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { DriftResolutionModal } from "../src/components/drift/DriftResolutionModal";
import type { DriftedEntity } from "../src/components/drift/DriftResolutionModal";

const ENTITY: DriftedEntity = {
  space: "helsinki",
  id: "urn:ngsi-ld:AirQualityObserved:hel.fi:helsinki:station-1",
  drift: "MODIFIED",
  diff: [{ path: "airQualityIndex.value", declared: 42, live: 7 }],
  resolutions: ["revert", "adopt"],
  source: "projects/helsinki/spaces/helsinki/entities/seed/stations.json",
};

function show(answer: () => Promise<Response>, onOpenChange: (open: boolean) => void = () => undefined) {
  const calls: string[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((input: RequestInfo | URL) => {
      const request = input as Request;
      calls.push(`${request.method} ${new URL(request.url).pathname}`);
      return answer();
    }),
  );
  render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <DriftResolutionModal project="helsinki" entity={ENTITY} open onOpenChange={onOpenChange} />
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return calls;
}

describe("closing the drift dialog", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it("closes on Cancel without asking the server anything", async () => {
    const onOpenChange = vi.fn();
    const calls = show(() => Promise.resolve(new Response(null, { status: 204 })), onOpenChange);
    await userEvent.click(screen.getByRole("button", { name: en.form.cancel }));
    expect(onOpenChange).toHaveBeenCalledWith(false);
    expect(calls).toHaveLength(0);
  });

  it("replaces the buttons with Close once an adopt is proposed, and closes from it", async () => {
    const onOpenChange = vi.fn();
    const calls = show(
      () => Promise.resolve(new Response(JSON.stringify({ kind: "Change" }), { status: 202, headers: { "content-type": "application/json" } })),
      onOpenChange,
    );
    await userEvent.click(screen.getByRole("button", { name: en.drift.modal.adopt }));
    expect(calls).toHaveLength(1);
    expect(calls[0]).toMatch(/^POST .*\/drift\/helsinki\/.*\/adopt$/);
    expect(screen.queryByRole("button", { name: en.drift.modal.revert })).not.toBeInTheDocument();
    const close = screen.getAllByRole("button", { name: en.drift.modal.close });
    await userEvent.click(close[close.length - 1]);
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("keeps the dialog open and says why when the revert request never reaches the Portal", async () => {
    const onOpenChange = vi.fn();
    show(() => Promise.reject(new TypeError("Failed to fetch")), onOpenChange);
    await userEvent.click(screen.getByRole("button", { name: en.drift.modal.revert }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Failed to fetch");
    expect(onOpenChange).not.toHaveBeenCalled();
  });
});
