/**
 * T-4595 (EP-127, EP-80, UI-02): Project settings → General names the body that publishes the
 * project's catalogue. An empty group writes no `spec.catalog`, a half-filled one is refused
 * before it is proposed (jc-core refuses a publisher without its registry IRI again), and a saved
 * one is the catalogue address the gateway serves.
 */
import { cleanup, screen } from "@testing-library/react";
import validator from "@rjsf/validator-ajv8";
import { parse as parseYaml } from "yaml";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import cs from "../src/locales/cs.json";
import de from "../src/locales/de.json";
import en from "../src/locales/en.json";
import sk from "../src/locales/sk.json";
import projectForm from "../src/schemas/forms/project.uischema.yaml?raw";
import { projectSchema } from "../src/schemas/kinds";
import { fromProject, ProjectSettingsPage, toProject } from "../src/pages/projectSettings/ProjectSettingsPage";
import { json, list, renderPage } from "./page_contract";

const PROJECT = "banskabystrica";
const IRI = "https://data.gov.sk/id/legal-subject/00313271";

const STORED = {
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Project",
  metadata: { name: PROJECT, namespace: "org" },
  spec: { organizationRef: "bb" },
};

const t = (key: string) => key;

/** The errors the form's own validator gives the whole form. */
function errors(form: Record<string, unknown>): string[] {
  return validator
    .validateFormData(form, projectSchema(t) as never)
    .errors.map((error) => `${error.property} ${error.name}`);
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cleanup();
});

describe("the catalogue group of a project (T-4595)", () => {
  it("left empty, is valid and writes no spec.catalog, and clearing it removes a saved one", () => {
    expect(errors(fromProject(STORED))).toEqual([]);
    expect((toProject(fromProject(STORED), STORED) as typeof STORED).spec).toEqual({ organizationRef: "bb" });

    const saved = { ...STORED, spec: { ...STORED.spec, catalog: { publisher: { name: { sk: "Mesto" }, uri: IRI } } } };
    const cleared = toProject(
      { catalog: { publisherName: { sk: "  " }, publisherUri: "", contactName: undefined } },
      saved,
    ) as typeof STORED;
    expect(cleared.spec).toEqual({ organizationRef: "bb" });
  });

  it("refuses a publisher without its registry IRI, a contact desk without its address, and a non-IRI", () => {
    expect(errors({ catalog: { publisherName: { sk: "Mesto Banská Bystrica" } } })).toContain(
      ".catalog.publisherUri required",
    );
    expect(errors({ catalog: { publisherUri: IRI } })).toContain(".catalog.publisherName required");
    expect(
      errors({ catalog: { publisherName: { sk: "Mesto" }, publisherUri: IRI, contactName: "Oddelenie otvorených dát" } }),
    ).toContain(".catalog.contactEmail dependencies");
    expect(errors({ catalog: { contactEmail: "opendata@banskabystrica.sk" } })).toEqual(
      expect.arrayContaining([".catalog.publisherUri required", ".catalog.contactName dependencies"]),
    );
    expect(errors({ catalog: { publisherName: { sk: "Mesto" }, publisherUri: "00313271" } })).toContain(
      ".catalog.publisherUri pattern",
    );
    expect(
      errors({ catalog: { publisherName: { sk: "Mesto" }, publisherUri: IRI, contactName: "Desk", contactEmail: "not-an-address" } }),
    ).toContain(".catalog.contactEmail format");
  });

  it("saves the filled group as jc-core's spec.catalog, trimmed, and reads it back", () => {
    const form = {
      catalog: {
        publisherName: { sk: " Mesto Banská Bystrica ", en: "City of Banská Bystrica", cs: undefined },
        publisherUri: IRI,
        contactName: "Oddelenie otvorených dát",
        contactEmail: "opendata@banskabystrica.sk",
      },
    };
    expect(errors(form)).toEqual([]);
    const proposed = toProject(form, STORED) as { spec: Record<string, unknown> };
    const catalog = {
      publisher: { name: { sk: "Mesto Banská Bystrica", en: "City of Banská Bystrica" }, uri: IRI },
      contactPoint: { name: "Oddelenie otvorených dát", email: "opendata@banskabystrica.sk" },
    };
    expect(proposed.spec).toEqual({ organizationRef: "bb", catalog });
    expect(fromProject(proposed).catalog).toEqual({
      publisherName: catalog.publisher.name,
      publisherUri: IRI,
      contactName: catalog.contactPoint.name,
      contactEmail: catalog.contactPoint.email,
    });
  });

  it("shows the catalogue address once a publisher is saved", async () => {
    const saved = { ...STORED, spec: { ...STORED.spec, catalog: { publisher: { name: { sk: "Mesto" }, uri: IRI } } } };
    renderPage(<ProjectSettingsPage project={PROJECT} tab="general" />, {
      path: `/projects/${PROJECT}/settings/general`,
      answer: (url) => {
        if (url.pathname.endsWith("/permissions/me")) return json({ project: PROJECT, bootstrap: true, grants: [] });
        if (url.pathname === `/api/v1/projects/org/projects/${PROJECT}`) return json(saved);
        if (url.pathname === "/api/v1/projects/org/organizations") return json(list([]));
        return undefined;
      },
    });
    const address = `${window.location.origin}/catalog/${PROJECT}.jsonld`;
    expect(await screen.findByRole("link", { name: address })).toHaveAttribute("href", address);
  });

  it("is a group of the shipped form, with its words in all four languages", () => {
    const form = parseYaml(projectForm) as {
      spec: { order: string[]; groups: { fields: string[]; title: Record<string, string> }[]; fields: Record<string, { help: Record<string, string> }> };
    };
    expect(form.spec.order).toContain("catalog");
    const group = form.spec.groups.find((candidate) => candidate.fields.includes("catalog"));
    expect(Object.keys(group?.title ?? {}).sort()).toEqual(["cs", "de", "en", "sk"]);
    const leaves = ["publisherName.sk", "publisherName.cs", "publisherName.en", "publisherName.de", "publisherUri", "contactName", "contactEmail"];
    for (const leaf of leaves) {
      expect(Object.keys(form.spec.fields[`catalog.${leaf}`].help).sort()).toEqual(["cs", "de", "en", "sk"]);
    }
    for (const field of ["publisherName", "publisherUri", "contactName", "contactEmail"]) {
      for (const words of [en, sk, cs, de]) {
        expect(words.projectSettings.field[field as keyof typeof en.projectSettings.field]).toBeTruthy();
      }
    }
    expect(form.spec.fields["catalog.publisherUri"].help.sk).toContain("https://data.gov.sk/id/legal-subject/{IČO}");
    for (const words of [en, sk, cs, de]) {
      expect(words.projectSettings.general.catalogUrl).toBeTruthy();
      expect(words.projectSettings.general.noCatalog).toBeTruthy();
    }
  });
});
