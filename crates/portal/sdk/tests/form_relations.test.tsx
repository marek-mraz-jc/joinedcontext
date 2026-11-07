/**
 * A relationship end in a form is picked and written as a Relationship (T-2861; DM-64, UI-84).
 *
 * Before: `fieldOf` gave a relationship a text box and `attrsOf`/`encodeAttrs` wrote every field
 * as a Property, so `school` reached the broker as `{type: "Property", value: "urn:…"}`, a value
 * naming no target, which the gateway refuses on a required end.
 */
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("maplibre-gl", () => ({ Map: class {} }));
vi.mock("maplibre-gl/dist/maplibre-gl.css", () => ({}));

import { App } from "../src/App";
import { parseSpec } from "../src/spec";
import { createClient } from "../src/sdk/client";
import type { JcConfig } from "../src/sdk/config";
import type { JcRequest } from "../src/sdk/transport";
import { attrsOf, fieldOf, requestOf } from "../src/write";
import type { Schema } from "../src/write";

const SCHEMA: Schema = {
  User: {
    properties: {
      name: { type: "string" },
      school: { type: "string", "x-ngsi-ld-relationship": { target: "School" } },
      courses: { type: "array", "x-ngsi-ld-relationship": { target: "Course" } },
      mentor: { type: "string", "x-ngsi-ld-relationship": { target: "User" } },
    },
    required: ["school"],
  },
};

const PREFIX = "urn:ngsi-ld:User:city.example:learning:";
const SCHOOL_A = "urn:ngsi-ld:School:city.example:learning:a";
const SCHOOL_B = "urn:ngsi-ld:School:city.example:learning:b";
// Normalized, as the endpoint answers: the picker reads a target's `name` Property.
const USERS = [
  { id: `${PREFIX}ana`, type: "User", name: { type: "Property", value: "Ana" }, school: { type: "Relationship", object: SCHOOL_A } },
];
const SCHOOLS = [
  { id: SCHOOL_A, type: "School", name: { type: "Property", value: "North" } },
  { id: SCHOOL_B, type: "School", name: { type: "Property", value: "South" } },
];

const spec = parseSpec({
  title: "Users",
  sources: [{ name: "users", type: "User", attrs: ["name", "school", "mentor"] }],
  views: [
    { kind: "table", columns: ["name"] },
    { kind: "form", title: "User", fields: ["name", "school", "mentor"] },
  ],
}).spec!;

type Call = { url: string; init: RequestInit };
const calls: Call[] = [];

beforeEach(() => {
  calls.length = 0;
  vi.stubGlobal(
    "fetch",
    vi.fn(async (url: string, init: RequestInit) => {
      calls.push({ url, init });
      return { ok: true, status: 204, json: async () => null };
    }),
  );
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

describe("a relationship field", () => {
  it("is a picker of its target class, many when the end is a list, required as the model says", () => {
    expect(fieldOf("school", SCHEMA.User, "text")).toEqual({ name: "school", input: "relation", target: "School", many: false, required: true });
    expect(fieldOf("courses", SCHEMA.User, "text")).toEqual({ name: "courses", input: "relation", target: "Course", many: true, required: false });
    // A property with no target stays what it was.
    expect(fieldOf("name", SCHEMA.User, "text").input).toBe("text");
  });

  it("is written as a Relationship by the form's patch, its request and the SDK client", async () => {
    expect(attrsOf({ school: { object: SCHOOL_A }, courses: { object: ["urn:c:1", "urn:c:2"] }, name: "Ana" })).toEqual({
      school: { type: "Relationship", object: SCHOOL_A },
      courses: { type: "Relationship", object: ["urn:c:1", "urn:c:2"] },
      name: { type: "Property", value: "Ana" },
    });
    expect(requestOf("s", { id: `${PREFIX}ana`, type: "User", patch: { school: { object: SCHOOL_B } } }).body).toEqual({
      school: { type: "Relationship", object: SCHOOL_B },
    });

    const sent: JcRequest[] = [];
    const config: JcConfig = { slug: "demo", orgDomain: "city.example", space: "learning", transport: "bridge", language: "en" };
    const client = createClient(config, async (req) => {
      sent.push(req);
      return { status: 204, body: null };
    });
    await client.entities.update(`${PREFIX}ana`, { school: { object: SCHOOL_B }, name: "Ana" });
    expect(sent[0].body).toEqual({ school: { type: "Relationship", object: SCHOOL_B }, name: { type: "Property", value: "Ana" } });
  });
});

async function openAna() {
  render(<App slug="demo" spec={spec} inline={{ users: USERS, schools: SCHOOLS }} schema={SCHEMA} />);
  await waitFor(() => expect(screen.getByText("1 entities")).toBeInTheDocument());
  fireEvent.click(screen.getByText("Ana"));
}

describe("the form's relationship picker", () => {
  it("replaces a single end's target with a pick and saves it as a Relationship", async () => {
    await openAna();
    const school = screen.getByRole("group", { name: "school" });
    // The last target of a required end cannot be removed: the model refuses that write.
    expect(within(school).queryByRole("button", { name: /Remove/ })).toBeNull();
    const box = within(school).getByRole("combobox", { name: "Search School" });
    fireEvent.focus(box);
    fireEvent.change(box, { target: { value: "b" } });
    const option = await within(school).findByRole("option", { name: /South/ });
    fireEvent.click(option);
    fireEvent.click(screen.getByText("Save"));
    await waitFor(() => expect(calls).toHaveLength(1));
    expect(calls[0].init.method).toBe("PATCH");
    expect(JSON.parse(calls[0].init.body as string)).toEqual({ school: { type: "Relationship", object: SCHOOL_B } });
  });

  it("offers only entities of the target class", async () => {
    await openAna();
    const mentor = screen.getByRole("group", { name: "mentor" });
    const box = within(mentor).getByRole("combobox", { name: "Search User" });
    fireEvent.focus(box);
    const option = await within(mentor).findByRole("option", { name: /Ana/ });
    expect(option).toBeInTheDocument();
    expect(within(mentor).queryByRole("option", { name: /North|South/ })).toBeNull();
  });

  it("refuses a new entity without its required end before any write", async () => {
    await openAna();
    fireEvent.click(screen.getByText("Close"));
    fireEvent.click(screen.getByRole("button", { name: "New User" }));
    fireEvent.change(screen.getByLabelText("id"), { target: { value: "ben" } });
    fireEvent.click(screen.getByText("Save"));
    expect(await screen.findByRole("alert")).toHaveTextContent("school needs a School.");
    expect(calls).toHaveLength(0);
  });
});
