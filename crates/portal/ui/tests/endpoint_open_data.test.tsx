/**
 * T-2405 (EP-62, EP-65, EP-69, UI-02): an Endpoint is published to the open-data catalogue from
 * its own form, and the manifest says exactly what the fields said.
 *
 * The publication existed as a manifest block and as a read-only page; the only way to turn it on
 * was to write `spec.publish.ckan` by hand. Naming a catalogue in the form is what publishes the
 * endpoint now, and leaving the field empty writes no block at all, so an endpoint nobody
 * published carries no empty declaration.
 *
 * The dataset's visibility is not in the form on purpose: it follows `spec.audience`, closed by
 * default, and a field offering it would be offering to contradict the endpoint (EP-69).
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import validator from "../src/components/forms/validator";
import { endpointSchema } from "../src/schemas/kinds";
import { toEnvelope, toForm } from "../src/routes/EndpointsPage";
import type { Manifest } from "../src/api/manifest";
import en from "../src/locales/en.json";

const PROJECT = "helsinki";
const SLUG = "scsd2eehkx42n53z2zyd6vshfh7s7irf";
const t = (key: string): string => i18n.t(key);

/** The form of one endpoint, with whatever the case is about. */
function form(over: Record<string, unknown> = {}) {
  return {
    name: "helsinki-bikes",
    contextSpaceRef: "bikes",
    audience: "public",
    enabledRepresentations: ["ngsi-ld", "csv"],
    ...over,
  } as Parameters<typeof toEnvelope>[1];
}

function spec(manifest: Manifest): Record<string, unknown> {
  return manifest.spec as Record<string, unknown>;
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

describe("the open-data option of an Endpoint", () => {
  it("writes the publication the catalogue name turns on, without the removed row mirror (T-5060)", () => {
    const manifest = toEnvelope(
      PROJECT,
      form({
        publish: {
          ckan: {
            instanceRef: "hel-fi",
            organization: "hel-fi",
            name: "ilmanlaatu",
            datastore: { representation: "csv", refresh: "onReconcile" },
          },
        },
      }),
      SLUG,
      [],
    );
    expect(spec(manifest).publish).toEqual({
      ckan: {
        instanceRef: { kind: "CkanInstance", name: "hel-fi" },
        organization: "hel-fi",
        name: "ilmanlaatu",
      },
    });
  });

  it("writes no publication at all while no catalogue is named", () => {
    for (const publish of [
      undefined,
      { ckan: undefined },
      { ckan: { instanceRef: "" } },
      { ckan: { instanceRef: "   ", organization: "hel-fi" } },
      // The other fields alone do not publish: they say how, not whether.
      { ckan: { organization: "hel-fi", name: "ilmanlaatu" } },
    ]) {
      const manifest = toEnvelope(PROJECT, form({ publish }), SLUG, []);
      expect(spec(manifest).publish, JSON.stringify(publish)).toBeUndefined();
    }
  });

  it("leaves out the empty parts, so the manifest reads as what it publishes", () => {
    const manifest = toEnvelope(
      PROJECT,
      form({ publish: { ckan: { instanceRef: "hel-fi", organization: "  ", name: "" } } }),
      SLUG,
      [],
    );
    expect(spec(manifest).publish).toEqual({
      ckan: { instanceRef: { kind: "CkanInstance", name: "hel-fi" } },
    });
  });

  it("is the same publication in both directions", () => {
    const written = {
      ckan: {
        instanceRef: "hel-fi",
        name: "ilmanlaatu",
        datastore: { representation: "csv", refresh: "onChange" },
      },
    };
    const manifest = toEnvelope(PROJECT, form({ publish: written }), SLUG, []);
    const back = toForm(manifest);
    expect(back.publish?.ckan?.instanceRef).toBe("hel-fi");
    expect(back.publish?.ckan?.name).toBe("ilmanlaatu");
    // A block written before T-5060 loses its row mirror on the way in, so a save drops it.
    expect(back.publish?.ckan).not.toHaveProperty("datastore");
    // A second round trip keeps the reference a name and not "[object Object]".
    expect(toForm(toEnvelope(PROJECT, { ...form(), ...back }, SLUG, [])).publish?.ckan?.instanceRef).toBe(
      "hel-fi",
    );
    // An endpoint that publishes nothing comes back with nothing, not with an empty block.
    expect(toForm(toEnvelope(PROJECT, form(), SLUG, [])).publish).toBeUndefined();
  });

  it("keeps the publication of an endpoint whose other fields are edited", () => {
    const stored = toEnvelope(
      PROJECT,
      form({ publish: { ckan: { instanceRef: "hel-fi" } } }),
      SLUG,
      [],
    );
    const edited = { ...toForm(stored), audience: "organization" };
    const next = toEnvelope(PROJECT, edited, SLUG, [], undefined, stored);
    expect(spec(next).audience).toBe("organization");
    expect(spec(next).publish).toEqual({
      ckan: { instanceRef: { kind: "CkanInstance", name: "hel-fi" } },
    });
  });

  it("offers no visibility of its own, because the dataset follows the endpoint", () => {
    // EP-69: a public endpoint becomes a public dataset and anything narrower a private one. The
    // mapping is the publisher's, and there is no field here that could disagree with it.
    const schema = JSON.stringify(endpointSchema(t, ["bikes"], [], undefined, ["hel-fi"]));
    for (const forbidden of ["private", "visibility", "public\":"]) {
      expect(schema.includes(forbidden), `the form offers no ${forbidden}`).toBe(false);
    }
  });

  it("offers the catalogues of the project, and keeps one the endpoint already names", () => {
    const listed = endpointSchema(t, ["bikes"], [], undefined, ["hel-fi", "data-gov"]) as unknown as {
      properties: { publish: { properties: { ckan: { properties: { instanceRef: { enum?: string[]; pattern?: string } } } } } };
    };
    const field = listed.properties.publish.properties.ckan.properties.instanceRef;
    expect(field.enum).toEqual(["hel-fi", "data-gov"]);

    // A project with no catalogue manifest yet: free text under the pattern the API validates,
    // never an empty list that would make the field unusable (PF-51).
    const none = endpointSchema(t, ["bikes"]) as unknown as typeof listed;
    const open = none.properties.publish.properties.ckan.properties.instanceRef;
    expect(open.enum).toBeUndefined();
    expect(open.pattern).toBeTruthy();
  });

  it("offers no row mirror: CKAN holds no rows (T-5060)", () => {
    const schema = endpointSchema(t, ["bikes"]) as unknown as {
      properties: { publish: { properties: { ckan: { properties: Record<string, unknown> } } } };
    };
    expect(Object.keys(schema.properties.publish.properties.ckan.properties)).not.toContain("datastore");
  });

  it("refuses at the field a catalogue name and an organization that are not names", () => {
    const schema = endpointSchema(t, ["bikes"]);
    const base = {
      name: "helsinki-bikes",
      contextSpaceRef: "bikes",
      audience: "public",
      enabledRepresentations: ["csv"],
    };
    expect(validator.validateFormData({ ...base, publish: { ckan: { instanceRef: "hel-fi" } } }, schema).errors).toEqual([]);
    for (const bad of [
      { instanceRef: "Hel Fi" },
      { instanceRef: "hel-fi", organization: "Helsingin Kaupunki" },
      { instanceRef: "hel-fi", name: "Ilmanlaatu" },
    ]) {
      expect(
        validator.validateFormData({ ...base, publish: { ckan: bad } }, schema).errors,
        JSON.stringify(bad),
      ).not.toEqual([]);
    }
  });

  it("shows the option under its own heading, in every language", async () => {
    for (const locale of ["en", "sk", "cs", "de"] as const) {
      await i18n.changeLanguage(locale);
      const bundle = i18n.getResourceBundle(locale, "translation") as typeof en;
      const { unmount } = render(
        <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
          <I18nextProvider i18n={i18n}>
            <SchemaForm
              schema={endpointSchema(t, ["bikes"], [], undefined, ["hel-fi"])}
              formData={{ publish: { ckan: { instanceRef: "hel-fi" } } }}
              onSubmit={() => {}}
            />
          </I18nextProvider>
        </QueryClientProvider>,
      );
      expect(
        screen.getByLabelText(new RegExp(bundle.endpoints.field.ckanInstance)),
        locale,
      ).toBeInTheDocument();
      expect(screen.getAllByText(new RegExp(bundle.endpoints.field.openData)).length, locale).toBeGreaterThan(0);
      unmount();
    }
    await i18n.changeLanguage("en");
  });

  it("turns the publication on from the field a person types into", async () => {
    const user = userEvent.setup();
    let held: unknown;
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <SchemaForm
            schema={endpointSchema(t, ["bikes"])}
            formData={{ name: "helsinki-bikes", audience: "public", enabledRepresentations: ["csv"] }}
            onChange={(data) => {
              held = data;
            }}
            onSubmit={() => {}}
          />
        </I18nextProvider>
      </QueryClientProvider>,
    );
    await user.type(screen.getByLabelText(new RegExp(en.endpoints.field.ckanInstance)), "hel-fi");
    const written = held as { publish?: { ckan?: { instanceRef?: string } } };
    expect(written.publish?.ckan?.instanceRef).toBe("hel-fi");
    expect(
      spec(toEnvelope(PROJECT, { ...form(), ...written } as Parameters<typeof toEnvelope>[1], SLUG, [])).publish,
    ).toEqual({ ckan: { instanceRef: { kind: "CkanInstance", name: "hel-fi" } } });
  });
});
