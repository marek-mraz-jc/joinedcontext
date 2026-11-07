/** T-0741, AG-78: the assistant prepares a change to entities; only the person's Apply sends it. */
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { EntityWriteCard, entityWriteOf } from "../src/pages/apps/EntityWriteCard";
import { expectNoAxeViolations } from "./page_contract";

const STATION = "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:kaivopuisto";
const STEP = {
  tool: "write_entities",
  status: "ok",
  output: {
    endpoint: "helsinki-bikes-ops",
    slug: "k7m2qz4tv6xh3n5jb2ryd3wcfa",
    entities: [{ id: STATION, type: "BikeHireDockingStation", changes: [{ attribute: "status", before: "working", after: "outOfService" }] }],
  },
};

function card(live = true) {
  const write = entityWriteOf(STEP);
  if (write === null) {
    throw new Error("the step is a preview");
  }
  render(
    <I18nextProvider i18n={i18n}>
      <EntityWriteCard write={write} live={live} />
    </I18nextProvider>,
  );
}

describe("the change to entities card", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    document.cookie = "__Host-jc_csrf=csrf-token-value; Secure; path=/";
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("shows every value before and after, by the entity's name, and sends nothing until Apply", async () => {
    const fetchMock = vi.fn(async () => new Response(null, { status: 204 }));
    vi.stubGlobal("fetch", fetchMock);
    card();

    expect(screen.getByText("kaivopuisto")).toHaveAttribute("title", STATION);
    expect(screen.queryByText(STATION)).not.toBeInTheDocument();
    expect(screen.getByText("working")).toBeInTheDocument();
    expect(screen.getByText("outOfService")).toBeInTheDocument();
    expect(fetchMock).not.toHaveBeenCalled();

    await userEvent.click(screen.getByRole("button", { name: en.agentRun.write.apply }));
    expect(await screen.findByText("Changed 1 entity.")).toBeInTheDocument();
    expect(fetchMock).toHaveBeenCalledTimes(1);
    const [url, init] = fetchMock.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe(`/api/endpoint/k7m2qz4tv6xh3n5jb2ryd3wcfa/ngsi-ld/v1/entities/${encodeURIComponent(STATION)}/attrs`);
    expect(init.method).toBe("PATCH");
    expect(init.credentials).toBe("same-origin");
    expect((init.headers as Record<string, string>)["x-csrf-token"]).toBe("csrf-token-value");
    // DM-113: the applied change names why it was written.
    expect((init.headers as Record<string, string>)["JC-Reason"]).toBe(encodeURIComponent(en.agentRun.write.reason));
    expect((init.headers as Record<string, string>)["JC-Record"]).toBe("version");
    expect(JSON.parse(init.body as string)).toEqual({ status: { type: "Property", value: "outOfService" } });
    expect(screen.queryByRole("button", { name: en.agentRun.write.apply })).not.toBeInTheDocument();
  });

  it("sends nothing on Cancel", async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
    card();

    await userEvent.click(screen.getByRole("button", { name: en.agentRun.write.cancel }));
    expect(screen.getByText(en.agentRun.write.cancelled)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: en.agentRun.write.apply })).not.toBeInTheDocument();
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("shows the gateway's reason when the endpoint refuses the person's write", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () =>
        new Response(JSON.stringify({ title: "Forbidden", status: 403, detail: "No grant covers updateAttrs on BikeHireDockingStation" }), {
          status: 403,
          headers: { "content-type": "application/problem+json" },
        }),
      ),
    );
    card();

    await userEvent.click(screen.getByRole("button", { name: en.agentRun.write.apply }));
    await waitFor(() =>
      expect(screen.getByText("kaivopuisto was not changed: No grant covers updateAttrs on BikeHireDockingStation")).toBeInTheDocument(),
    );
    expect(screen.queryByText(/Changed/)).not.toBeInTheDocument();
  });

  it("offers nothing to apply on a conversation that is over, and draws no card for a refused step", () => {
    card(false);
    expect(screen.queryByRole("button", { name: en.agentRun.write.apply })).not.toBeInTheDocument();
    expect(entityWriteOf({ ...STEP, status: "failed" })).toBeNull();
    expect(entityWriteOf({ ...STEP, tool: "change_resource" })).toBeNull();
  });

  // T-3986: the app builder draws this card only in a model run, which the live walk cannot start (spend guard).
  it("has_no_serious_axe_violation_as_a_model_run_draws_it", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response(null, { status: 204 })));
    card();
    await expectNoAxeViolations(document.body);
  });
});
