/**
 * T-6339 (ADR-N-066 2.6, DM-148, DM-153): a Policy entry's `attributes` block is a table in the
 * Policy form. Rows are `*` plus the attributes the model declares for the entry's types; columns
 * read, history, versions, metadata and edit; a cell the grant cannot give is disabled and says
 * why. The journey fills the matrix and reads the manifest the form proposes.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import { policySchema, policyUiSchema } from "../src/schemas/kinds";
import { toPolicyEnvelope } from "../src/routes/PoliciesPage";
import type { PolicyForm } from "../src/routes/PoliciesPage";
import {
  AttributeCapabilitiesField,
  CapabilityCell,
  CapabilityHints,
  GrantNote,
} from "../src/components/forms/widgets/AttributeCapabilitiesField";
import type { FieldProps } from "@rjsf/utils";
import type { ReactElement } from "react";
import {
  columnBlocked,
  grantLimits,
  matrixRows,
  toggled,
  valueBlocked,
  withCapability,
} from "../src/components/forms/widgets/attributeCapabilities";

const PROJECT = "banskabystrica";
const t = (key: string): string => i18n.t(key);

const MODEL = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "DataModel",
  metadata: { name: "air", namespace: PROJECT },
  spec: {
    linkml: "classes:\n  AirQualityObserved:\n    attributes:\n      name: { range: string }\n      pm10: { range: float }\n",
  },
};

function stubModels(): void {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL((input as Request).url ?? String(input), "http://portal.test");
      const body = url.pathname.endsWith("/datamodels") ? { kind: "List", items: [MODEL] } : { kind: "List", items: [] };
      return new Response(JSON.stringify(body), { status: 200, headers: { "Content-Type": "application/json" } });
    }),
  );
}

function form(formData: Partial<PolicyForm>, onChange: (data: PolicyForm) => void) {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <SchemaForm
          project={PROJECT}
          schema={policySchema(t, ["ovzdusie"], ["AirQualityObserved"])}
          uiSchema={policyUiSchema}
          formData={formData}
          onChange={(data) => onChange(data as PolicyForm)}
          onSubmit={() => {}}
        />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

const PUBLIC_READER: Partial<PolicyForm> = {
  name: "air-public",
  contextSpaceRef: "ovzdusie",
  effect: "permission",
  assignee: { kind: "role", id: "public" },
  operations: ["retrieveOps", "retrieveTemporal"],
  information: [{ entities: [{ type: "AirQualityObserved" }] }],
} as Partial<PolicyForm>;

describe("the attribute capability matrix (DM-153)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    stubModels();
  });
  afterEach(() => vi.unstubAllGlobals());

  it("lists * and the model's attributes, fills the matrix and proposes the block", async () => {
    const user = userEvent.setup();
    let last: PolicyForm | undefined;
    form(PUBLIC_READER, (data) => (last = data));
    const table = await screen.findByRole("table", { name: /Attribute capabilities of AirQualityObserved/ });
    await waitFor(() => expect(within(table).getByRole("rowheader", { name: "pm10" })).toBeTruthy());
    const rows = within(table).getAllByRole("rowheader").map((cell) => cell.textContent);
    expect(rows).toEqual(["All attributes (*)", "name", "pm10"]);
    // Every column says what it does, with an example.
    expect(screen.getByText(/Example: Allowed on pm10 serves its readings of last week/)).toBeTruthy();

    await user.selectOptions(within(table).getByRole("combobox", { name: "pm10: Read" }), "false");
    await user.selectOptions(within(table).getByRole("combobox", { name: "pm10: History" }), "true");
    const envelope = toPolicyEnvelope(PROJECT, "banskabystrica.sk", last as PolicyForm) as {
      spec: { information: { attributes?: unknown }[] };
    };
    expect(envelope.spec.information[0].attributes).toEqual({ pm10: { read: false, history: true } });

    // Back to the default: the attribute leaves the block, and the block leaves the manifest.
    await user.selectOptions(within(table).getByRole("combobox", { name: "pm10: Read" }), "");
    await user.selectOptions(within(table).getByRole("combobox", { name: "pm10: History" }), "");
    expect((last as PolicyForm).information?.[0]).not.toHaveProperty("attributes.pm10");
  });

  it("disables what a public reader cannot give and says why", async () => {
    form(PUBLIC_READER, () => {});
    const table = await screen.findByRole("table", { name: /Attribute capabilities/ });
    await waitFor(() => expect(within(table).getByRole("rowheader", { name: "pm10" })).toBeTruthy());
    // No write operation on a public grant: the edit column is left out and the note says why.
    expect(screen.getByText("A public grant never writes.")).toBeTruthy();
    expect(within(table).queryByRole("columnheader", { name: "Edit" })).toBeNull();
    expect(within(table).queryByRole("checkbox", { name: /Edit update/ })).toBeNull();
    // versions: read needs readVersions, keep is never public.
    const read = within(table).getByRole("checkbox", { name: "All attributes (*): Versions read" });
    expect(read.getAttribute("aria-disabled") ?? String((read as HTMLInputElement).disabled)).toMatch(/true/);
    expect(within(table).getAllByText("A public grant never keeps versions.").length).toBeGreaterThan(0);
    // History is allowed: retrieveTemporal is named.
    const history = within(table).getByRole("combobox", { name: "pm10: History" }) as HTMLSelectElement;
    expect(history.disabled).toBe(false);
  });

  it("says history needs a temporal operation when none is named", async () => {
    form({ ...PUBLIC_READER, operations: ["retrieveOps"] } as Partial<PolicyForm>, () => {});
    const table = await screen.findByRole("table", { name: /Attribute capabilities/ });
    await waitFor(() => expect(within(table).getByRole("rowheader", { name: "pm10" })).toBeTruthy());
    // Once per row: *, name and pm10.
    expect(within(table).getAllByText("Needs queryTemporal or retrieveTemporal in the operations.").length).toBe(3);
    expect(within(table).queryByRole("combobox", { name: /History/ })).toBeNull();
  });

  it("is in Slovak in Slovak", async () => {
    await i18n.changeLanguage("sk");
    form(PUBLIC_READER, () => {});
    expect(await screen.findByRole("table", { name: /Schopnosti atribútov typu AirQualityObserved/ })).toBeTruthy();
    expect(screen.getByText(/Napríklad Povolené pri pm10/)).toBeTruthy();
  });
});

describe("the matrix logic (DM-148, DM-152)", () => {
  it("reads the limits jc-core checks", () => {
    const internal = grantLimits({ operations: ["updateOps", "queryTemporal"], actions: ["readVersions"], assignee: { kind: "group", id: "staff" } });
    expect(internal).toEqual({ temporal: true, write: true, readVersions: true, public: false });
    // A group never holds the temporal reads (Table 4.20-2).
    expect(grantLimits({ operations: ["retrieveOps"] }).temporal).toBe(false);
    const open = grantLimits({ operations: ["updateOps"], assignee: { kind: "role", id: "public" } });
    expect(columnBlocked("write", open)).toBe("policies.capabilities.neverPublicWrite");
    expect(valueBlocked("metadata", "modifiedBy.actor", open)).toBe("policies.capabilities.neverPublicActor");
    expect(valueBlocked("metadata", "observedAt", open)).toBeUndefined();
    expect(valueBlocked("versions", "keep", internal)).toBeUndefined();
  });

  it("keeps listed names, then held keys, and drops empty rows", () => {
    expect(matrixRows(["name"], ["name", "pm10"], ["typo", "*"])).toEqual(["*", "name", "typo"]);
    expect(matrixRows([], ["name", "pm10"], [])).toEqual(["*", "name", "pm10"]);
    const one = withCapability(undefined, "name", "versions", ["read"]);
    expect(one).toEqual({ name: { versions: ["read"] } });
    expect(withCapability(one, "name", "versions", undefined)).toBeUndefined();
    // An empty list is a value: it narrows `*` to nothing for that attribute.
    expect(withCapability(one, "name", "versions", [])).toEqual({ name: { versions: [] } });
    expect(toggled(["read", "keep"], ["keep"], "read", true)).toEqual(["read", "keep"]);
  });
});

describe("the matrix cells on their own", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    stubModels();
  });
  afterEach(() => vi.unstubAllGlobals());

  const limits = grantLimits({ operations: ["updateOps", "queryTemporal"], assignee: { kind: "group", id: "staff" } });
  const wrap = (node: ReactElement) =>
    render(
      <QueryClientProvider client={new QueryClient()}>
        <I18nextProvider i18n={i18n}>{node}</I18nextProvider>
      </QueryClientProvider>,
    );

  it("names the chosen metadata members and takes a row back to the * value", async () => {
    const user = userEvent.setup();
    const changes: unknown[] = [];
    wrap(
      <CapabilityCell
        attribute="pm10"
        column="metadata"
        row={{ metadata: ["observedAt", "unitCode"] }}
        limits={limits}
        frozen={false}
        describedBy="hints"
        onChange={(value) => changes.push(value)}
      />,
    );
    expect(screen.getByText("observedAt, unitCode")).toBeTruthy();
    await user.click(screen.getByText("observedAt, unitCode"));
    await user.click(screen.getByRole("button", { name: "pm10: Metadata: Same as all (*)" }));
    await user.click(screen.getByRole("checkbox", { name: "pm10: Metadata provenance" }));
    expect(changes).toEqual([undefined, ["observedAt", "unitCode", "provenance"]]);
  });

  it("shows the * row's value in an unset cell and a dash where the grant gives nothing", () => {
    wrap(
      <>
        <CapabilityCell attribute="name" column="read" row={undefined} every={{ read: false }} limits={limits} frozen={false} describedBy="h" onChange={() => {}} />
        <CapabilityCell attribute="name" column="history" row={undefined} limits={grantLimits({})} frozen={false} describedBy="h" onChange={() => {}} />
        <CapabilityCell attribute="name" column="write" row={undefined} limits={limits} frozen={false} describedBy="h" onChange={() => {}} />
      </>,
    );
    expect(screen.getByRole("option", { name: "Same as all (*): Withheld" })).toBeTruthy();
    expect(screen.getByText("Needs queryTemporal or retrieveTemporal in the operations.")).toBeTruthy();
    expect(screen.getByRole("checkbox", { name: "name: Edit update" })).toBeTruthy();
  });

  it("says nothing when the grant can give every column", () => {
    const all = grantLimits({ operations: ["updateOps", "queryTemporal"], actions: ["readVersions"] });
    const { container } = wrap(<GrantNote limits={all} />);
    expect(container.textContent).toBe("");
    wrap(<CapabilityHints />);
    expect(screen.getByText("What each column means, with examples")).toBeTruthy();
  });

  it("renders as a field and writes the block through its path", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    const props = {
      fieldPathId: { $id: "root_information_0_attributes", path: ["information", 0, "attributes"] },
      formData: undefined,
      onChange,
    } as unknown as FieldProps;
    wrap(<AttributeCapabilitiesField {...props} />);
    await user.selectOptions(screen.getByRole("combobox", { name: "All attributes (*): Read" }), "true");
    expect(onChange).toHaveBeenCalledWith({ "*": { read: true } }, ["information", 0, "attributes"]);
  });
});
