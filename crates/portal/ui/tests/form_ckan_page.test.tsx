/**
 * T-1765: the catalogue form of the CKAN page against the UI contract (UI-04, UI-15, UI-16, UI-44, UI-48).
 *
 * Four defects on main, all held here. The form emptied itself the moment submit fired, so a
 * steward who filled in name, address, organization and secret name lost all four to a 403 or a
 * name that was taken. A failed `/ckan/status` was not read at all, so the page stated as a fact
 * that the project has no catalogue and publishes nothing — and the obvious next move is to
 * propose a catalogue that is already there. The address was checked only by `type="url"`, which
 * accepts `javascript:` and `file:`. And "nothing here" was a bare paragraph where `EmptyState`
 * carries the reason and what to do about it.
 *
 * `checkForm` (T-1730) carries the shared rules. The cases below are this form's own, and one of
 * them is the security property of this page: the API token is never a field, only the name of a
 * secret (MF-24, EP-67).
 */
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { byLabel, checkForm, type FormSpec } from "./formContract";
import { expectDenied, expectNoRawKeys } from "./checks";
import { json, LOCALES, problem, renderPage } from "./page_contract";

vi.mock("../src/components/ChangeNotice", () => ({
  ChangeNotice: ({ change }: { change: { metadata: { name: string } } }) => <p>{change.metadata.name}</p>,
}));

let mayPropose = true;
vi.mock("../src/api/permissions", () => ({
  usePermissions: () => ({ data: { grants: [] }, can: () => mayPropose }),
}));

const { CkanPage } = await import("../src/pages/ckan/CkanPage");

const API = "joinedcontext.com/v1alpha1";
const PROJECT = "banskabystrica";
const STATUS_PATH = `/api/v1/projects/${PROJECT}/ckan/status`;
const INSTANCES = `/api/v1/projects/${PROJECT}/ckaninstances`;
const REFUSAL = "A catalogue named opendata-bb is already registered in this project.";

const STATUS = {
  instances: [
    {
      name: "open-data",
      url: "https://data.banskabystrica.sk",
      organizationDefault: "mesto-banska-bystrica",
      apiTokenRef: "ckan-open-data",
    },
  ],
  publications: [],
};

/** The status read and the dry run: everything the page asks for before it proposes. */
function reads(over: { status?: () => Response } = {}) {
  return (url: URL) => {
    if (url.pathname === STATUS_PATH) return over.status?.() ?? json(STATUS);
    // The catalogue's organizations, which the Default organization picker offers (T-4948).
    if (url.pathname === "/api/v1/catalogue") {
      const none: unknown[] = [];
      return json({
        total: 0, page: 1, pageSize: 20, datasets: [], unavailable: [],
        facets: { publisher: [{ value: "mesto-banska-bystrica", label: "Mesto Banská Bystrica", count: 3 }],
                  theme: none, format: none, licence: none, spatial: none, year: none, keyword: none },
      });
    }
    if (url.pathname === INSTANCES && url.searchParams.get("dryRun") === "All") {
      return json({ valid: true, verdict: { ok: true, findings: [] } });
    }
    return undefined;
  };
}

const spec: FormSpec = {
  fields: [
    { id: "ckan-instance-name", label: /^Name/, value: "opendata-bb", required: true },
    {
      id: "ckan-instance-url",
      label: /^URL/,
      value: "https://opendata.example.sk",
      required: true,
      // What `type="url"` lets through and this form refuses itself.
      browser: { refuses: "javascript:alert(1)", accepts: "https://opendata.example.sk" },
    },
    { id: "ckan-instance-secret", label: /API token secret/, value: "ckan-api-token", required: true },
  ],
  submit: /Propose catalogue/,
  submitPath: INSTANCES,
  refusal: REFUSAL,
  answer: reads(),
  path: `/projects/${PROJECT}`,
};

const page = () => <CkanPage project={PROJECT} />;

beforeEach(async () => {
  mayPropose = true;
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the catalogue form (T-1765)", () => {
  it("offers the catalogue's organizations and takes a new one typed, lower case (T-4948)", async () => {
    const user = userEvent.setup();
    renderPage(page(), { path: `/projects/${PROJECT}`, answer: reads() });
    const org = await screen.findByLabelText(/Default organization/);
    expect(org).toHaveAccessibleDescription(/organization\/<name>/);
    await user.type(org, "Mesto-BB");
    await user.click(await screen.findByRole("option", { name: /New: “Mesto-BB”/ }));
    expect((org as HTMLInputElement).value).toBe("mesto-bb");
  });

  it("meets_the_form_contract", async () => {
    await checkForm(page, spec);
  });

  it("proposes_the_catalogue_with_the_secret_named_and_never_a_token", async () => {
    const bodies: Record<string, unknown>[] = [];
    const user = userEvent.setup();
    renderPage(page(), {
      path: `/projects/${PROJECT}`,
      answer: async (url, request) => {
        const own = reads()(url);
        if (own) return own;
        if (url.pathname === INSTANCES) {
          bodies.push((await request.json()) as Record<string, unknown>);
          return json({ apiVersion: API, kind: "Change", metadata: { name: "chg-9" } }, 201);
        }
        return undefined;
      },
    });
    await user.type(await screen.findByLabelText(/^Name/), "opendata-bb");
    await user.clear(byLabel(/^URL/));
    await user.type(byLabel(/^URL/), "https://opendata.example.sk");
    await user.click(byLabel(/Default organization/));
    await user.click(await screen.findByRole("option", { name: /Mesto Banská Bystrica/ }));
    await user.type(byLabel(/API token secret/), "ckan-api-token");
    await user.click(screen.getByRole("button", { name: /Propose catalogue/ }));

    await waitFor(() => expect(bodies).toHaveLength(1));
    expect(bodies[0]).toMatchObject({
      kind: "CkanInstance",
      spec: {
        url: "https://opendata.example.sk",
        organizationDefault: "mesto-banska-bystrica",
        apiTokenRef: { name: "ckan-api-token", key: "apiToken" },
      },
    });
    // The token itself has no field to come from: only its secret's name travels (MF-24, EP-67).
    expect(JSON.stringify(bodies[0])).not.toMatch(/apiToken"\s*:\s*"[^"]/);
    expect(document.querySelectorAll('input[type="password"]')).toHaveLength(0);
  });

  it("clears_the_form_only_once_the_change_came_back", async () => {
    const user = userEvent.setup();
    renderPage(page(), {
      path: `/projects/${PROJECT}`,
      answer: (url) => {
        const own = reads()(url);
        if (own) return own;
        if (url.pathname === INSTANCES) {
          return json({ apiVersion: API, kind: "Change", metadata: { name: "chg-9" } }, 201);
        }
        return undefined;
      },
    });
    await user.type(await screen.findByLabelText(/^Name/), "opendata-bb");
    await user.clear(byLabel(/^URL/));
    await user.type(byLabel(/^URL/), "https://opendata.example.sk");
    await user.type(byLabel(/API token secret/), "ckan-api-token");
    await user.click(screen.getByRole("button", { name: /Propose catalogue/ }));

    await screen.findByText("chg-9");
    await waitFor(() => expect((screen.getByLabelText(/^Name/) as HTMLInputElement).value).toBe(""));
  });

  it("keeps_every_word_when_the_server_refuses", async () => {
    const user = userEvent.setup();
    renderPage(page(), {
      path: `/projects/${PROJECT}`,
      answer: (url) => {
        const own = reads()(url);
        if (own) return own;
        if (url.pathname === INSTANCES) return problem(409, REFUSAL);
        return undefined;
      },
    });
    await user.type(await screen.findByLabelText(/^Name/), "opendata-bb");
    await user.clear(byLabel(/^URL/));
    await user.type(byLabel(/^URL/), "https://opendata.example.sk");
    await user.click(byLabel(/Default organization/));
    await user.click(await screen.findByRole("option", { name: /Mesto Banská Bystrica/ }));
    await user.type(byLabel(/API token secret/), "ckan-api-token");
    await user.click(screen.getByRole("button", { name: /Propose catalogue/ }));

    expect(await screen.findByText(REFUSAL)).toBeInTheDocument();
    expect((screen.getByLabelText(/^Name/) as HTMLInputElement).value).toBe("opendata-bb");
    expect((byLabel(/^URL/) as HTMLInputElement).value).toBe("https://opendata.example.sk");
    expect((byLabel(/Default organization/) as HTMLInputElement).value).toBe("Mesto Banská Bystrica");
    expect((byLabel(/API token secret/) as HTMLInputElement).value).toBe("ckan-api-token");
  });

  it("refuses_a_javascript_address_itself_and_sends_nothing", async () => {
    const sent: string[] = [];
    const user = userEvent.setup();
    renderPage(page(), {
      path: `/projects/${PROJECT}`,
      answer: (url) => {
        const own = reads()(url);
        if (own) return own;
        if (url.pathname === INSTANCES) {
          sent.push("proposed");
          return json({ apiVersion: API, kind: "Change", metadata: { name: "chg-9" } }, 201);
        }
        return undefined;
      },
    });
    await user.type(await screen.findByLabelText(/^Name/), "opendata-bb");
    await user.clear(byLabel(/^URL/));
    await user.type(byLabel(/^URL/), "javascript:alert(1)");
    await user.type(byLabel(/API token secret/), "ckan-api-token");
    await user.click(screen.getByRole("button", { name: /Propose catalogue/ }));

    expect(await screen.findByText(/starts with http:\/\/ or https:\/\//)).toBeInTheDocument();
    expect(sent, "a javascript: address was proposed").toEqual([]);
    expect(byLabel(/^URL/)).toHaveAttribute("aria-invalid", "true");
  });

  it.each([
    ["a JWT", "eyJhbGciOiJIUzI1NiJ9.eyJqdGkiOiJ4In0.c2ln", /looks like the API token itself/],
    ["a legacy API key", "3f2b8c1e-9a4d-4e2f-8b1c-7d6e5f4a3b2c", /looks like the API token itself/],
    ["an upper-case name", "CKAN_Token", /lowercase letters, digits, dashes and dots/],
  ])("refuses_%s_in_the_secret_name_and_sends_nothing (T-3215)", async (_what, typed, sentence) => {
    const sent: string[] = [];
    const user = userEvent.setup();
    renderPage(page(), {
      path: `/projects/${PROJECT}`,
      answer: (url) => {
        const own = reads()(url);
        if (own) return own;
        if (url.pathname === INSTANCES) sent.push("proposed");
        return undefined;
      },
    });
    await user.type(await screen.findByLabelText(/^Name/), "opendata-bb");
    await user.type(byLabel(/^URL/), "https://opendata.example.sk");
    await user.type(byLabel(/API token secret/), typed);
    await user.click(screen.getByRole("button", { name: /Propose catalogue/ }));

    expect(await screen.findByText(sentence)).toBeInTheDocument();
    expect(sent, "a token or an invalid secret name was proposed").toEqual([]);
    expect(byLabel(/API token secret/)).toHaveAttribute("aria-invalid", "true");
    expect(byLabel(/API token secret/)).toHaveFocus();
  });

  it("names_the_key_inside_the_secret_and_sends_what_was_typed (T-3215)", async () => {
    const bodies: unknown[] = [];
    const user = userEvent.setup();
    renderPage(page(), {
      path: `/projects/${PROJECT}`,
      answer: async (url, request) => {
        const own = reads()(url);
        if (own) return own;
        if (url.pathname === INSTANCES) {
          bodies.push(await request.clone().json());
          return json({ apiVersion: API, kind: "Change", metadata: { name: "chg-9" } }, 201);
        }
        return undefined;
      },
    });
    await user.type(await screen.findByLabelText(/^Name/), "opendata-bb");
    await user.type(byLabel(/^URL/), "https://opendata.example.sk");
    await user.type(byLabel(/API token secret/), "ckan-api-token");
    const key = screen.getByLabelText(/Key inside the secret/);
    expect(key).toHaveValue("apiToken");
    await user.clear(key);
    await user.type(key, "token");
    await user.click(screen.getByRole("button", { name: /Propose catalogue/ }));

    await waitFor(() => expect(bodies.length).toBeGreaterThan(0));
    expect(JSON.stringify(bodies.at(-1))).toContain('"apiTokenRef":{"name":"ckan-api-token","key":"token"}');
  });

  it("offers_the_publish_flow_once_a_catalogue_exists_and_says_why_not_before (T-3215)", async () => {
    const first = renderPage(page(), {
      path: `/projects/${PROJECT}`,
      answer: reads({ status: () => json({ instances: [], publications: [] }) }),
    });
    expectDenied(
      await screen.findByRole("button", { name: i18n.t("catalogue.publish.open") }),
      i18n.t("ckan.publications.needsCatalogue"),
    );
    first.unmount();

    const user = userEvent.setup();
    renderPage(page(), { path: `/projects/${PROJECT}`, answer: reads() });
    const open = await screen.findByRole("button", { name: i18n.t("catalogue.publish.open") });
    await waitFor(() => expect(open).not.toHaveAttribute("aria-disabled"));
    await user.click(open);
    expect(await screen.findByRole("dialog", { name: i18n.t("catalogue.publish.title") })).toBeInTheDocument();
  });

  it("says_the_status_could_not_be_read_instead_of_claiming_there_is_nothing", async () => {
    renderPage(page(), {
      path: `/projects/${PROJECT}`,
      answer: reads({ status: () => problem(503, "The CKAN status is not available right now.") }),
    });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("The CKAN status is not available right now.");
    expect(alert).toHaveTextContent(/may be incomplete/);
    expect(
      screen.queryByText(i18n.t("ckan.instances.empty")),
      "a failed read claimed the project has no catalogue",
    ).toBeNull();
    expect(screen.queryByText(i18n.t("ckan.publications.empty"))).toBeNull();
  });

  it("says_what_would_fill_each_list_when_both_are_empty", async () => {
    renderPage(page(), {
      path: `/projects/${PROJECT}`,
      answer: reads({ status: () => json({ instances: [], publications: [] }) }),
    });
    expect(await screen.findByText(i18n.t("ckan.instances.empty"))).toBeInTheDocument();
    expect(screen.getByText(i18n.t("ckan.instances.emptyHint"))).toBeInTheDocument();
    expect(screen.getByText(i18n.t("ckan.publications.empty"))).toBeInTheDocument();
    expect(screen.getByText(i18n.t("ckan.publications.emptyHint"))).toBeInTheDocument();
  });

  it("shows_a_viewer_the_refusal_sentence_and_the_catalogues_but_no_form (T-4997)", async () => {
    mayPropose = false;
    renderPage(page(), { path: `/projects/${PROJECT}`, answer: reads() });
    expect(await screen.findByText(i18n.t("form.mayNotCreate"))).toBeInTheDocument();
    // The read parts stay: the catalogue table and who manages it.
    expect(await screen.findByRole("table", { name: i18n.t("ckan.instances.title") })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Propose catalogue/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: /API token secret/ })).not.toBeInTheDocument();
  });

  it("shows_a_steward_the_form (T-4997)", async () => {
    renderPage(page(), { path: `/projects/${PROJECT}`, answer: reads() });
    expect(await screen.findByRole("button", { name: /Propose catalogue/ })).toBeInTheDocument();
    expect(screen.queryByText(i18n.t("form.mayNotCreate"))).not.toBeInTheDocument();
  });

  it.each(LOCALES)("says_everything_in_%s_with_no_raw_key", async (locale) => {
    await i18n.changeLanguage(locale);
    const view = renderPage(page(), { path: `/projects/${PROJECT}`, answer: reads() });
    await screen.findByRole("button", { name: i18n.t("ckan.instances.propose") });
    expectNoRawKeys(view.container, ["option"]);
  });
});
