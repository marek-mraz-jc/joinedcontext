/**
 * UI-03, CC-43: a form that references an entity picks it from what the chosen space serves,
 * read through the gateway under the person's own session.
 *
 * The Subscription form is the one that holds entity references; each row names a type and,
 * optionally, one entity of it. The row's id is a picker once the form has a space and the row a
 * type, and a plain box before that, so a person who holds a URN can still paste it.
 */
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import { subscriptionSchema, subscriptionUiSchema } from "../src/schemas/kinds";
import { EntitySelectorField } from "../src/components/forms/widgets/EntitySelectorField";
import { portalFields } from "../src/components/forms/widgets";
import { expectNoViolations } from "./checks";

/** What the space answers: normalized NGSI-LD, the name a Property. */
const SENSORS = [
  {
    id: "urn:ngsi-ld:AirQualityObserved:hel.fi:air:kallio",
    type: "AirQualityObserved",
    name: { type: "Property", value: "Kallio" },
  },
  {
    id: "urn:ngsi-ld:AirQualityObserved:hel.fi:air:vallila",
    type: "AirQualityObserved",
    name: { type: "Property", value: "Vallila" },
  },
];

let changes: unknown[] = [];
let fetchMock: ReturnType<typeof vi.fn>;

function renderForm(formData: Record<string, unknown>) {
  changes = [];
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <SchemaForm
          schema={subscriptionSchema(i18n.t.bind(i18n), ["air", "mobility"])}
          uiSchema={subscriptionUiSchema}
          formData={formData}
          onSubmit={() => undefined}
          onChange={(data) => changes.push(data)}
        />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

const idBox = () => screen.getByLabelText(en.subscriptions.field.entityId);
const last = () => changes.at(-1) as { entities?: { type?: string; id?: string }[] } | undefined;

beforeEach(async () => {
  await i18n.changeLanguage("en");
  fetchMock = vi.fn(async () =>
    new Response(JSON.stringify(SENSORS), { status: 200, headers: { "content-type": "application/ld+json" } }),
  );
  vi.stubGlobal("fetch", fetchMock);
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("an entity reference in a form (UI-03)", () => {
  it("is the field the Subscription form's rows name", () => {
    expect(portalFields.entitySelector).toBe(EntitySelectorField);
    expect(subscriptionUiSchema.entities).toEqual({
      items: { "ui:field": "entitySelector", "ui:options": { spaceField: "contextSpaceRef" } },
    });
  });

  // UI-03: with a space and a type the id is chosen from the space's own entities.
  it("lists the entities of the form's space and the row's type, and keeps the one chosen", async () => {
    const user = userEvent.setup();
    renderForm({ contextSpaceRef: "air", entities: [{ type: "AirQualityObserved" }] });

    expect(idBox()).toHaveAttribute("role", "combobox");
    await user.type(idBox(), "Kal");
    // The search is debounced: wait for the call that carries the typed term, then pick.
    const typedTerm = ([input]: unknown[]) =>
      new URL(String(input), window.location.origin).searchParams.get("q") === 'name~="Kal"';
    await waitFor(() => expect(fetchMock.mock.calls.some(typedTerm)).toBe(true));
    await user.click(await screen.findByRole("option", { name: /Kallio/ }));

    expect(last()?.entities?.[0]).toEqual({
      type: "AirQualityObserved",
      id: "urn:ngsi-ld:AirQualityObserved:hel.fi:air:kallio",
    });
    // CC-43: read through the gateway's space surface, with the person's own session.
    const calls = fetchMock.mock.calls as [string, RequestInit][];
    for (const [input, init] of calls) {
      const asked = new URL(input, window.location.origin);
      expect(asked.pathname).toBe("/cs/air/ngsi-ld/v1/entities");
      expect(asked.searchParams.get("type")).toBe("AirQualityObserved");
      expect(init.credentials).toBe("same-origin");
    }
  });

  it("takes a pasted id as it is, without asking the list", async () => {
    const user = userEvent.setup();
    renderForm({ contextSpaceRef: "air", entities: [{ type: "AirQualityObserved" }] });

    await user.click(idBox());
    await user.paste("urn:ngsi-ld:AirQualityObserved:hel.fi:air:pasila");
    await user.tab();

    await waitFor(() =>
      expect(last()?.entities?.[0]?.id).toBe("urn:ngsi-ld:AirQualityObserved:hel.fi:air:pasila"),
    );
  });

  it("says what to choose first while the row has no type, and still takes a typed id", async () => {
    const user = userEvent.setup();
    renderForm({ contextSpaceRef: "air", entities: [{}] });

    expect(idBox()).not.toHaveAttribute("role", "combobox");
    expect(idBox()).toBeEnabled();
    expect(idBox()).toHaveAccessibleDescription(expect.stringContaining(en.form.entityPickerPending));
    expect(screen.queryByRole("alert")).toBeNull();

    await user.type(idBox(), "urn:ngsi-ld:Sensor:x");
    expect(last()?.entities?.[0]?.id).toBe("urn:ngsi-ld:Sensor:x");
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("asks for nothing until the form names a space", () => {
    renderForm({ entities: [{ type: "AirQualityObserved" }] });
    expect(idBox()).not.toHaveAttribute("role", "combobox");
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("has no axe violation, waiting and picking", async () => {
    const { container, unmount } = renderForm({ contextSpaceRef: "air", entities: [{}] });
    await expectNoViolations(container);
    unmount();
    const picking = renderForm({ contextSpaceRef: "air", entities: [{ type: "AirQualityObserved" }] });
    await expectNoViolations(picking.container);
  });
});
