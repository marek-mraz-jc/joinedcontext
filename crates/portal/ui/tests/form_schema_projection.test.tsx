/**
 * T-1767: the published-attributes panel against the UI contract (UI-04, UI-15, UI-16, UI-44).
 *
 * Five defects of this panel are held here. Its artifact query is `enabled: index.isSuccess`,
 * and a disabled query stays `pending` for ever in react-query v5, so a failed index showed
 * "reading the published schema" and "publishes no schema yet" at the same time, for ever. Both
 * failures were muted grey paragraphs with no role and no retry, styled like the hint above
 * them, so a fault read as advice. An endpoint whose schema names no type rendered nothing at
 * all. A typed attribute name was added with no word about whether it hides anything. And the
 * panel took no permission at all: a viewer could tick every box and type new names, and learn
 * it was refused only when Propose said so — the sequence UI-44 exists to prevent.
 *
 * What is deliberately NOT refused: a name the published schema does not carry. An attribute can
 * be hidden before the model that introduces it is compiled, and `endpoint_projection_ui.test.tsx`
 * holds that case; it is said under the field instead.
 */
import { useState } from "react";
import { screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { checkForm, type FormSpec } from "./formContract";
import { expectDenied, expectNoViolations } from "./checks";
import { json, problem, renderPage } from "./page_contract";

const { SchemaProjectionPanel } = await import("../src/pages/endpoints/SchemaProjectionPanel");

const SLUG = "k7m2qz4tv6xh3n5jb2ryd3wcfa";
const INDEX = `/api/endpoint/${SLUG}/schema/index.json`;
const ARTIFACT = `/api/endpoint/${SLUG}/schema/v1/json-schema`;

const SCHEMA = {
  $defs: {
    AirQualityObserved: {
      title: "AirQualityObserved",
      properties: { pm10: {}, calibrationOffset: {}, dateObserved: {} },
    },
  },
};

function reads(over: { index?: () => Response; artifact?: () => Response } = {}) {
  return (url: URL) => {
    if (url.pathname === INDEX) return over.index?.() ?? json({ models: [{ name: "air", version: 1 }] });
    if (url.pathname === ARTIFACT) return over.artifact?.() ?? json(SCHEMA);
    return undefined;
  };
}

const panel = (props: Partial<{ hidden: string[]; onHiddenChange: (h: string[]) => void; disabledReason: string }> = {}) => (
  <SchemaProjectionPanel
    slug={SLUG}
    hidden={props.hidden ?? []}
    onHiddenChange={props.onHiddenChange ?? (() => {})}
    disabledReason={props.disabledReason}
  />
);

const spec: FormSpec = {
  fields: [{ id: "projection-typed", label: /Hide an attribute by name/, value: "pm10" }],
  submit: /^Hide$/,
  path: "/projects/banskabystrica",
  answer: reads(),
  // The formalism list is the gateway's own names — `json-schema`, `context.jsonld` — which no
  // bundle translates and which are shaped exactly like a translation key.
  keysExclude: ["option"],
};

beforeEach(async () => {
  await i18n.changeLanguage("en");
});
afterEach(() => {
  vi.unstubAllGlobals();
});

describe("hiding an attribute by name (T-1767)", () => {
  it("meets_the_form_contract", async () => {
    await checkForm((proposed) => panel({ onHiddenChange: () => proposed() }), spec);
  });

  it("refuses_an_empty_name_a_repeat_and_a_name_with_a_space", async () => {
    const added: string[][] = [];
    const user = (await import("@testing-library/user-event")).default.setup();
    renderPage(panel({ hidden: ["calibrationOffset"], onHiddenChange: (h) => added.push(h) }), {
      path: "/projects/banskabystrica",
      answer: reads(),
    });
    const input = await screen.findByLabelText(/Hide an attribute by name/);
    const add = screen.getByRole("button", { name: /^Hide$/ });

    await user.click(add);
    expect(await screen.findByText(i18n.t("endpoints.projection.addEmpty"))).toBeInTheDocument();

    await user.type(input, "calibrationOffset");
    await user.click(add);
    expect(await screen.findByText(i18n.t("endpoints.projection.addAlready"))).toBeInTheDocument();

    await user.clear(input);
    await user.type(input, "two words");
    await user.click(add);
    expect(added, "a name that cannot be an attribute was added").toEqual([]);
  });

  it("adds_a_name_the_schema_does_not_publish_and_says_it_hides_nothing_yet", async () => {
    const added: string[][] = [];
    const user = (await import("@testing-library/user-event")).default.setup();
    // A wrapper that keeps the hidden list, because the panel is controlled by its caller and
    // `rerender` from the harness would remount it outside the providers.
    function Holder(): React.JSX.Element {
      const [hidden, setHidden] = useState<string[]>([]);
      return panel({
        hidden,
        onHiddenChange: (next) => {
          added.push(next);
          setHidden(next);
        },
      });
    }
    renderPage(<Holder />, { path: "/projects/banskabystrica", answer: reads() });
    await waitFor(() => expect(screen.getByText("pm10")).toBeInTheDocument());
    await user.type(await screen.findByLabelText(/Hide an attribute by name/), "internalNote");
    await user.click(screen.getByRole("button", { name: /^Hide$/ }));
    expect(added, "a name the schema has not caught up with must still be hideable").toEqual([
      ["internalNote"],
    ]);
    expect(await screen.findByText(/no attribute called internalNote today/)).toBeInTheDocument();
  });

  it("reports_a_failed_index_once_with_a_way_to_try_again", async () => {
    const view = renderPage(panel(), {
      path: "/projects/banskabystrica",
      answer: reads({ index: () => problem(503, "The schema surface is not answering.") }),
    });
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent(i18n.t("endpoints.projection.unavailable"));
    expect(within_(alert).getByRole("button", { name: i18n.t("form.listRetry") })).toBeInTheDocument();
    // The artifact query never starts when the index failed; "reading the schema" must not stay
    // on the page beside the failure that stopped it.
    expect(screen.queryByText(i18n.t("endpoints.projection.loading"))).toBeNull();
    await expectNoViolations(view.container);
  });

  it("says_an_endpoint_that_publishes_no_type_publishes_none", async () => {
    renderPage(panel(), {
      path: "/projects/banskabystrica",
      answer: reads({ artifact: () => json({ $defs: {} }) }),
    });
    expect(await screen.findByText(i18n.t("endpoints.projection.noTypes"))).toBeInTheDocument();
  });

  it("refuses_every_control_with_its_reason_to_a_person_who_may_not_propose", async () => {
    const reason = "You may not propose an Endpoint here.";
    renderPage(panel({ hidden: ["calibrationOffset"], disabledReason: reason }), {
      path: "/projects/banskabystrica",
      answer: reads(),
    });
    expectDenied(await screen.findByRole("button", { name: /^Hide$/ }), reason);
    const boxes = await screen.findAllByRole("checkbox");
    for (const box of boxes) expectDenied(box, reason);
  });
});

/** `within` without importing the whole namespace twice. */
function within_(element: HTMLElement) {
  return {
    getByRole: (role: string, options: { name: string }) =>
      [...element.querySelectorAll<HTMLElement>("button")].find(
        (node) => (node.textContent ?? "").trim() === options.name && role === "button",
      ) ?? (() => { throw new Error(`no ${role} named ${options.name}`); })(),
  };
}
