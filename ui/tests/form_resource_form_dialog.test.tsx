/**
 * T-1753: ResourceFormDialog against the UI contract (UI-04, UI-15, UI-16, UI-44, UI-48).
 *
 * Every manifest form of the Portal opens in this dialog, so what it does by hand it does on every
 * page. Read against the contract it had four such things: a hand-rolled tab list (two tab stops,
 * no arrow keys, an unnamed panel), a YAML-view Propose that was hard-disabled with its reason out
 * of reach, a bare checkbox with no focus ring for the advanced fields, and a verdict chip styled
 * by hand whose age was English inside a translated sentence. The cases below are those four,
 * then `checkForm` (T-1730) and the contract's axe, keyboard and locale runs.
 */
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider, useMutation } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n, { SUPPORTED_LOCALES } from "../src/i18n";
import en from "../src/locales/en.json";
import de from "../src/locales/de.json";
import { api, ApiError, unwrap } from "../src/api/client";
import { BrandingProvider } from "../src/branding";
import { ResourceFormDialog } from "../src/components/ResourceFormDialog";
import type { ManifestSource } from "../src/components/ResourceFormDialog";
import type { JsonSchema } from "../src/components/forms/types";
import { digestOf } from "../src/api/drafts";
import type { Verdict } from "../src/api/drafts";
import { DNS1123 } from "../src/schemas/kinds";
import {
  expectDenied,
  expectNoRawKeys,
  expectNoViolations,
  expectOpen,
  expectTabOrder,
} from "./checks";
import { checkForm } from "./formContract";
import { OTHER_BRAND } from "./page_contract";

// Monaco draws on a canvas and starts a worker, neither of which jsdom has; a textarea with the
// same contract stands in, so the YAML view's own work is what runs here.
vi.mock("../src/pages/models/MonacoSourceView", () => ({
  default: ({ value, onChange }: { value: string; onChange?: (value: string) => void }) => (
    <textarea aria-label="YAML" value={value} onChange={(event) => onChange?.(event.target.value)} />
  ),
}));

interface Space {
  name?: string;
  description?: string;
}

const SCHEMA: JsonSchema = {
  type: "object",
  required: ["name"],
  properties: {
    name: { type: "string", title: "Name", pattern: DNS1123 },
    description: { type: "string", title: "Description" },
  },
};

const SOURCE: ManifestSource<Space> = {
  toManifest: (form) => ({ apiVersion: "joinedcontext.com/v1alpha1", kind: "ContextSpace", metadata: { name: form.name ?? "" }, spec: { description: form.description ?? "" } }),
  fromManifest: (manifest) => {
    const m = manifest as { metadata?: { name?: string }; spec?: { description?: string } };
    return { name: m.metadata?.name, description: m.spec?.description };
  },
};

const PROJECT = "helsinki";
const SUBMIT_PATH = `/api/v1/projects/${PROJECT}/contextspaces`;
const REFUSAL = "A space called air-quality is already there; give this one another name.";

/**
 * A page around the dialog, the way the kind pages use it: the page owns the form's data and the
 * proposal, the dialog owns the fields, the views and the footer.
 */
function SpacePage(props: {
  withSource?: boolean;
  closedReason?: string;
  verdict?: Verdict | null;
  initial?: Space;
  draft?: boolean;
  kind?: string;
}): React.JSX.Element {
  const [data, setData] = useState<Space>(props.initial ?? {});
  const propose = useMutation({
    mutationFn: async (form: Space) =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/{plural}", {
          params: { path: { project: PROJECT, plural: "contextspaces" } },
          body: SOURCE.toManifest(form) as never,
        }),
      ),
  });
  const error = propose.error
    ? propose.error instanceof ApiError
      ? (propose.error.problem?.detail ?? propose.error.message)
      : propose.error.message
    : null;
  return (
    <ResourceFormDialog<Space>
      open
      onOpenChange={() => {}}
      title="New context space"
      description="A place for the data of one purpose."
      schema={SCHEMA}
      kind={props.kind}
      formData={data}
      onChange={(next) => setData(next ?? {})}
      submitLabel="Propose space"
      submitDisabledReason={props.closedReason}
      submitting={propose.isPending}
      error={error}
      source={props.withSource ? SOURCE : undefined}
      project={props.draft ? PROJECT : undefined}
      draftKind={props.draft ? "ContextSpace" : undefined}
      draftName={props.draft ? data.name : undefined}
      verdict={props.verdict}
      onSubmit={(form) => propose.mutate(form)}
    />
  );
}

function json(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });
}

/** Mounts the page with `fetch` answering the dialog's reads and recording every write. */
function show(props: Parameters<typeof SpacePage>[0] = {}, answers: { forms?: unknown[] } = {}) {
  const sent: Request[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const request = input instanceof Request ? input : new Request(new URL(String(input), window.location.origin), init);
      const path = new URL(request.url).pathname;
      if (request.method !== "GET") sent.push(request);
      if (path === "/api/v1/branding") return json(OTHER_BRAND);
      if (path.endsWith("/preferences")) return json({});
      if (path.endsWith("/forms")) return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: answers.forms ?? [] });
      if (path.includes("/drafts/")) return json({ title: "Not Found", status: 404 }, 404);
      return json({ apiVersion: "joinedcontext.com/v1alpha1", kind: "List", items: [] });
    }),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  const rendered = render(
    <QueryClientProvider client={client}>
      <I18nextProvider i18n={i18n}>
        <BrandingProvider>
          <SpacePage {...props} />
        </BrandingProvider>
      </I18nextProvider>
    </QueryClientProvider>,
  );
  return { ...rendered, sent };
}

const dialog = () => screen.findByRole("dialog");

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("the form and the YAML are one tab list (UI-16)", () => {
  // UI-16: the shared Tabs, so both views are one tab stop and the arrows move between them.
  it("the_two_views_are_one_tab_stop_and_the_arrow_keys_switch_them", async () => {
    const user = userEvent.setup();
    show({ withSource: true, initial: { name: "air-quality" } });
    const form = await within(await dialog()).findByRole("tab", { name: en.form.view.form });
    const yaml = within(await dialog()).getByRole("tab", { name: en.form.view.yaml });
    expect(form).toHaveAttribute("tabindex", "0");
    expect(yaml).toHaveAttribute("tabindex", "-1");
    form.focus();
    await user.keyboard("{ArrowRight}");
    expect(yaml).toHaveAttribute("aria-selected", "true");
    expect(yaml).toHaveFocus();
    expect((await screen.findByRole("textbox", { name: "YAML" }) as HTMLTextAreaElement).value).toContain("air-quality");
  });

  // UI-16: the panel is named by the tab that shows it, and the tab says which panel it controls.
  it("each_panel_is_named_by_its_tab_and_the_tab_controls_it", async () => {
    show({ withSource: true });
    const tab = await within(await dialog()).findByRole("tab", { name: en.form.view.form });
    const panel = within(await dialog()).getByRole("tabpanel");
    expect(tab).toHaveAttribute("aria-controls", panel.id);
    expect(panel).toHaveAccessibleName(en.form.view.form);
  });

  // UI-16: a dialog without a YAML view has no tab list and no orphan tab panel.
  it("a_dialog_without_a_yaml_view_draws_no_tabs", async () => {
    show();
    await within(await dialog()).findByLabelText(/Name/);
    expect(screen.queryByRole("tablist")).toBeNull();
    expect(screen.queryByRole("tabpanel")).toBeNull();
  });
});

describe("the YAML view's Propose (UI-44, UI-48)", () => {
  // UI-44: closed for a reason the page gives — reachable by Tab and it says why.
  it("a_closed_proposal_is_reachable_and_says_why", async () => {
    const user = userEvent.setup();
    const reason = "Proposing is closed while this project's change is under review.";
    const { sent } = show({ withSource: true, closedReason: reason, initial: { name: "air-quality" } });
    await user.click(await within(await dialog()).findByRole("tab", { name: en.form.view.yaml }));
    const propose = within(await dialog()).getByRole("button", { name: "Propose space" });
    expectDenied(propose, reason);
    await user.click(propose);
    expect(sent.filter((request) => new URL(request.url).pathname === SUBMIT_PATH)).toEqual([]);
  });

  // UI-48: an open proposal from the YAML view sends the manifest typed there, once.
  it("an_open_proposal_sends_the_typed_manifest_once", async () => {
    const user = userEvent.setup();
    const { sent } = show({ withSource: true, initial: { name: "air-quality" } });
    await user.click(await within(await dialog()).findByRole("tab", { name: en.form.view.yaml }));
    const propose = within(await dialog()).getByRole("button", { name: "Propose space" });
    expectOpen(propose);
    await user.click(propose);
    await waitFor(() => expect(sent.filter((request) => new URL(request.url).pathname === SUBMIT_PATH)).toHaveLength(1));
    const body = (await sent.find((request) => new URL(request.url).pathname === SUBMIT_PATH)!.json()) as {
      metadata: { name: string };
    };
    expect(body.metadata.name).toBe("air-quality");
  });
});

describe("the advanced fields switch (UI-16)", () => {
  const FORM = {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "UiSchema",
    metadata: { name: "testspace", namespace: "org" },
    spec: { for: "TestSpace", fields: { description: { advanced: true } } },
  };

  // UI-16: the shared Checkbox, labelled, on the focus ring, and it saves the preference.
  it("is_the_shared_checkbox_with_a_label_and_the_focus_ring_and_saves_the_choice", async () => {
    const user = userEvent.setup();
    const { sent } = show({ kind: "TestSpace" }, { forms: [FORM] });
    const box = await within(await dialog()).findByRole("checkbox", { name: en.form.advancedMode });
    expect(box.className).toContain("focus-ring");
    expect(screen.queryByLabelText(/Description/)).toBeNull();
    await user.click(box);
    expect(box).toBeChecked();
    expect(await screen.findByLabelText(/Description/)).toBeInTheDocument();
    await waitFor(() =>
      expect(sent.some((request) => request.method === "PUT" && new URL(request.url).pathname.endsWith("/preferences"))).toBe(true),
    );
  });
});

describe("the verdict chip (UI-15, UI-30)", () => {
  const initial = { name: "air-quality" };
  const checkedAt = () => new Date(Date.now() - 2 * 60_000).toISOString();
  const verdict = (ok: boolean, findings: Verdict["findings"] = []): Verdict => ({
    ok,
    findings,
    checkedAt: checkedAt(),
    inputDigest: digestOf(initial),
  });

  // UI-15: the age inside the translated verdict is the locale's own, not an English "2m ago".
  it("says_the_age_in_the_persons_language", async () => {
    show({ draft: true, initial, verdict: verdict(true) });
    expect(await screen.findByTestId("draft-verdict")).toHaveTextContent("Checked 2 min ago");
    await i18n.changeLanguage("de");
    await waitFor(() => expect(screen.getByTestId("draft-verdict")).toHaveTextContent(`${de.drafts.verdict.green.replace("{age}", "2 min")}`));
    expect(screen.getByTestId("draft-verdict").textContent).not.toMatch(/ago/);
  });

  // UI-30: the chip is the shared Badge, its tone follows the verdict and the words carry it too.
  it("is_the_shared_badge_in_the_tone_of_the_verdict", async () => {
    show({ draft: true, initial, verdict: verdict(false, [{ level: "error", path: "", message: "The space names a data model nobody declared." }]) });
    const chip = await screen.findByTestId("draft-verdict");
    expect(chip).toHaveTextContent("Check failed 2 min ago");
    expect(chip.className).toContain("bg-danger-soft");
    // The findings heading is the locale's word, with no colon glued on outside the translation.
    const findings = screen.getByTestId("draft-findings");
    expect(within(findings).getByText(en.drafts.findings)).toBeInTheDocument();
    expect(findings.textContent).not.toContain(`${en.drafts.findings}:`);
    expect(findings).toHaveTextContent("The space names a data model nobody declared.");
  });

  // MF-48: a verdict that passes once another change is approved says so and names it, in its own
  // tone, and the proposal stays open with the warning listed.
  it("names_the_change_a_waiting_verdict_waits_on", async () => {
    const message = "DataModel 'air' resolves once chg-0000002a is approved";
    show({
      draft: true,
      initial,
      verdict: { ...verdict(true, [{ level: "warning", path: "spec.dataModelRef", message }]), waitsOn: ["chg-0000002a"] },
    });
    const chip = await screen.findByTestId("draft-verdict");
    expect(chip).toHaveTextContent("Passes once chg-0000002a is approved · checked 2 min ago");
    expect(chip.className).toContain("bg-info-soft");
    expect(screen.getByTestId("draft-findings")).toHaveTextContent(message);
    expect(screen.getByRole("button", { name: "Propose space" })).not.toHaveAttribute("aria-disabled", "true");
  });

  // T-4209 (UI-44, ui-writing rule 1): the server's refusal of a name says what to write, in the
  // person's language, the way the field's own check does, never a regular expression.
  it("words_the_servers_name_refusal_as_the_field_does", async () => {
    const raw =
      "invalid metadata.name ``: must match DNS-1123 label regex ^[a-z0-9]([-a-z0-9]*[a-z0-9])?$ and be 1 to 63 characters";
    show({ draft: true, initial, verdict: verdict(false, [{ level: "error", path: "metadata.name", message: raw }]) });
    const findings = await screen.findByTestId("draft-findings");
    expect(findings).toHaveTextContent(en.form.dns1123);
    expect(document.body.textContent).not.toMatch(/regex|DNS-1123/);
  });
});

describe("the dialog meets the UI contract", () => {
  // UI-04, UI-15, UI-16, UI-44, UI-48: the form contract, rule by rule (T-1730).
  it("meets_the_form_contract", async () => {
    await checkForm(() => <SpacePage />, {
      fields: [
        { label: /Name/, value: "air-quality", required: true, browser: { refuses: "Air Quality", accepts: "air-quality" } },
        { label: /Description/, value: "Air quality of the region" },
      ],
      submit: "Propose space",
      cancel: en.form.cancel,
      submitPath: SUBMIT_PATH,
      refusal: REFUSAL,
      answer: (url) => {
        if (url.pathname === "/api/v1/branding") return json(OTHER_BRAND);
        if (url.pathname.endsWith("/preferences")) return json({});
        return undefined;
      },
    });
  });

  // UI-16: axe finds nothing in either view, in the light theme and the dark one.
  it.each(["light", "dark"])("has_no_axe_violation_in_either_view_in_the_%s_theme", async (theme) => {
    document.documentElement.dataset.theme = theme;
    const user = userEvent.setup();
    show({ withSource: true, draft: true, initial: { name: "air-quality" }, verdict: null });
    await expectNoViolations(await dialog());
    await user.click(within(await dialog()).getByRole("tab", { name: en.form.view.yaml }));
    await screen.findByRole("textbox", { name: "YAML" });
    await expectNoViolations(await dialog());
    delete document.documentElement.dataset.theme;
  });

  // UI-16: every control is reached by Tab in the order it is drawn.
  it("every_control_is_reached_by_keyboard_in_dom_order", async () => {
    const user = userEvent.setup();
    show({ withSource: true, initial: { name: "air-quality" } });
    await within(await dialog()).findByLabelText(/Name/);
    await expectTabOrder(user, await dialog());
  });

  // UI-15: every string of both views in the four locales, and no key shown in place of a sentence.
  it.each(SUPPORTED_LOCALES)("says_everything_in_%s_with_no_raw_key", async (locale) => {
    await i18n.changeLanguage(locale);
    const user = userEvent.setup();
    show({ withSource: true, draft: true, initial: { name: "air-quality" }, verdict: null });
    await within(await dialog()).findByLabelText(/Name/);
    expectNoRawKeys(await dialog());
    await user.click(within(await dialog()).getAllByRole("tab")[1]);
    await screen.findByRole("textbox", { name: "YAML" });
    expectNoRawKeys(await dialog());
  });
});
