/**
 * The form that writes (T-0594, AP-61…AP-63): inputs from the endpoint's schema, a save as one
 * PATCH or POST through the endpoint, a refusal kept on the form with its reason, and in a
 * preview the write travelling to the host page and its answer coming back.
 */
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("maplibre-gl", () => ({ Map: class {} }));
vi.mock("maplibre-gl/dist/maplibre-gl.css", () => ({}));

import { App } from "../src/App";
import { parseSpec } from "../src/spec";
import { attrsOf, fieldOf, requestOf } from "../src/write";
import type { Schema } from "../src/write";

const spec = parseSpec({
  title: "Stations",
  sources: [{ name: "stations", type: "BikeHireDockingStation", attrs: ["name", "availableBikeNumber", "status"] }],
  views: [
    { kind: "table", columns: ["name", "status"] },
    { kind: "form", title: "Station", fields: ["availableBikeNumber", "status"] },
  ],
}).spec!;

const SCHEMA: Schema = {
  BikeHireDockingStation: {
    properties: {
      name: { type: "string" },
      availableBikeNumber: { type: "integer", minimum: 0, maximum: 500 },
      status: { enum: ["working", "closed"] },
    },
    required: ["name"],
  },
};

const ID = "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:002";
const STATIONS = [
  { id: "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:001", type: "BikeHireDockingStation", name: "Kaivopuisto", availableBikeNumber: 12, status: "working" },
  { id: ID, type: "BikeHireDockingStation", name: "Laivasillankatu", availableBikeNumber: 3, status: "working" },
];

type Call = { url: string; init: RequestInit };
const calls: Call[] = [];
function stubFetch(status = 204, body: unknown = null) {
  vi.stubGlobal("fetch", vi.fn(async (url: string, init: RequestInit) => {
    calls.push({ url, init });
    return new Response(body === null ? null : JSON.stringify(body), { status, headers: { "content-type": "application/json" } });
  }));
}

beforeEach(() => {
  calls.length = 0;
  // jsdom draws no dialog; the attribute is what makes its content visible to role queries.
  HTMLDialogElement.prototype.showModal = function () {
    this.setAttribute("open", "");
  };
  HTMLDialogElement.prototype.close = function () {
    this.removeAttribute("open");
  };
  document.cookie = "__Host-jc_csrf=token-123; Secure; path=/";
});
afterEach(() => {
  vi.unstubAllGlobals();
});

async function openForm(bridge = false) {
  render(<App slug="demo" spec={spec} inline={{ stations: STATIONS }} schema={SCHEMA} bridge={bridge} />);
  await waitFor(() => expect(screen.getByText("2 entities")).toBeInTheDocument());
  fireEvent.click(screen.getByText("Laivasillankatu"));
}

describe("the form's inputs", () => {
  it("come from the schema: a select over the enum, a number within its bounds, required marks", async () => {
    stubFetch();
    await openForm();
    const status = screen.getByLabelText("status") as HTMLSelectElement;
    expect(status.tagName).toBe("SELECT");
    expect([...status.options].map((o) => o.value)).toEqual(["", "working", "closed"]);
    expect(status.value).toBe("working");
    const bikes = screen.getByLabelText("availableBikeNumber") as HTMLInputElement;
    expect(bikes.type).toBe("number");
    expect(bikes.min).toBe("0");
    expect(bikes.max).toBe("500");
    expect(fieldOf("name", SCHEMA.BikeHireDockingStation, "text")).toEqual({ name: "name", input: "text", pattern: undefined, required: true });
    expect(fieldOf("ghost", SCHEMA.BikeHireDockingStation, "number").input).toBe("number");
    expect(fieldOf("when", { properties: { when: { type: "string", format: "date-time" } } }, "text").input).toBe("date");
    // DM-06, T-2812: a quantity carries the code its model measures it in, and nothing else does.
    const pm10 = { type: "number", "x-unit": { exactMappings: ["qudt-unit:MicroGM-PER-M3", "ucefact:GQ"] } };
    expect(fieldOf("pm10", { properties: { pm10 } }, "number").unit).toBe("GQ");
    expect(fieldOf("pm10", { properties: { pm10: { type: "number", "x-unit": { exactMappings: ["unece:CEL"] } } } }, "number").unit).toBe("CEL");
    expect(fieldOf("count", { properties: { count: { type: "integer" } } }, "number")).not.toHaveProperty("unit");
    // SDK-07: a LanguageProperty is not a text box, which would write one language over them all.
    expect(fieldOf("name", { properties: { name: { type: ["object", "null"], "x-ngsi-ld-kind": "LanguageProperty" } } }, "text").input).toBe("language");
  });
});

describe("a save", () => {
  it("is one PATCH of the changed attributes through the endpoint, and the screen follows", async () => {
    stubFetch();
    await openForm();
    fireEvent.change(screen.getByLabelText("status"), { target: { value: "closed" } });
    fireEvent.change(screen.getByLabelText("availableBikeNumber"), { target: { value: "5" } });
    fireEvent.click(screen.getByText("Save"));
    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0].url).toBe(`/api/endpoint/demo/ngsi-ld/v1/entities/${encodeURIComponent(ID)}/attrs`);
    expect(calls[0].init.method).toBe("PATCH");
    expect(calls[0].init.credentials).toBe("same-origin");
    expect((calls[0].init.headers as Record<string, string>)["x-csrf-token"]).toBe("token-123");
    expect(JSON.parse(calls[0].init.body as string)).toEqual({
      availableBikeNumber: { type: "Property", value: 5 },
      status: { type: "Property", value: "closed" },
    });
    await waitFor(() => expect(screen.getByText(`Saved ${ID}`)).toBeInTheDocument());
    expect(screen.getAllByRole("cell").map((c) => c.textContent)).toContain("closed");
  });

  it("also leaves on Enter in an input, and never with an input the schema refuses", async () => {
    stubFetch();
    await openForm();
    fireEvent.change(screen.getByLabelText("availableBikeNumber"), { target: { value: "-1" } });
    fireEvent.click(screen.getByText("Save"));
    await new Promise((r) => setTimeout(r, 20));
    expect(calls).toHaveLength(0);
    fireEvent.change(screen.getByLabelText("availableBikeNumber"), { target: { value: "5" } });
    fireEvent.keyDown(screen.getByLabelText("availableBikeNumber"), { key: "Enter" });
    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0].init.method).toBe("PATCH");
  });

  it("the endpoint refuses stays on the form with the reason, and nothing reloads", async () => {
    stubFetch(403, { type: "about:blank", title: "Forbidden", status: 403, detail: "no policy grants a write of status here" });
    await openForm();
    fireEvent.change(screen.getByLabelText("status"), { target: { value: "closed" } });
    fireEvent.click(screen.getByText("Save"));
    expect(await screen.findByRole("alert")).toHaveTextContent("no policy grants a write of status here");
    expect(screen.getByLabelText("status")).toBeInTheDocument();
    expect((screen.getByLabelText("status") as HTMLSelectElement).value).toBe("closed");
    expect(screen.queryByText(/^Saved/)).not.toBeInTheDocument();
    expect(screen.getAllByRole("cell").map((c) => c.textContent)).not.toContain("closed");
  });

  it("of a new entity is a POST with the id minted under the rows' prefix", async () => {
    stubFetch(201);
    await openForm();
    fireEvent.click(screen.getByText("Close"));
    fireEvent.click(screen.getByRole("button", { name: "New BikeHireDockingStation" }));
    fireEvent.change(screen.getByLabelText("id"), { target: { value: "004" } });
    fireEvent.change(screen.getByLabelText("status"), { target: { value: "working" } });
    fireEvent.change(screen.getByLabelText("availableBikeNumber"), { target: { value: "1" } });
    fireEvent.click(screen.getByText("Save"));
    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0].url).toBe("/api/endpoint/demo/ngsi-ld/v1/entities");
    expect(calls[0].init.method).toBe("POST");
    expect(JSON.parse(calls[0].init.body as string)).toEqual({
      type: "BikeHireDockingStation",
      id: "urn:ngsi-ld:BikeHireDockingStation:hel.fi:helsinki:004",
      availableBikeNumber: { type: "Property", value: 1 },
      status: { type: "Property", value: "working" },
    });
    await waitFor(() => expect(screen.getByText("3 entities")).toBeInTheDocument());
  });

  it("in a preview leaves as a message to the host page and takes its answer", async () => {
    stubFetch();
    const seen: { kind: string; id: number; method: string; path: string }[] = [];
    const host = (event: MessageEvent) => {
      const data = event.data as (typeof seen)[number];
      if (data?.kind !== "jc-request") return;
      seen.push(data);
      // The answer comes from the framing page, which in jsdom is the window itself.
      const answer = { kind: "jc-response", id: data.id, status: 403, body: { detail: "the sandbox policy says no" } };
      window.dispatchEvent(new MessageEvent("message", { data: answer, source: window }));
    };
    window.addEventListener("message", host);
    try {
      await openForm(true);
      fireEvent.change(screen.getByLabelText("status"), { target: { value: "closed" } });
      fireEvent.click(screen.getByText("Save"));
      expect(await screen.findByRole("alert")).toHaveTextContent("the sandbox policy says no");
      expect(calls).toHaveLength(0);
      expect(seen).toHaveLength(1);
      expect(seen[0].method).toBe("PATCH");
      expect(seen[0].path).toBe(`/api/endpoint/demo/ngsi-ld/v1/entities/${encodeURIComponent(ID)}/attrs`);
    } finally {
      window.removeEventListener("message", host);
    }
  });
});

describe("helpers", () => {
  it("shape a patch as Properties and a write as its request", () => {
    expect(attrsOf({ a: 1, b: null })).toEqual({ a: { type: "Property", value: 1 }, b: { type: "Property", value: null } });
    expect(requestOf("s", { id: "urn:x:1", type: "T", patch: { a: 1 } })).toEqual({ method: "PATCH", path: "/api/endpoint/s/ngsi-ld/v1/entities/urn%3Ax%3A1/attrs", body: { a: { type: "Property", value: 1 } } });
    expect(requestOf("s", { type: "T", entity: { id: "urn:x:2" } })).toEqual({ method: "POST", path: "/api/endpoint/s/ngsi-ld/v1/entities", body: { type: "T", id: "urn:x:2" } });
  });
});
