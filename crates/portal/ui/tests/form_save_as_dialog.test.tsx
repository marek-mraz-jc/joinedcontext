/**
 * T-1754: the SaveAsDialog against the UI contract (UI-04, UI-15, UI-16, UI-44, UI-48).
 *
 * Four things this file holds, each of which was wrong on main: the manifest being copied had one
 * state instead of three, so a slow GET showed a complete dialog and a failed one kept showing it
 * for ever; Check and Propose were hard-disabled out of the tab order with the only explanation in
 * a separate Alert tied to neither; the choice that decides whether a Context Space is copied,
 * mapped or referenced was a set of bare radios in hand-written labels; and the mapping Select had
 * no Field, no label and no reading of its own query, so an empty or failed list left Check closed
 * with nothing saying why.
 *
 * `checkForm` is T-1730's and joins this file with it.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import en from "../src/locales/en.json";
import type { Manifest } from "../src/api/manifest";
import {
  expectDenied,
  expectNoRawKeys,
  expectNoViolations,
  expectOpen,
  expectTabOrder,
  focusables,
} from "./checks";

vi.mock("../src/components/ChangeNotice", () => ({
  ChangeNotice: ({ change }: { change: { metadata: { name: string } } }) => (
    <p>{change.metadata.name}</p>
  ),
}));

let mayPropose = true;
vi.mock("../src/api/permissions", () => ({
  usePermissions: () => ({ can: () => mayPropose }),
}));

const { SaveAsDialog } = await import("../src/components/SaveAsDialog");

const API = "joinedcontext.com/v1alpha1";
const SOURCE = "helsinki";
const OTHER = "helsinki-mobility";
const NAME = "bikes-public";

const ENDPOINT = {
  apiVersion: API,
  kind: "Endpoint",
  metadata: { name: NAME, namespace: SOURCE },
  spec: { contextSpaceRef: SOURCE, slug: "scsd2eehkx42n53z2zyd6vshfh7s7irf", audience: "public" },
  status: { phase: "Live" },
} as unknown as Manifest;

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": status >= 400 ? "application/problem+json" : "application/json" },
  });

const list = (items: unknown[]) => json({ apiVersion: API, kind: "List", metadata: {}, items });

interface Answers {
  /** What a GET of the endpoint being copied answers; each call may answer differently. */
  endpoint?: (attempt: number) => Response | Promise<Response>;
  /** What a GET of the target project's spaces answers. */
  spaces?: () => Response | Promise<Response>;
}

let answers: Answers;
let endpointReads: number;

beforeEach(async () => {
  await i18n.changeLanguage("en");
  mayPropose = true;
  endpointReads = 0;
  answers = {};
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request =
        input instanceof Request ? input : new Request(new URL(String(input), "http://localhost"), init);
      const url = new URL(request.url);
      if (url.pathname === "/api/v1/projects") {
        return list([{ name: SOURCE }, { name: OTHER }]);
      }
      if (url.pathname.endsWith(`/endpoints/${NAME}`)) {
        endpointReads += 1;
        return answers.endpoint ? await answers.endpoint(endpointReads) : json(ENDPOINT);
      }
      if (url.pathname === `/api/v1/projects/${OTHER}/spaces`) {
        return answers.spaces
          ? await answers.spaces()
          : list([{ apiVersion: API, kind: "ContextSpace", metadata: { name: "mobility", namespace: OTHER } }]);
      }
      return list([]);
    }),
  );
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

function show() {
  return render(
    <QueryClientProvider
      client={new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } })}
    >
      <I18nextProvider i18n={i18n}>
        <SaveAsDialog
          target={{ project: SOURCE, kind: "Endpoint", plural: "endpoints", name: NAME }}
          open
          onOpenChange={() => undefined}
        />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

const check = () => screen.getByRole("button", { name: en.saveAs.check });
const propose = () => screen.getByRole("button", { name: en.saveAs.propose });

/** Move the dialog to the other project and choose the mapping outcome. */
async function mapIntoOther() {
  await screen.findByRole("option", { name: OTHER });
  await userEvent.selectOptions(screen.getByLabelText(en.saveAs.project), OTHER);
  await userEvent.click(await screen.findByLabelText(en.saveAs.choice.map));
}

describe("the manifest being copied has three states (UI-15)", () => {
  it("the_wait_is_announced_and_the_buttons_say_what_is_being_waited_for", async () => {
    answers.endpoint = () => new Promise<Response>(() => {});
    show();
    const waiting = await screen.findByRole("status");
    expect(waiting).toHaveAttribute("aria-busy", "true");
    expectDenied(check(), `Reading ${NAME}…`);
    expectDenied(propose(), `Reading ${NAME}…`);
  });

  it("a_manifest_that_cannot_be_read_says_why_and_offers_a_retry", async () => {
    answers.endpoint = () => json({ title: "Gone", status: 503, detail: "The store is away." }, 503);
    show();
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("The store is away.");
    expect(within(alert).getByRole("button", { name: en.app.error.retry })).toBeInTheDocument();
    expectDenied(check(), `${NAME} could not be read.`);
  });

  it("the_retry_reads_it_again_and_check_opens", async () => {
    answers.endpoint = (attempt) =>
      attempt === 1 ? json({ title: "Gone", status: 503, detail: "Away." }, 503) : json(ENDPOINT);
    show();
    const alert = await screen.findByRole("alert");
    await userEvent.click(within(alert).getByRole("button", { name: en.app.error.retry }));
    await waitFor(() => expectOpen(check()));
    expect(endpointReads).toBe(2);
  });

  it("a_check_is_never_sent_while_the_manifest_is_missing", async () => {
    answers.endpoint = () => new Promise<Response>(() => {});
    show();
    await userEvent.click(check());
    await userEvent.click(propose());
    expect(endpointReads).toBe(1);
  });
});

describe("the refusals are on the controls, not only in a notice (UI-44)", () => {
  it("a_role_that_may_not_propose_there_keeps_both_buttons_reachable_with_the_reason", async () => {
    mayPropose = false;
    show();
    await screen.findByLabelText(en.saveAs.name);
    const reason = `You may not propose this kind in ${SOURCE}.`;
    await waitFor(() => expectDenied(check(), reason));
    expectDenied(propose(), reason);
    expect(focusables(await screen.findByRole("dialog"))).toContain(check());
  });

  it("propose_says_the_copy_has_to_be_checked_first", async () => {
    show();
    await screen.findByLabelText(en.saveAs.name);
    await waitFor(() => expectOpen(check()));
    expectDenied(propose(), en.saveAs.checkFirst);
  });

  it("choosing_to_map_without_a_space_says_so_on_the_button", async () => {
    show();
    await mapIntoOther();
    await waitFor(() => expectDenied(check(), en.saveAs.chooseSpace));
  });
});

describe("the space of a copy into another project (T-1441)", () => {
  it("the_three_outcomes_are_one_radio_group_with_one_tab_stop", async () => {
    show();
    await screen.findByRole("option", { name: OTHER });
    await userEvent.selectOptions(screen.getByLabelText(en.saveAs.project), OTHER);
    // `radiogroup`, not `group`: since T-1254 the shared `RadioGroup` spells the role out, so a
    // screen reader reads "radio group, 1 of 3" instead of announcing a plain fieldset (UI-15).
    const group = await screen.findByRole("radiogroup", { name: /The space/ });
    const radios = within(group).getAllByRole("radio");
    expect(radios).toHaveLength(3);
    // One name on all three is what makes the arrow keys walk them and Tab step over the group.
    expect(new Set(radios.map((radio) => radio.getAttribute("name"))).size).toBe(1);
  });

  it("the_arrow_keys_move_between_the_outcomes", async () => {
    show();
    await screen.findByRole("option", { name: OTHER });
    await userEvent.selectOptions(screen.getByLabelText(en.saveAs.project), OTHER);
    const copy = await screen.findByLabelText(en.saveAs.choice.copy);
    copy.focus();
    await userEvent.keyboard("{ArrowDown}");
    expect(screen.getByLabelText(en.saveAs.choice.map)).toBeChecked();
  });

  it("the_mapping_box_has_a_label_of_its_own", async () => {
    show();
    await mapIntoOther();
    expect(await screen.findByLabelText(en.saveAs.mapTo)).toBeInstanceOf(HTMLSelectElement);
  });

  it("a_target_project_with_no_space_says_so_beside_the_box", async () => {
    answers.spaces = () => list([]);
    show();
    await mapIntoOther();
    const box = await screen.findByLabelText(en.saveAs.mapTo);
    await waitFor(() => expect(box).toHaveAttribute("aria-invalid", "true"));
    expect(box).toHaveAccessibleDescription(
      new RegExp(`${OTHER} has no space to map to yet\\.`),
    );
  });

  it("a_space_list_that_cannot_be_read_says_so_beside_the_box", async () => {
    answers.spaces = () => json({ title: "Forbidden", status: 403, detail: "No." }, 403);
    show();
    await mapIntoOther();
    const box = await screen.findByLabelText(en.saveAs.mapTo);
    await waitFor(() => expect(box).toHaveAttribute("aria-invalid", "true"));
    expect(box).toHaveAccessibleDescription(
      new RegExp(`The spaces of ${OTHER} could not be read\\.`),
    );
  });
});

describe("the dialog meets the UI contract", () => {
  it("has_no_axe_violation_while_reading_when_it_failed_and_with_the_outcomes_open", async () => {
    answers.endpoint = () => new Promise<Response>(() => {});
    const waiting = show();
    await screen.findByRole("status");
    await expectNoViolations(await screen.findByRole("dialog"));
    waiting.unmount();

    answers.endpoint = () => json({ title: "Gone", status: 503, detail: "Away." }, 503);
    const failed = show();
    await screen.findByRole("alert");
    await expectNoViolations(await screen.findByRole("dialog"));
    failed.unmount();

    answers.endpoint = undefined;
    show();
    await mapIntoOther();
    await expectNoViolations(await screen.findByRole("dialog"));
  });

  it("every_control_is_reached_by_keyboard_in_dom_order", async () => {
    const user = userEvent.setup();
    show();
    await screen.findByLabelText(en.saveAs.name);
    await expectTabOrder(user, await screen.findByRole("dialog"));
  });

  it.each(SUPPORTED_LOCALES)("says_everything_in_%s_with_no_raw_key", async (locale) => {
    await i18n.changeLanguage(locale);
    const { unmount } = show();
    await screen.findByLabelText(i18n.t("saveAs.name"));
    // The project names in the Select are the organization's own, not a bundle's.
    expectNoRawKeys(await screen.findByRole("dialog"), ["option"]);
    unmount();
  });
});
