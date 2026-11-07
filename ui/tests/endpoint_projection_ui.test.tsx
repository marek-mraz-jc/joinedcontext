import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { expectOpen } from "./checks";
import en from "../src/locales/en.json";
import { App } from "../src/App";
import { publishedTypes } from "../src/pages/endpoints/SchemaProjectionPanel";
import { greenVerdict, isCheck } from "./verdict";
import { findFormPage } from "./formPage";

/**
 * T-0301: the attributes an Endpoint holds back, and the formalisms it publishes
 * (EP-07, EP-46, EP-49, EP-61, R9).
 */

const IDENTITY = {
  subject: "b7c1e0f4",
  username: "jana.kovacova",
  name: "Jana Kováčová",
  email: "jana.kovacova@banskabystrica.sk",
  roles: ["portal-editor"],
};

const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";

const ENDPOINTS = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "List",
  items: [
    {
      apiVersion: "joinedcontext.com/v1alpha1",
      kind: "Endpoint",
      metadata: {
        name: "public-air",
        namespace: "banskabystrica",
        labels: { "joinedcontext.com/space": "ovzdusie" },
      },
      spec: {
        contextSpaceRef: "ovzdusie",
        slug: SLUG,
        audience: "public",
        enabledRepresentations: ["ngsi-ld"],
        rateLimits: { requestsPerMinute: 600 },
        projection: { hiddenAttributes: ["calibrationOffset"] },
      },
      status: { phase: "Live" },
    },
  ],
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

const SCHEMA_INDEX = {
  endpoint: SLUG,
  models: [{ name: "air-quality", version: 2, semver: "2.1.0", types: ["AirQualityObserved"] }],
};

const JSON_SCHEMA = {
  $schema: "http://json-schema.org/draft-07/schema#",
  title: "air-quality 2.1.0",
  $defs: {
    AirQualityObserved: {
      properties: {
        pm10: { type: "number" },
        pm25: { type: "number" },
        calibrationOffset: { type: "number" },
        dateObserved: { type: "string" },
      },
    },
  },
};

const CHANGE = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Change",
  metadata: { name: "chg-77aa11bb", namespace: "banskabystrica" },
  status: { lane: "yellow", phase: "PendingApproval", plan: { update: 1 } },
};

const LINKML = "id: https://banskabystrica.sk/ovzdusie\nname: air-quality\n";

function renderEndpoints({ compiled = true } = {}) {
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

    if (url.pathname.endsWith("/schema/index.json")) {
      return json(SCHEMA_INDEX);
    }
    if (url.pathname.endsWith("/schema/v2/json-schema")) {
      return json(JSON_SCHEMA);
    }
    if (url.pathname.endsWith("/schema/v2/model.linkml.yaml")) {
      return compiled
        ? Promise.resolve(new Response(LINKML, { status: 200 }))
        : Promise.resolve(new Response("", { status: 406 }));
    }
    if (url.pathname.endsWith("/auth/me")) {
      return json(IDENTITY);
    }
    // The check is a dry run: it answers a verdict and writes nothing (AG-62, PF-57).
    if (request instanceof Request && isCheck(request, url)) {
      return greenVerdict(request, { valid: true, lane: "yellow" }).then((body) => json(body));
    }
    if (request instanceof Request && request.method !== "GET") {
      return json(CHANGE, 202);
    }
    if (url.pathname.endsWith("/spaces")) {
      return json(SPACES);
    }
    if (url.pathname.endsWith("/endpoints")) {
      return json(ENDPOINTS);
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
  return fetchMock;
}

function writes(fetchMock: ReturnType<typeof vi.fn>): Request[] {
  return fetchMock.mock.calls
    .map((call) => call[0] as Request)
    .filter((request) => request instanceof Request && request.method !== "GET" && !String((request as Request).url ?? request).includes("/drafts") && !new URL((request as Request).url).searchParams.has("dryRun"));
}

async function openEditor() {
  const row = (await screen.findByText("public-air")).closest("tr") as HTMLElement;
  // The row's actions are behind its one menu now (T-2287).
  await userEvent.click(within(row).getByRole("button", { name: /More actions/ }));
  await userEvent.click(await screen.findByRole("menuitem", { name: en.endpoints.edit }));
  return findFormPage();
}

function hideBox(dialog: HTMLElement, attribute: string) {
  return within(dialog).getByRole("checkbox", {
    name: `${en.endpoints.projection.hide} ${attribute}`,
  });
}

describe("endpoint projection ui", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    window.history.pushState({}, "", "/projects/banskabystrica/endpoints");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("reads the attribute names of every class out of the published JSON Schema", () => {
    expect(publishedTypes(JSON_SCHEMA)).toEqual([
      {
        name: "AirQualityObserved",
        attributes: ["calibrationOffset", "dateObserved", "pm10", "pm25"],
      },
    ]);
    expect(publishedTypes({ $defs: { Empty: {} } })).toEqual([]);
    expect(publishedTypes(null)).toEqual([]);
  });

  it("lists the endpoint's attributes and ticks the ones it already hides", async () => {
    renderEndpoints();
    const dialog = await openEditor();

    await waitFor(() => expect(hideBox(dialog, "pm10")).toBeInTheDocument());
    expect(hideBox(dialog, "calibrationOffset")).toBeChecked();
    expect(hideBox(dialog, "pm10")).not.toBeChecked();
  });

  it("leaves a masked attribute out of the published preview (EP-61, R9)", async () => {
    renderEndpoints();
    const dialog = await openEditor();

    await waitFor(() => expect(hideBox(dialog, "pm10")).toBeInTheDocument());
    const published = () =>
      within(dialog).getByText(en.endpoints.projection.effective, { exact: false }).parentElement
        ?.textContent ?? "";

    // Already hidden by the manifest, so it is absent before anybody clicks anything.
    expect(published()).not.toContain("calibrationOffset");
    expect(published()).toContain("pm10");

    await userEvent.click(hideBox(dialog, "pm10"));
    expect(published()).not.toContain("pm10");
    expect(published()).toContain("pm25");
  });

  it("proposes the masked attribute list as spec.projection.hiddenAttributes", async () => {
    const fetchMock = renderEndpoints();
    const dialog = await openEditor();

    await waitFor(() => expect(hideBox(dialog, "pm10")).toBeInTheDocument());
    await userEvent.click(hideBox(dialog, "pm10"));
    // Strict validation proposes nothing without a fresh green verdict (AG-62, T-0779).
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.check }));
    await waitFor(() =>
      expectOpen(within(dialog).getByRole("button", { name: en.endpoints.propose })),
    );
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.propose }));

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const body = (await writes(fetchMock)[0].clone().json()) as {
      spec: { projection?: { hiddenAttributes: string[] } };
    };
    expect(body.spec.projection?.hiddenAttributes).toEqual(["calibrationOffset", "pm10"]);
  });

  it("unhides an attribute the endpoint currently masks", async () => {
    const fetchMock = renderEndpoints();
    const dialog = await openEditor();

    await waitFor(() => expect(hideBox(dialog, "calibrationOffset")).toBeInTheDocument());
    await userEvent.click(hideBox(dialog, "calibrationOffset"));
    // Strict validation proposes nothing without a fresh green verdict (AG-62, T-0779).
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.check }));
    await waitFor(() =>
      expectOpen(within(dialog).getByRole("button", { name: en.endpoints.propose })),
    );
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.propose }));

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const body = (await writes(fetchMock)[0].clone().json()) as {
      spec: { projection?: unknown };
    };
    // An empty list is no narrowing at all, so the field goes away rather than shipping `[]`.
    expect(body.spec.projection).toBeUndefined();
  });

  it("hides an attribute the published schema does not name", async () => {
    const fetchMock = renderEndpoints();
    const dialog = await openEditor();

    await waitFor(() => expect(hideBox(dialog, "pm10")).toBeInTheDocument());
    await userEvent.type(
      within(dialog).getByLabelText(en.endpoints.projection.addHidden),
      "internalNote",
    );
    await userEvent.click(
      within(dialog).getByRole("button", { name: en.endpoints.projection.add }),
    );
    // Strict validation proposes nothing without a fresh green verdict (AG-62, T-0779).
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.check }));
    await waitFor(() =>
      expectOpen(within(dialog).getByRole("button", { name: en.endpoints.propose })),
    );
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.propose }));

    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const body = (await writes(fetchMock)[0].clone().json()) as {
      spec: { projection?: { hiddenAttributes: string[] } };
    };
    expect(body.spec.projection?.hiddenAttributes).toEqual([
      "calibrationOffset",
      "internalNote",
    ]);
  });

  it("offers no hide panel on a new endpoint, whose fresh slug publishes no schema yet", async () => {
    const fetchMock = renderEndpoints();
    await screen.findByText("public-air");
    await userEvent.click(screen.getByRole("button", { name: en.endpoints.add }));
    const dialog = await findFormPage();
    await userEvent.type(dialog.querySelector("#root_name") as HTMLElement, "bikes-regional");
    await userEvent.selectOptions(dialog.querySelector("#root_contextSpaceRef") as HTMLElement, "ovzdusie");
    await userEvent.selectOptions(
      dialog.querySelector("#root_audience") as HTMLElement,
      en.endpoints.audienceOption.public,
    );
    await userEvent.click(
      within(dialog).getByRole("checkbox", { name: en.endpoints.representationOption.csv }),
    );

    expect(within(dialog).queryByText(en.endpoints.section.hidden)).toBeNull();
    expect(within(dialog).queryByLabelText(en.endpoints.projection.addHidden)).toBeNull();
    // Nothing reads a schema of the slug that does not exist yet.
    expect(
      fetchMock.mock.calls.some((call) => {
        const input = call[0] as RequestInfo | URL;
        const url = input instanceof Request ? input.url : String(input);
        return url.includes("/schema/");
      }),
    ).toBe(false);

    // Strict validation proposes nothing without a fresh green verdict (AG-62, T-0779).
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.check }));
    await waitFor(() =>
      expectOpen(within(dialog).getByRole("button", { name: en.endpoints.propose })),
    );
    await userEvent.click(within(dialog).getByRole("button", { name: en.endpoints.propose }));
    await waitFor(() => expect(writes(fetchMock)).toHaveLength(1));
    const request = writes(fetchMock)[0];
    expect(request.method).toBe("POST");
    const body = (await request.clone().json()) as {
      spec: { projection?: unknown; enabledRepresentations: string[] };
    };
    expect(body.spec.projection).toBeUndefined();
  });

  it("previews another formalism from the version the schema index names (EP-46, EP-49)", async () => {
    renderEndpoints();
    const dialog = await openEditor();

    await waitFor(() => expect(hideBox(dialog, "pm10")).toBeInTheDocument());
    await userEvent.selectOptions(
      within(dialog).getByLabelText(en.endpoints.projection.formalism),
      "model.linkml.yaml",
    );

    // v2, from the index, not the v1 a hard-coded path would ask for.
    expect(
      await within(dialog).findByText(new RegExp(`schema/v2/model.linkml.yaml`)),
    ).toBeInTheDocument();
    expect(await within(dialog).findByText(/name: air-quality/)).toBeInTheDocument();
  });

  it("says so when the gateway does not compile the chosen formalism", async () => {
    renderEndpoints({ compiled: false });
    const dialog = await openEditor();

    await waitFor(() => expect(hideBox(dialog, "pm10")).toBeInTheDocument());
    await userEvent.selectOptions(
      within(dialog).getByLabelText(en.endpoints.projection.formalism),
      "model.linkml.yaml",
    );

    expect(
      await within(dialog).findByText(en.endpoints.projection.notCompiled),
    ).toBeInTheDocument();
  });

  it("the chip that takes an attribute back out of the hidden list says so", async () => {
    // It announced "Hide": a person unhiding an attribute was told they were hiding it, and the
    // endpoint then published a field they believed they had withheld (UI-01, UI-44).
    renderEndpoints();
    const dialog = await openEditor();
    await waitFor(() => expect(hideBox(dialog, "pm10")).toBeInTheDocument());
    const chip = within(dialog).getByRole("button", {
      name: new RegExp(`calibrationOffset.*${en.endpoints.projection.unhide}`, "s"),
    });
    expect(chip).toBeInTheDocument();
    expect(
      within(dialog).queryByRole("button", {
        name: new RegExp(`calibrationOffset.*${en.endpoints.projection.hide}$`, "s"),
      }),
      "and it no longer claims to hide what it reveals",
    ).toBeNull();
  });
});
