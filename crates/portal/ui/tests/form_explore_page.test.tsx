/**
 * T-1768: what the explorer says when a list it needs does not arrive (UI-15, UI-16, UI-44).
 *
 * Three of this page's five defects are here: `/spaces` failing left the picker at "—" and the
 * page read like a project with no spaces; `/datamodels` failing removed every type and every
 * filter and left one grey line saying "no type", with no reason and no retry; and that same
 * line was a bare paragraph where `EmptyState` carries a status role, an icon and a sentence
 * about what to do. The other two — the delete button's reason on a hard-disabled control, and
 * the detail pane announcing nothing — are asserted in `explore_page.test.tsx`, which is where
 * the fixture that opens an entity already lives.
 *
 * No `checkForm` call: the explorer has no form that proposes. Its pickers change a view, its
 * filters are `EntityFilters` with a task and a test of their own (T-1758), and its one
 * proposing control is a confirmation dialog with no fields, so every rule of the contract would
 * skip or assert nothing. The rules that do apply to a page — landmarks, axe, the four locales —
 * are the page contract's and run below.
 */
import { screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { expectNoViolations, expectNoRawKeys } from "./checks";
import { json, list, LOCALES, problem, renderPage } from "./page_contract";

const { ExplorePage } = await import("../src/pages/explore/ExplorePage");

const API = "joinedcontext.com/v1alpha1";
const PROJECT = "helsinki";
const SPACES_PATH = `/api/v1/projects/${PROJECT}/spaces`;
const ENDPOINTS_PATH = `/api/v1/projects/${PROJECT}/endpoints`;
const MODELS_PATH = `/api/v1/projects/${PROJECT}/datamodels`;

const SPACE = {
  apiVersion: API,
  kind: "ContextSpace",
  metadata: { name: "helsinki", namespace: PROJECT },
  spec: { dataModelRef: "helsinki-mobility" },
};
const ENDPOINT = {
  apiVersion: API,
  kind: "Endpoint",
  metadata: { name: "helsinki-bikes", namespace: PROJECT },
  spec: { contextSpaceRef: "helsinki", slug: "scsd2eehkx42n53z2zyd6vshfh7s7irf", audience: "public" },
};
const MODEL = {
  apiVersion: API,
  kind: "DataModel",
  metadata: { name: "helsinki-mobility", namespace: PROJECT },
  spec: { classes: ["BikeHireDockingStation"], linkml: "models/helsinki-mobility.yaml" },
};

function reads(over: { spaces?: () => Response; models?: () => Response } = {}) {
  return (url: URL) => {
    if (url.pathname === SPACES_PATH) return over.spaces?.() ?? json(list([SPACE]));
    if (url.pathname === ENDPOINTS_PATH) return json(list([ENDPOINT]));
    if (url.pathname === MODELS_PATH) return over.models?.() ?? json(list([MODEL]));
    return undefined;
  };
}

const page = () => <ExplorePage project={PROJECT} initialSpace="helsinki" initialEndpoint="helsinki-bikes" />;

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the explorer when a list does not arrive (T-1768)", () => {
  it("says_the_space_list_failed_instead_of_showing_a_project_with_no_spaces", async () => {
    renderPage(page(), {
      path: `/projects/${PROJECT}`,
      answer: reads({ spaces: () => problem(503, "The space list is not answering.") }),
    });
    expect(await screen.findByText(/The space list is not answering\./)).toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: i18n.t("form.listRetry") }),
      "a failed list offers a way to try again",
    ).toBeInTheDocument();
    // The picker is still there: what failed is one list, not the page.
    expect(screen.getByLabelText(i18n.t("explore.space"))).toBeInTheDocument();
  });

  it("says_the_model_list_failed_instead_of_no_type", async () => {
    renderPage(page(), {
      path: `/projects/${PROJECT}`,
      answer: reads({ models: () => problem(500, "The model list could not be read.") }),
    });
    expect(await screen.findByText(/The model list could not be read\./)).toBeInTheDocument();
    expect(
      screen.queryByText(i18n.t("explore.noType")),
      "a failure was reported as an empty result",
    ).toBeNull();
  });

  it("says_what_would_fill_the_grid_when_no_type_is_chosen", async () => {
    const view = renderPage(page(), { path: `/projects/${PROJECT}`, answer: reads() });
    expect(await screen.findByText(i18n.t("explore.noType"))).toBeInTheDocument();
    expect(screen.getByText(i18n.t("explore.noTypeHint"))).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent(i18n.t("explore.noType"));
    await expectNoViolations(view.container);
  });

  it.each(LOCALES)("says_everything_in_%s_with_no_raw_key", async (locale) => {
    await i18n.changeLanguage(locale);
    const view = renderPage(page(), { path: `/projects/${PROJECT}`, answer: reads() });
    await screen.findByLabelText(i18n.t("explore.space"));
    await waitFor(() => expect(screen.getByText(i18n.t("explore.noType"))).toBeInTheDocument());
    expectNoRawKeys(view.container, ["option"]);
  });
});
