/**
 * T-2634 (UI-44, PF-57): a Subscription or a Policy whose Context Space is left empty is refused at
 * that field, and Check sends nothing. Sent, it reached the server as `contextSpaceRef: {kind}`
 * with no name, and the only answer was "manifest does not parse: … untagged enum Ref".
 */
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import en from "../src/locales/en.json";
import { ResourceFormDialog } from "../src/components/ResourceFormDialog";
import type { JsonSchema } from "../src/components/forms/types";
import { policySchema, subscriptionSchema } from "../src/schemas/kinds";
import { toPolicyEnvelope, fromPolicyEnvelope } from "../src/routes/PoliciesPage";
import { toSubscriptionEnvelope, fromSubscriptionEnvelope } from "../src/routes/SubscriptionsPage";

const PROJECT = "helsinki";

class SilentEventSource {
  addEventListener() {}
  removeEventListener() {}
  close() {}
}

interface Kind {
  kind: string;
  plural: string;
  schema: JsonSchema;
  space: string;
  filled: Record<string, unknown>;
  source: { toManifest: (form: Record<string, unknown>) => unknown; fromManifest: (manifest: unknown) => Record<string, unknown> };
}

const KINDS: Kind[] = [
  {
    kind: "Subscription",
    plural: "subscriptions",
    schema: subscriptionSchema((key) => i18n.t(key), ["helsinki"]),
    space: en.subscriptions.field.space,
    filled: { name: "bikes-watch", notification: { endpoint: { uri: "https://hooks.example.test/in" } } },
    source: {
      toManifest: (form) => toSubscriptionEnvelope(PROJECT, form as never),
      fromManifest: (manifest) => fromSubscriptionEnvelope(manifest) as never,
    },
  },
  {
    kind: "Policy",
    plural: "policies",
    schema: policySchema((key) => i18n.t(key), ["helsinki"]),
    space: en.policies.field.space,
    filled: { name: "stewards-read", assignee: { group: "stewards" }, operations: ["queryEntity"] },
    source: {
      toManifest: (form) => toPolicyEnvelope(PROJECT, "hel.fi", form as never),
      fromManifest: (manifest) => fromPolicyEnvelope(manifest) as never,
    },
  },
];

function Harness({ kind }: { kind: Kind }) {
  const [form, setForm] = useState<Record<string, unknown> | undefined>(kind.filled);
  return (
    <ResourceFormDialog<Record<string, unknown>>
      kind={kind.kind}
      open
      onOpenChange={() => {}}
      title={kind.kind}
      description={kind.kind}
      project={PROJECT}
      draftKind={kind.kind}
      plural={kind.plural}
      schema={kind.schema}
      source={kind.source}
      formData={form}
      onChange={setForm}
      submitLabel="Propose"
      onSubmit={() => {}}
    />
  );
}

describe("a required reference left empty is refused at the field, and Check sends nothing (T-2634)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    vi.stubGlobal("EventSource", SilentEventSource);
  });
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it.each(KINDS)("$kind", async (kind) => {
    const checks: unknown[] = [];
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: RequestInfo | URL) => {
        const request = input as Request;
        if (request.url.includes("/api/v1/branding")) {
          return new Response(JSON.stringify({ validation: "strict" }), { status: 200 });
        }
        if (request.method === "POST" && request.url.includes("dryRun=All")) {
          checks.push(await request.clone().json());
        }
        return new Response(JSON.stringify({}), { status: 200 });
      }),
    );
    render(
      <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
        <I18nextProvider i18n={i18n}>
          <Harness kind={kind} />
        </I18nextProvider>
      </QueryClientProvider>,
    );

    await userEvent.click(await screen.findByRole("button", { name: en.form.check }));

    const space = await screen.findByLabelText(new RegExp(`^${kind.space}`));
    await waitFor(() => expect(space).toHaveAttribute("aria-invalid", "true"));
    const described = (space.getAttribute("aria-describedby") ?? "")
      .split(" ")
      .map((id) => document.getElementById(id)?.textContent ?? "")
      .join(" ");
    expect(described).toContain(i18n.t("form.required"));
    // Given a moment to send, it sends nothing.
    await new Promise((resolve) => setTimeout(resolve, 100));
    expect(checks).toEqual([]);
  });
});
