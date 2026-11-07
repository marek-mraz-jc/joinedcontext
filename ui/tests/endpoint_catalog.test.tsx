/**
 * T-2789 (EP-78, EP-79, EP-80, UI-50): an Endpoint's catalogue description is written from its
 * form, keeps every language the manifest already holds, starts from the Organization's open-data
 * desk and never a person, and its page says in sentences what the licence lets a user do.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { endpointSchema } from "../src/schemas/kinds";
import { toEnvelope, toForm } from "../src/routes/EndpointsPage";
import { CatalogSection } from "../src/pages/endpoints/CatalogSection";
import {
  CATALOG_LICENCES,
  catalogForm,
  catalogOf,
  EMAIL_PATTERN,
  offerSentences,
  prefillFromOrganization,
  SPATIAL_PATTERN,
} from "../src/pages/endpoints/catalog";
import type { CatalogManifest } from "../src/pages/endpoints/catalog";
import type { Manifest } from "../src/api/manifest";

const SLUG = "scsd2eehkx42n53z2zyd6vshfh7s7irf";
const t = (key: string, values?: Record<string, string>): string => i18n.t(key, values);

/** What a steward's manifest already holds: Slovak beside English, two keyword languages. */
const STORED: CatalogManifest = {
  publisher: { name: { sk: "Mesto", en: "The city" }, uri: "https://city.example.org/" },
  contactPoint: { name: "Open data desk", email: "opendata@city.example.org" },
  license: "CC_BY_4_0",
  attribution: { sk: "Zdroj: mesto", en: "Source: the city" },
  themes: ["TRAN"],
  keywords: { sk: ["doprava"], en: ["transport"] },
  spatial: ["DE300"],
  temporal: { start: "2020-01-01" },
  frequency: "DAILY",
  source: [
    {
      url: "https://data.example.org/dataset/stops",
      title: { sk: "Zastávky", en: "Stops" },
      description: { sk: "Všetky zastávky", en: "Every stop" },
    },
  ],
  pipelineRef: { kind: "Pipeline", name: "stops-import" },
};

function endpoint(catalog?: CatalogManifest): Manifest {
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "Endpoint",
    metadata: { name: "stops", namespace: "city" },
    spec: {
      contextSpaceRef: "transport",
      slug: SLUG,
      audience: "public",
      enabledRepresentations: ["ngsi-ld"],
      ...(catalog ? { catalog } : {}),
    },
  };
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the catalogue description in the endpoint form", () => {
  it("round-trips the stored block unchanged when nothing is edited", () => {
    const form = toForm(endpoint(STORED));
    const next = toEnvelope("city", form, SLUG, [], undefined, endpoint(STORED), "en");
    expect((next.spec as { catalog?: CatalogManifest }).catalog).toEqual(STORED);
  });

  it("writes an edited text over the entry it showed and keeps the other languages", () => {
    const form = catalogForm(STORED);
    const next = catalogOf(
      { ...form, publisher: { ...form?.publisher, name: "The city of stops" }, keywords: ["bus", "tram"] },
      STORED,
      "sk",
    );
    expect(next?.publisher?.name).toEqual({ sk: "Mesto", en: "The city of stops" });
    expect(next?.keywords).toEqual({ sk: ["doprava"], en: ["bus", "tram"] });
    expect(next?.source?.[0]?.title).toEqual({ sk: "Zastávky", en: "Stops" });
  });

  it("writes a new text in the author's language", () => {
    const next = catalogOf({ attribution: "Zdroj: mesto" }, undefined, "sk");
    expect(next).toEqual({ attribution: { sk: "Zdroj: mesto" } });
  });

  it("writes no block for a form that holds nothing, and drops a cleared entry", () => {
    expect(catalogOf({ themes: [], keywords: ["  "], publisher: { name: "" } }, undefined, "en")).toBeUndefined();
    const cleared = catalogOf({ ...catalogForm(STORED), attribution: "" }, STORED, "en");
    expect(cleared?.attribution).toEqual({ sk: "Zdroj: mesto" });
  });

  it("names the pipeline by reference and a source without an address not at all", () => {
    const next = catalogOf(
      { pipelineRef: "stops-import", source: [{ url: " ", title: "Nothing" }] },
      undefined,
      "en",
    );
    expect(next).toEqual({ pipelineRef: { kind: "Pipeline", name: "stops-import" } });
  });

  it("offers the licences, themes and frequencies as words, and the pipelines as a list", () => {
    const schema = endpointSchema(t, ["transport"], [], undefined, [], ["stops-import"]);
    const catalog = (schema.properties as Record<string, { properties: Record<string, unknown> }>).catalog;
    const license = catalog.properties.license as { oneOf: { const: string; title: string }[] };
    expect(license.oneOf.map((option) => option.const)).toEqual([...CATALOG_LICENCES]);
    expect(license.oneOf[0].title).toBe("Creative Commons Attribution 4.0");
    expect(catalog.properties.pipelineRef).toMatchObject({ enum: ["stops-import"] });
  });

  it("checks an address and an area the way the platform does", () => {
    const email = new RegExp(EMAIL_PATTERN);
    expect(email.test("opendata@city.example.org")).toBe(true);
    expect(email.test("open data@city")).toBe(false);
    const spatial = new RegExp(SPATIAL_PATTERN);
    for (const ok of ["SK032", "DE", "https://sws.geonames.org/3061186/"]) {
      expect(spatial.test(ok), ok).toBe(true);
    }
    for (const bad of ["sk032", "Somewhere", "https://x/a b"]) {
      expect(spatial.test(bad), bad).toBe(false);
    }
  });
});

describe("the catalogue a new endpoint starts from (EP-80)", () => {
  const organization = {
    metadata: { title: { en: "City of Example" } },
    spec: {
      domain: "city.example.org",
      contacts: [
        { role: "data-protection", name: "Eva", email: "eva@city.example.org" },
        { role: "open-data", name: "Open data desk", email: "opendata@city.example.org" },
      ],
    },
  };

  it("takes the Organization's name, domain and open-data desk", () => {
    expect(prefillFromOrganization(organization)).toEqual({
      publisher: { name: "City of Example", uri: "https://city.example.org/" },
      contactPoint: { name: "Open data desk", email: "opendata@city.example.org" },
    });
  });

  it("never offers a person's contact when there is no open-data desk", () => {
    const withoutDesk = {
      ...organization,
      spec: { ...organization.spec, contacts: organization.spec.contacts.slice(0, 1) },
    };
    expect(prefillFromOrganization(withoutDesk)?.contactPoint).toBeUndefined();
    expect(prefillFromOrganization(undefined)).toBeUndefined();
  });
});

describe("the offer as sentences (EP-79)", () => {
  it.each([
    ["CC0", "public", ["Anyone may use this data for any purpose, without conditions."]],
    ["CC_BY_4_0", "public", ["Anyone may use this data if they credit The city."]],
    [
      "ODC_ODBL",
      "organization",
      [
        "Anyone in the organization may use this data if they credit The city.",
        "What they publish from it goes out under the same licence.",
      ],
    ],
  ])("%s for a %s endpoint", (license, audience, expected) => {
    expect(offerSentences(t, { ...STORED, license }, audience, "en")).toEqual(expected);
  });

  it("says no offer is made without a licence", () => {
    expect(offerSentences(t, { publisher: STORED.publisher }, "public", "en")[0]).toMatch(/No licence/);
  });
});

describe("the catalogue section of the endpoint page", () => {
  function show(catalog?: CatalogManifest) {
    return render(
      <I18nextProvider i18n={i18n}>
        <CatalogSection slug={SLUG} catalog={catalog} audience="public" />
      </I18nextProvider>,
    );
  }

  it("shows the record as a card and the offer as a sentence", () => {
    show(STORED);
    expect(screen.getByRole("link", { name: "The city" })).toHaveAttribute("href", "https://city.example.org/");
    expect(screen.getByRole("link", { name: "Creative Commons Attribution 4.0" })).toHaveAttribute(
      "href",
      "http://publications.europa.eu/resource/authority/licence/CC_BY_4_0",
    );
    expect(screen.getByText("Every day")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Open data desk" })).toHaveAttribute(
      "href",
      "mailto:opendata@city.example.org",
    );
    expect(screen.getByText("filled by the pipeline stops-import")).toBeInTheDocument();
    expect(within(screen.getByTestId("catalog-offer")).getByText(
      "Anyone may use this data if they credit The city.",
    )).toBeInTheDocument();
  });

  it("says what is missing when the endpoint has no catalogue description", () => {
    show(undefined);
    expect(screen.getByText(/No catalogue description yet/)).toBeInTheDocument();
  });

  it("reads the raw record with the media type a harvester asks for, and says when it cannot", async () => {
    const fetch = vi
      .fn()
      .mockResolvedValueOnce(new Response("<a> a <b> .", { status: 200 }))
      .mockResolvedValueOnce(new Response("", { status: 404 }));
    vi.stubGlobal("fetch", fetch);
    const user = userEvent.setup();
    show(STORED);

    await user.click(screen.getByRole("button", { name: "DCAT-AP record (Turtle)" }));
    await waitFor(() => expect(screen.getByText("<a> a <b> .")).toBeInTheDocument());
    const asked = fetch.mock.calls[0][0] as Request;
    expect(asked.headers.get("Accept")).toBe("text/turtle");
    expect(asked.url).toBe(`${window.location.origin}/api/endpoint/${SLUG}/`);
    await user.keyboard("{Escape}");

    await user.click(screen.getByRole("button", { name: "Your access as ODRL" }));
    await waitFor(() => expect(screen.getByText(/did not answer/)).toBeInTheDocument());
    expect((fetch.mock.calls[1][0] as Request).headers.get("Accept")).toBe("application/odrl+json");
  });
});
