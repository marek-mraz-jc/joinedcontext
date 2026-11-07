/**
 * T-3103, API/01 §30 and §33: the form view of a space's type asks for the model's fields as the
 * view's settings choose them, hides a field under its condition, takes prefilled answers from the
 * address, says what is wrong before sending, and creates one entity through the space surface; a
 * public form is a public Endpoint that allows creating alone, and its page posts anonymously.
 */
// covers (T-2137, the module gate in gate_modules.test.ts): src/pages/spaces/FormView.tsx.
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import type { LinkmlSlot } from "../src/pages/models/linkml";
import { FormView, PublicFormPage, publishedType } from "../src/pages/spaces/FormView";
import { expectNoViolations } from "./checks";
import { renderPage } from "./page_contract";

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": status < 300 ? "application/json" : "application/problem+json" },
  });

const SLOTS: LinkmlSlot[] = [
  { name: "name", kind: "Property", required: true, description: "What it is called" },
  { name: "category", kind: "Property", range: "Category" },
  { name: "capacity", kind: "Property", range: "integer" },
];
const ENUMS = { category: [{ value: "street" }, { value: "garage" }] };

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the form view", () => {
  it("creates one entity through the space surface from the shown, typed answers", async () => {
    const posted: { path: string; body: Record<string, unknown>; reason: string | null }[] = [];
    renderPage(
      <FormView
        space="parking"
        type="ParkingSpot"
        slots={SLOTS}
        enums={ENUMS}
        settings={{ conditions: [{ attr: "capacity", when: { attr: "category", equals: "garage" } }] }}
        search={new URLSearchParams("name=Hlavná")}
      />,
      {
        path: "/projects/city/spaces/parking",
        answer: async (url, request) => {
          if (request.method !== "POST") return undefined;
          posted.push({
            path: url.pathname,
            body: (await request.json()) as Record<string, unknown>,
            reason: request.headers.get("jc-reason"),
          });
          return new Response(null, { status: 201 });
        },
      },
    );
    const name = await screen.findByRole("textbox", { name: /^name/ });
    expect(name).toHaveValue("Hlavná");
    // The capacity waits for its condition.
    expect(screen.queryByRole("textbox", { name: /capacity/ })).toBeNull();
    await userEvent.selectOptions(screen.getByRole("combobox", { name: /category/ }), "garage");
    await userEvent.type(screen.getByRole("textbox", { name: /capacity/ }), "40");
    await expectNoViolations(name.closest("form") as HTMLElement);
    await userEvent.click(screen.getByRole("button", { name: en.spaces.form.submit }));

    await waitFor(() => expect(posted).toHaveLength(1));
    expect(posted[0].path).toBe("/cs/parking/ngsi-ld/v1/entities");
    // DM-113: the form's reason, as it stood, travels percent-encoded as JC-Reason.
    expect(posted[0].reason).toBe(encodeURIComponent(en.spaces.form.reasonDefault));
    const { id, ...rest } = posted[0].body;
    expect(String(id)).toMatch(/^urn:ngsi-ld:ParkingSpot:[0-9a-f-]{36}$/);
    expect(rest).toEqual({
      type: "ParkingSpot",
      name: { type: "Property", value: "Hlavná" },
      category: { type: "Property", value: "garage" },
      capacity: { type: "Property", value: 40 },
    });
    expect(await screen.findByText(new RegExp(String(id)))).toBeInTheDocument();
  });

  it("says what is wrong before sending, and the gateway's refusal after", async () => {
    let posts = 0;
    renderPage(<FormView space="parking" type="ParkingSpot" slots={SLOTS} enums={ENUMS} search={new URLSearchParams()} />, {
      path: "/projects/city/spaces/parking",
      answer: async (_url, request) => {
        if (request.method !== "POST") return undefined;
        posts += 1;
        return json({ title: "Forbidden", status: 403, detail: "the policy does not grant createEntity" }, 403);
      },
    });
    await userEvent.type(await screen.findByRole("textbox", { name: /capacity/ }), "many");
    await userEvent.click(screen.getByRole("button", { name: en.spaces.form.submit }));
    expect(screen.getByRole("textbox", { name: /^name/ })).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByText(en.spaces.form.problem.integer)).toBeInTheDocument();
    expect(screen.getByText(/2 answers need fixing/)).toBeInTheDocument();
    expect(posts).toBe(0);

    await userEvent.type(screen.getByRole("textbox", { name: /^name/ }), "Hlavná");
    await userEvent.clear(screen.getByRole("textbox", { name: /capacity/ }));
    await userEvent.click(screen.getByRole("button", { name: en.spaces.form.submit }));
    expect(await screen.findByText(/the policy does not grant createEntity/)).toBeInTheDocument();
    expect(screen.getByText(/Ask a project owner for a Policy that lets you create ParkingSpot/)).toBeInTheDocument();
    expect(posts).toBe(1);
  });

  it("asks why the person adds it: an empty reason stops the send, a typed one is sent", async () => {
    const reasons: (string | null)[] = [];
    renderPage(<FormView space="parking" type="ParkingSpot" slots={SLOTS} enums={ENUMS} search={new URLSearchParams("name=Hlavná")} />, {
      path: "/projects/city/spaces/parking",
      answer: async (_url, request) => {
        if (request.method !== "POST") return undefined;
        reasons.push(request.headers.get("jc-reason"));
        return new Response(null, { status: 201 });
      },
    });
    const reason = await screen.findByRole("textbox", { name: new RegExp(en.spaces.form.reason) });
    expect(reason).toHaveValue(en.spaces.form.reasonDefault);
    expect(reason).toHaveAccessibleDescription(new RegExp(en.spaces.form.hint.reason.slice(0, 20)));
    await userEvent.clear(reason);
    await userEvent.click(screen.getByRole("button", { name: en.spaces.form.submit }));
    expect(screen.getByText(en.spaces.form.reasonError.required)).toBeInTheDocument();
    expect(reason).toHaveAttribute("aria-invalid", "true");
    expect(reasons).toHaveLength(0);

    await userEvent.type(reason, "Kontrola 22. 10.");
    await userEvent.click(screen.getByRole("button", { name: en.spaces.form.submit }));
    await waitFor(() => expect(reasons).toEqual([encodeURIComponent("Kontrola 22. 10.")]));
  });
});

describe("a public form", () => {
  it("builds its fields from the published schema and posts anonymously through the Endpoint", async () => {
    const requests: Request[] = [];
    renderPage(<PublicFormPage slug="abc" search={new URLSearchParams("capacity=12")} />, {
      path: "/f/abc",
      answer: async (url, request) => {
        if (!url.pathname.startsWith("/api/endpoint/abc/")) return undefined;
        requests.push(request);
        if (url.pathname.endsWith("/schema/index.json")) return json({ models: [{ version: 2 }] });
        if (url.pathname.endsWith("/schema/v2/json-schema")) {
          return json({
            definitions: {
              ParkingSpot: {
                required: ["name", "id", "type"],
                properties: {
                  id: { type: "string" },
                  type: { type: "string" },
                  name: { type: ["string", "null"], description: "What it is called" },
                  capacity: { type: ["integer", "null"] },
                },
              },
            },
          });
        }
        if (request.method === "POST") {
          return new Response(null, { status: 201, headers: { Location: "/ngsi-ld/v1/entities/urn:ngsi-ld:ParkingSpot:minted-7" } });
        }
        return undefined;
      },
    });
    expect(await screen.findByRole("heading", { level: 1, name: "ParkingSpot" })).toBeInTheDocument();
    expect(screen.getByText(en.spaces.form.publicData)).toBeInTheDocument();
    expect(screen.getByRole("textbox", { name: /capacity/ })).toHaveValue("12");
    await userEvent.type(screen.getByRole("textbox", { name: /^name/ }), "Hlavná");
    await expectNoViolations(screen.getByTestId("public-form"));
    await userEvent.click(screen.getByRole("button", { name: en.spaces.form.submit }));
    await waitFor(() => expect(requests.some((r) => r.method === "POST")).toBe(true));
    const post = requests.find((r) => r.method === "POST") as Request;
    expect(new URL(post.url).pathname).toBe("/api/endpoint/abc/ngsi-ld/v1/entities");
    expect(post.credentials).toBe("omit");
    const sentEntity = (await post.clone().json()) as Record<string, unknown>;
    expect(sentEntity).not.toHaveProperty("website");
    // The id the gateway minted is the one shown, not the one the page proposed (EP-97).
    expect(await screen.findByText("Created urn:ngsi-ld:ParkingSpot:minted-7.")).toBeInTheDocument();
  });

  it("sends a filled trap field, which the form's Policy does not grant", async () => {
    const posts: Request[] = [];
    renderPage(<PublicFormPage slug="abc" search={new URLSearchParams()} />, {
      path: "/f/abc",
      answer: async (url, request) => {
        if (!url.pathname.startsWith("/api/endpoint/abc/")) return undefined;
        if (url.pathname.endsWith("/schema/index.json")) return json({ models: [{ version: 1 }] });
        if (url.pathname.endsWith("/schema/v1/json-schema")) {
          return json({ definitions: { ParkingSpot: { properties: { id: { type: "string" }, type: { type: "string" }, name: { type: ["string", "null"] } } } } });
        }
        if (request.method === "POST") {
          posts.push(request);
          return json({ title: "Forbidden" }, 403);
        }
        return undefined;
      },
    });
    await screen.findByRole("heading", { level: 1, name: "ParkingSpot" });
    // A person never reaches it: it is outside the accessibility tree and the tab order.
    expect(screen.queryByRole("textbox", { name: en.spaces.form.trap })).toBeNull();
    const trap = document.getElementById("public-form-website") as HTMLInputElement;
    expect(trap.tabIndex).toBe(-1);
    await userEvent.type(trap, "https://spam.example");
    await userEvent.click(screen.getByRole("button", { name: en.spaces.form.submit }));
    await waitFor(() => expect(posts).toHaveLength(1));
    expect(((await posts[0].clone().json()) as Record<string, unknown>).website).toEqual({ type: "Property", value: "https://spam.example" });
    expect(await screen.findByText(/Not created/)).toBeInTheDocument();
  });

  /** The schema a form of one required field publishes, answered for slug `abc`. */
  const parkingSchema = async (url: URL) => {
    if (url.pathname.endsWith("/schema/index.json")) return json({ models: [{ version: 1 }] });
    if (url.pathname.endsWith("/schema/v1/json-schema")) {
      return json({ definitions: { ParkingSpot: { required: ["name"], properties: { id: { type: "string" }, type: { type: "string" }, name: { type: ["string", "null"] } } } } });
    }
    return undefined;
  };

  it("in a test run checks the entry and asks the gateway, and writes nothing (EP-102)", async () => {
    const requests: Request[] = [];
    let decision = true;
    renderPage(<PublicFormPage slug="abc" search={new URLSearchParams("test=1")} />, {
      path: "/f/abc",
      answer: async (url, request) => {
        if (!url.pathname.startsWith("/api/endpoint/abc/")) return undefined;
        requests.push(request);
        if (url.pathname.endsWith("/access/check")) return json(decision ? { decision: true, context: { reason: "policy_grant_matched" } } : { decision: false });
        return parkingSchema(url);
      },
    });
    await screen.findByRole("heading", { level: 1, name: "ParkingSpot" });
    expect(screen.getByText(en.spaces.form.testMode)).toBeInTheDocument();
    const send = screen.getByRole("button", { name: en.spaces.form.testSubmit });
    // The schema's required field is checked before anything is asked.
    await userEvent.click(send);
    expect(await screen.findByText("1 answer needs fixing before the form can be sent.")).toBeInTheDocument();
    expect(requests.filter((r) => r.method === "POST")).toHaveLength(0);
    await userEvent.type(screen.getByRole("textbox", { name: /^name/ }), "Hlavná");
    await userEvent.click(send);
    expect(await screen.findByText(en.spaces.form.testPassed)).toBeInTheDocument();
    decision = false;
    await userEvent.type(screen.getByRole("textbox", { name: /^name/ }), "Druhá");
    await userEvent.click(send);
    expect(await screen.findByText(/would not be accepted: this form does not take entries/)).toBeInTheDocument();
    const posts = requests.filter((r) => r.method === "POST");
    expect(posts.map((r) => new URL(r.url).pathname)).toEqual(["/api/endpoint/abc/access/check", "/api/endpoint/abc/access/check"]);
    expect(await posts[0].clone().json()).toEqual({ action: { name: "createEntity" }, resource: { type: "ParkingSpot" } });
    // Anonymous on every request, the reads included.
    expect(requests.every((r) => r.credentials === "omit")).toBe(true);
  });

  it("tells the page that frames it how tall it is, and nothing else", async () => {
    const posted: unknown[] = [];
    const parent = { postMessage: (message: unknown, target: string) => posted.push({ message, target }) };
    vi.stubGlobal("ResizeObserver", class {
      constructor(private readonly callback: () => void) {}
      observe() {
        this.callback();
      }
      disconnect() {}
    });
    const real = Object.getOwnPropertyDescriptor(window, "parent");
    Object.defineProperty(window, "parent", { value: parent, configurable: true });
    try {
      renderPage(<PublicFormPage slug="abc" search={new URLSearchParams()} />, {
        path: "/f/abc",
        answer: async (url) => (url.pathname.startsWith("/api/endpoint/abc/") ? parkingSchema(url) : undefined),
      });
      await waitFor(() => expect(posted.length).toBeGreaterThan(0));
      for (const one of posted) {
        expect(one).toMatchObject({ message: { type: "jc-form-height", height: expect.any(Number) }, target: "*" });
      }
    } finally {
      if (real) Object.defineProperty(window, "parent", real);
    }
  });

  it("takes the one entity definition as its type, never a nested object's, and guesses none of two", () => {
    const address = { properties: { streetAddress: { type: "string" } } };
    const spot = { properties: { id: { type: "string" }, type: { type: "string" }, name: { type: "string" } } };
    expect(publishedType({ Address: address, ParkingSpot: spot })).toBe("ParkingSpot");
    expect(publishedType({ Address: address })).toBeUndefined();
    expect(publishedType({ ParkingSpot: spot, Zone: spot })).toBeUndefined();
  });

  it("says the form is not published when the Endpoint is gone", async () => {
    renderPage(<PublicFormPage slug="gone" search={new URLSearchParams()} />, {
      path: "/f/gone",
      answer: async (url) => (url.pathname.startsWith("/api/endpoint/gone/") ? json({ title: "Not Found" }, 404) : undefined),
    });
    expect(await screen.findByText(en.spaces.form.notPublished)).toBeInTheDocument();
  });
});
