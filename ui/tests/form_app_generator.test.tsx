/**
 * T-1760: the application generator against the UI contract (UI-04, UI-11, UI-15, UI-16, UI-44).
 *
 * What this file holds, each of which was measured on the file before it: a failed
 * `/api/v1/blueprints` was rendered as "this deployment has no app builder", so a person whose
 * request 500ed was told to stop trying; the endpoint list had neither an error nor an empty
 * branch, so a failure, a slow network and a project with nothing published all looked like a
 * dropdown with one placeholder in it; the button that starts the run carried no permission, so
 * a viewer wrote a whole brief and met a raw 403; and the typed name went into `appName`, into
 * the router param and into the served URL with no check at all, although `slugOf` sanitises
 * only the derived one.
 *
 * `checkForm` (T-1730) carries the shared rules over the brief itself.
 */
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { checkForm, type FormSpec } from "./formContract";
import { expectDenied } from "./checks";
import { json, list, problem, renderPage } from "./page_contract";

let mayPropose = true;
vi.mock("../src/api/permissions", () => ({
  usePermissions: () => ({ can: () => mayPropose }),
}));
vi.mock("../src/components/ChangeNotice", () => ({
  ChangeNotice: ({ change }: { change: { metadata: { name: string } } }) => <p>{change.metadata.name}</p>,
}));

const { AppGenerator, APP_NAME, slugOf } = await import("../src/pages/apps/AppGenerator");

const API = "joinedcontext.com/v1alpha1";
const PROJECT = "banskabystrica";
const BLUEPRINTS = "/api/v1/blueprints";
const ENDPOINTS = `/api/v1/projects/${PROJECT}/endpoints`;
const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";

const BLUEPRINT = {
  apiVersion: API,
  kind: "AgentProfile",
  metadata: { name: "app-from-prompt" },
  spec: {},
};
const ENDPOINT = {
  apiVersion: API,
  kind: "Endpoint",
  metadata: { name: "ovzdusie-public", namespace: PROJECT },
  spec: { contextSpaceRef: "ovzdusie", slug: SLUG, audience: "public" },
  status: { phase: "Live" },
};

function reads(over: { blueprints?: () => Response; endpoints?: () => Response } = {}) {
  return (url: URL) => {
    if (url.pathname === BLUEPRINTS) return over.blueprints?.() ?? json(list([BLUEPRINT]));
    if (url.pathname === ENDPOINTS) return over.endpoints?.() ?? json(list([ENDPOINT]));
    if (url.pathname.includes("/schema/index.json")) return json({ models: [{ version: 1 }] });
    if (url.pathname.includes("/json-schema")) {
      return json({
        $defs: {
          AirQualityObserved: { title: "AirQualityObserved", properties: { pm10: {}, dateObserved: {} } },
        },
      });
    }
    return undefined;
  };
}

const generator = () => <AppGenerator project={PROJECT} />;

const spec: FormSpec = {
  fields: [
    { label: /What should the app do/i, value: "Show today's air quality" },
    { label: /^Endpoint/, value: "ovzdusie-public" },
  ],
  submit: /Start the build|Build it|Generate/i,
  submitPath: `/api/v1/projects/${PROJECT}/agent-runs`,
  path: `/projects/${PROJECT}`,
  answer: reads(),
  keysExclude: ["option"],
  // The form appears once the blueprint list has arrived; before that the page is one status
  // line, and every rule would read that instead of the form.
  open: async () => {
    await screen.findByLabelText(/What should the app do/i, { exact: false });
  },
};

beforeEach(async () => {
  mayPropose = true;
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the application generator (T-1760)", () => {
  it("meets_the_form_contract", async () => {
    await checkForm(generator, { ...spec, submit: new RegExp(i18n.t("apps.generate.submit")) });
  });

  it("says_the_builder_list_failed_instead_of_saying_there_is_no_builder", async () => {
    renderPage(generator(), {
      path: `/projects/${PROJECT}`,
      answer: reads({ blueprints: () => problem(500, "The blueprint list could not be read.") }),
    });
    expect(await screen.findByText(/The blueprint list could not be read\./)).toBeInTheDocument();
    expect(
      screen.queryByText(i18n.t("apps.generate.noBuilder")),
      "a failed read was reported as a deployment without an app builder",
    ).toBeNull();
    expect(screen.getByRole("button", { name: i18n.t("form.listRetry") })).toBeInTheDocument();
  });

  it("says_the_endpoint_list_failed_beside_the_picker_it_empties", async () => {
    renderPage(generator(), {
      path: `/projects/${PROJECT}`,
      answer: reads({ endpoints: () => problem(503, "The endpoint list is not answering.") }),
    });
    const picker = await screen.findByLabelText(i18n.t("apps.generate.endpoint"), { exact: false });
    await waitFor(() => expect(picker).toHaveAttribute("aria-invalid", "true"));
    expect(screen.getByText(/The endpoint list is not answering\./)).toBeInTheDocument();
  });

  it("says_a_project_with_nothing_published_has_nothing_to_read_from", async () => {
    renderPage(generator(), {
      path: `/projects/${PROJECT}`,
      answer: reads({ endpoints: () => json(list([])) }),
    });
    expect(await screen.findByText(i18n.t("apps.generate.noEndpoints"))).toBeInTheDocument();
  });

  it("refuses_the_build_with_its_reason_to_a_person_who_may_not_propose_an_app", async () => {
    mayPropose = false;
    renderPage(generator(), { path: `/projects/${PROJECT}`, answer: reads() });
    // Refused once the person's permissions are read, not before: the button exists earlier.
    const button = await screen.findByRole("button", { name: i18n.t("apps.generate.submit") });
    await waitFor(() => expectDenied(button, /propose/));
  });

  it("refuses_a_typed_name_that_could_not_be_a_url_segment", async () => {
    const user = userEvent.setup();
    const sent: string[] = [];
    renderPage(generator(), {
      path: `/projects/${PROJECT}`,
      answer: (url) => {
        const own = reads()(url);
        if (own) return own;
        if (url.pathname.endsWith("/agent-runs")) {
          sent.push("started");
          return json({ id: "run-1", appName: "x" }, 201);
        }
        return undefined;
      },
    });
    await user.type(
      await screen.findByLabelText(i18n.t("apps.generate.prompt"), { exact: false }),
      "Show today's air quality",
    );
    await user.selectOptions(
      screen.getByLabelText(i18n.t("apps.generate.endpoint"), { exact: false }),
      "ovzdusie-public",
    );
    await user.type(
      screen.getByLabelText(i18n.t("apps.generate.name"), { exact: false }),
      "Air Quality/../etc",
    );
    expect(await screen.findByText(i18n.t("apps.generate.nameInvalid"))).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: i18n.t("apps.generate.submit") }));
    expect(sent, "a name that is not a URL segment was sent as one").toEqual([]);
  });

  it("holds_the_typed_name_to_the_same_rule_the_derived_one_already_met", () => {
    // Every name `slugOf` produces passes the rule, so the check refuses nothing the form
    // itself would have proposed.
    for (const prompt of ["Show today's air quality", "", "Ukáž mi dnešné ovzdušie", "a".repeat(200)]) {
      expect(APP_NAME.test(slugOf(prompt, "ovzdusie-public")), prompt).toBe(true);
    }
    expect(APP_NAME.test("Air Quality")).toBe(false);
    expect(APP_NAME.test("air/quality")).toBe(false);
    expect(APP_NAME.test("-air")).toBe(false);
    expect(APP_NAME.test("a".repeat(41))).toBe(false);
  });
  it("names_what_the_grey_submit_still_waits_for_in_the_order_a_new_user_fills_it", async () => {
    // T-3214: a new user met a grey "Generate the app" with nothing saying why.
    const user = userEvent.setup();
    renderPage(generator(), { path: `/projects/${PROJECT}`, answer: reads() });
    const submit = await screen.findByRole("button", { name: i18n.t("apps.generate.submit") });
    expectDenied(submit, i18n.t("apps.generate.missing.endpoint"));
    await user.selectOptions(
      screen.getByLabelText(i18n.t("apps.generate.endpoint"), { exact: false }),
      "ovzdusie-public",
    );
    await waitFor(() => expectDenied(submit, i18n.t("apps.generate.missing.prompt")));
    await user.type(screen.getByLabelText(i18n.t("apps.generate.prompt"), { exact: false }), "Show air quality");
    await user.type(screen.getByLabelText(i18n.t("apps.generate.name"), { exact: false }), "Bad Name");
    await waitFor(() => expectDenied(submit, i18n.t("apps.generate.missing.name")));
  });
});
