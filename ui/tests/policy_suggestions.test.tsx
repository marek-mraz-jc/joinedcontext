// covers (T-2137, the module gate in gate_modules.test.ts): src/components/forms/widgets/SuggestWidgets.tsx,
// the Policy form's grantee and attribute suggestions (T-3217): a new user picks a role, group,
// service account or person that exists, and the attribute names of the types the policy covers.
import type { ReactNode } from "react";
import { render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import type { WidgetProps } from "@rjsf/utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { FormDataContext } from "../src/components/forms/widgets/EntitySelectorField";
import { FormProjectContext } from "../src/components/forms/widgets/ModelWidgets";
import { AssigneePicker, AttributeSuggest } from "../src/components/forms/widgets/SuggestWidgets";
import { isPersonId } from "../src/api/personNames";

const list = (items: unknown[]) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items });
const named = (kind: string, name: string, namespace: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind,
  metadata: { name, namespace },
  spec: {},
});
const MODEL = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "DataModel",
  metadata: { name: "air", namespace: "helsinki" },
  spec: {
    format: "linkml",
    source: [
      "id: https://example.org/air",
      "name: air",
      "classes:",
      "  AirQualityObserved:",
      "    attributes:",
      "      pm10:",
      "        range: float",
      "      no2:",
      "        range: float",
      "      refDevice:",
      "        range: Device",
      "        annotations:",
      "          ngsi_ld_kind: Relationship",
      "  Device:",
      "    attributes:",
      "      serial:",
      "        range: string",
    ].join("\n"),
  },
};

const JANA = "00000000-0000-4000-8000-000000000001";

function stub(peopleStatus = 200) {
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL) => {
      const url = new URL((input as Request).url);
      const json = (body: unknown, status = 200) =>
        new Response(JSON.stringify(body), { status, headers: { "Content-Type": status >= 400 ? "application/problem+json" : "application/json" } });
      if (url.pathname === "/api/v1/projects/helsinki/roles") return json(list([named("Role", "air-reader", "helsinki")]));
      if (url.pathname === "/api/v1/projects/org/roles") return json(list([named("Role", "viewer", "org"), named("Role", "air-reader", "org")]));
      if (url.pathname === "/api/v1/projects/org/groups") return json(list([named("Group", "air-team", "org")]));
      if (url.pathname === "/api/v1/projects/helsinki/serviceaccounts") return json(list([named("ServiceAccount", "sensor-feed", "helsinki")]));
      if (url.pathname.endsWith("/permissions/me")) {
        return json({ project: "org", bootstrap: true, grants: [] });
      }
      if (url.pathname === "/api/v1/organization/people") {
        return peopleStatus === 200
          ? json({ items: [{ id: JANA, username: "jana", email: "jana@hel.fi", firstName: "Jana", lastName: "K", enabled: true }] })
          : json({ title: "Forbidden", status: 403 }, 403);
      }
      if (url.pathname === "/api/v1/projects/helsinki/datamodels") return json(list([MODEL]));
      return json(list([]));
    }),
  );
}

function props(id: string, extra: Partial<WidgetProps> = {}): WidgetProps {
  return {
    id,
    name: id,
    value: undefined,
    required: false,
    disabled: false,
    readonly: false,
    autofocus: false,
    label: id,
    onChange: vi.fn(),
    onBlur: vi.fn(),
    onFocus: vi.fn(),
    options: {},
    schema: { type: "string" },
    uiSchema: {},
    rawErrors: [],
    formContext: {},
    registry: {} as WidgetProps["registry"],
    ...extra,
  } as WidgetProps;
}

function wrap(form: unknown, node: ReactNode) {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <FormProjectContext.Provider value="helsinki">
          <FormDataContext.Provider value={form}>{node}</FormDataContext.Provider>
        </FormProjectContext.Provider>
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

function field(id: string): HTMLElement {
  const found = document.getElementById(id);
  if (!found) throw new Error(`no field ${id}`);
  return found;
}

/** The values the field's suggestion list offers. */
function offered(input: HTMLElement): string[] {
  const id = input.getAttribute("list");
  const datalist = id ? document.getElementById(id) : null;
  return [...(datalist?.querySelectorAll("option") ?? [])].map((option) => option.getAttribute("value") ?? "");
}

describe("the Policy form's suggestions (T-3217)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("offers the project's and the organization's roles once each for a role", async () => {
    stub();
    wrap({ assignee: { kind: "role" } }, <AssigneePicker {...props("root_assignee_id")} />);
    await waitFor(() => expect(offered(field("root_assignee_id"))).toEqual(["air-reader", "viewer"]));
  });

  it("offers the groups for a group and the service accounts for an account", async () => {
    stub();
    const { unmount } = wrap({ assignee: { kind: "group" } }, <AssigneePicker {...props("root_assignee_id")} />);
    await waitFor(() => expect(offered(screen.getByRole("combobox"))).toEqual(["air-team"]));
    unmount();
    wrap({ assignee: { kind: "serviceAccount" } }, <AssigneePicker {...props("root_assignee_id")} />);
    await waitFor(() => expect(offered(screen.getByRole("combobox"))).toEqual(["sensor-feed"]));
  });

  it("picks a person by their Keycloak id, never the e-mail jc-core refuses (ADR-N-061)", async () => {
    stub(200);
    const onChange = vi.fn();
    wrap({ assignee: { kind: "user" } }, <AssigneePicker {...props("root_assignee_id", { onChange })} />);
    const { default: userEvent } = await import("@testing-library/user-event");
    await userEvent.click(screen.getByRole("combobox"));
    await userEvent.click(await screen.findByRole("option", { name: /Jana/ }));
    expect(onChange).toHaveBeenCalledWith(JANA);
    expect(isPersonId(onChange.mock.calls[0][0] as string)).toBe(true);
  });

  it("says a DID is typed, with an example", () => {
    stub();
    wrap({ assignee: { kind: "did" } }, <AssigneePicker {...props("root_assignee_id")} />);
    expect(screen.getByText(/did:web:example.org/)).toBeInTheDocument();
  });

  it("offers the properties of the covered types, and the relationships to a relationship field", async () => {
    stub();
    const form = { information: [{ entities: [{ type: "AirQualityObserved" }] }] };
    const { unmount } = wrap(
      form,
      <AttributeSuggest {...props("root_information_0_propertyNames_0", { options: { kind: "Property" } })} />,
    );
    await waitFor(() => expect(offered(screen.getByRole("combobox"))).toEqual(["no2", "pm10"]));
    unmount();
    wrap(form, <AttributeSuggest {...props("root_information_0_relationshipNames_0", { options: { kind: "Relationship" } })} />);
    await waitFor(() => expect(offered(screen.getByRole("combobox"))).toEqual(["refDevice"]));
  });

  it("asks nothing before a type is chosen, and a typed value still goes through", async () => {
    stub();
    const onChange = vi.fn();
    wrap({ information: [{ entities: [] }] }, <AttributeSuggest {...props("root_information_0_propertyNames_0", { onChange })} />);
    const input = screen.getByRole("textbox");
    expect(input.getAttribute("list")).toBeNull();
    input.focus();
    const { default: userEvent } = await import("@testing-library/user-event");
    await userEvent.type(input, "x");
    expect(onChange).toHaveBeenLastCalledWith("x");
  });
});

describe("the Subscription form's attribute suggestions (T-3291)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  const subscription = { contextSpaceRef: "air", entities: [{ type: "AirQualityObserved" }] };
  const watched = (value?: string) =>
    props("root_watchedAttributes_0", { value, options: { kind: "any", typesFrom: "entities" } });

  it("offers every attribute of the watched types, properties and relationships alike", async () => {
    stub();
    wrap(subscription, <AttributeSuggest {...watched()} />);
    await waitFor(() => expect(offered(field("root_watchedAttributes_0"))).toEqual(["no2", "pm10", "refDevice"]));
  });

  it("says beside the field when a typed name is no attribute of the watched types", async () => {
    stub();
    wrap(subscription, <AttributeSuggest {...watched("pm1O")} />);
    expect(
      await screen.findByText("pm1O is not an attribute of AirQualityObserved in the project's models; check the spelling."),
    ).toBeInTheDocument();
  });

  it("asks nothing while no type is watched, and says nothing about a name it cannot judge", async () => {
    stub();
    wrap({ contextSpaceRef: "air", entities: [] }, <AttributeSuggest {...watched("anything")} />);
    expect(offered(field("root_watchedAttributes_0"))).toEqual([]);
    expect(screen.queryByText(/is not an attribute of/)).toBeNull();
  });
});
