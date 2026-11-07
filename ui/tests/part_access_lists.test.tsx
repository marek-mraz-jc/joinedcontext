/**
 * T-1827, T-1829: the groups and the service accounts of the Access page (UI-01, UI-15, UI-16,
 * UI-44, PF-50). Since T-2606 the accounts are on Project settings → Service accounts and the
 * groups on Organization → Groups; the cases follow them there.
 *
 * Neither file was named by a test. Both carried a defect the checklist looks for: a refusal
 * that could not be read — the Propose button was hard-disabled with its reason wired by
 * `aria-describedby`, which is out of the tab order, so nobody could reach it to be told why. (The
 * key list's cases went with the keys, T-4616.)
 */
import { screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import {
  expectAxeClean,
  jsonResponse,
  list,
  LOCALES,
  renderRoute,
  VIEWER,
} from "./pageHarness";
import { findFormPage } from "./formPage";

const ACCOUNTS = "/projects/helsinki/settings/service-accounts";
const GROUPS = "/organization/groups";

const serviceAccount = (name: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "ServiceAccount",
  metadata: { name, namespace: "helsinki" },
  spec: { credentials: [{ name: "default" }] },
  status: { phase: "Live" },
});

const group = (name: string) => ({
  apiVersion: "joinedcontext.com/v1alpha1",
  kind: "Group",
  metadata: { name, namespace: "org" },
  spec: { members: ["jana.kovacova@banskabystrica.sk"] },
  status: { phase: "Live" },
});

const answering =
  (over: (path: string) => Response | undefined = () => undefined) =>
  (path: string) => {
    const own = over(path);
    if (own) return own;
    if (path.endsWith("/serviceaccounts")) return jsonResponse(list([serviceAccount("harvester")]));
    if (path.endsWith("/groups")) return jsonResponse(list([group("stewards")]));
    return undefined;
  };

describe("the service accounts and the groups", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("opens_with_its_lists_and_no_axe_violation", async () => {
    const { container } = await renderRoute({ path: ACCOUNTS, answer: answering(), identity: VIEWER });
    await screen.findByText("harvester");
    await expectAxeClean(container);
  });

  it("the_new_group_form_opens_on_its_fields_and_not_on_a_manifest", async () => {
    // It used to open on a textarea holding a nameless example, with Propose disabled until the
    // example was named. The fields replace both: the name is a field the schema validates, and
    // the manifest is one view away rather than the first thing a person meets (T-2400).
    await renderRoute({ path: GROUPS, answer: answering() });

    await userEvent.click(await screen.findByRole("button", { name: en.access.groups.new }));
    const dialog = await findFormPage();
    expect(
      within(dialog).getByLabelText(new RegExp(en.access.groups.field.name)),
    ).toBeInTheDocument();
    expect(
      within(dialog).getByRole("button", { name: en.access.groups.propose }),
    ).toBeInTheDocument();
  });

  it("says_everything_it_says_in_all_four_languages", async () => {
    for (const locale of LOCALES) {
      const { unmount } = await renderRoute({ path: GROUPS, locale, answer: answering() });
      expect(
        await screen.findByRole("button", { name: i18n.t("access.groups.new") }),
      ).toBeInTheDocument();
      if (locale !== "en") {
        expect(screen.queryByRole("button", { name: en.access.groups.new })).toBeNull();
      }
      unmount();
      vi.restoreAllMocks();
    }
  });

  it("a_service_account_name_out_of_the_api_is_rendered_as_text", async () => {
    const { container } = await renderRoute({
      path: ACCOUNTS,
      answer: answering((path) =>
        path.endsWith("/serviceaccounts")
          ? jsonResponse(list([serviceAccount("<img src=x onerror=alert(1)>")]))
          : undefined,
      ),
    });
    expect(await screen.findByText("<img src=x onerror=alert(1)>")).toBeInTheDocument();
    expect(container.querySelector("img")).toBeNull();
  });
});
