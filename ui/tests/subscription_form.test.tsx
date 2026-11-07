/**
 * T-2344 (CC-72, DS-16, MF-31, UI-01, UI-44): a Subscription is authored through a form like every
 * other kind.
 *
 * A standing query of a space was the one thing a project declares that only YAML could write.
 * The form holds the manifest's own shape, so what it writes is what jc-core's
 * `SubscriptionSpec` reads; the refusals it makes before a round trip are the ones jc-core makes
 * after one — a credential in the address or in a header, a subscription watching nothing.
 */
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import axe from "axe-core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import validator from "../src/components/forms/validator";
import { subscriptionSchema, subscriptionUiSchema } from "../src/schemas/kinds";
import {
  fromSubscriptionEnvelope,
  toSubscriptionEnvelope,
  watchesSomething,
} from "../src/routes/SubscriptionsPage";
import type { SubscriptionForm } from "../src/routes/SubscriptionsPage";
import en from "../src/locales/en.json";

const PROJECT = "banskabystrica";

/** The labels the way the browser reads them: from the bundle of the current language. */
const t = (key: string): string => i18n.t(key);

/** A field by its whole label, so "Name" does not also find "Header name" or "Secret name". */
const label = (text: string): RegExp => new RegExp(`^${text}\\s*\\*?$`, "i");

const schema = () => subscriptionSchema(t, ["ovzdusie"], ["dispecing-hook"]);

/** A subscription a dispatcher would write: PM10 over the limit, posted to the city's hook. */
const FILLED: SubscriptionForm = {
  name: "ovzdusie-prekrocenia",
  contextSpaceRef: "ovzdusie",
  subscriptionName: "Prekročenia PM10",
  entities: [{ type: "AirQualityObserved" }],
  watchedAttributes: ["pm10"],
  q: "pm10>50",
  notification: {
    endpoint: {
      uri: "https://dispecing.banskabystrica.sk/hooks/ovzdusie",
      accept: "application/json",
      receiverInfo: [{ key: "X-Dispecing-Zdroj", value: "jc-ovzdusie" }],
      secretRef: { name: "dispecing-hook", key: "token" },
    },
    format: "normalized",
    attributes: ["pm10", "location"],
  },
  throttling: 60,
  expiresAt: "2027-01-01T00:00:00Z",
  isActive: true,
};

function errorsOf(data: unknown): string {
  return validator
    .validateFormData(data, schema())
    .errors.map((error) => `${error.property} ${error.message}`)
    .join("\n");
}

function form(formData?: Partial<SubscriptionForm>, onChange?: (data: unknown) => void) {
  return render(
    <QueryClientProvider client={new QueryClient({ defaultOptions: { queries: { retry: false } } })}>
      <I18nextProvider i18n={i18n}>
        <SchemaForm
          schema={schema()}
          uiSchema={subscriptionUiSchema}
          formData={formData}
          onChange={(data) => onChange?.(data)}
          onSubmit={() => {}}
        />
      </I18nextProvider>
    </QueryClientProvider>,
  );
}

describe("the Subscription form", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("accepts a filled subscription and writes the manifest jc-core reads", () => {
    expect(errorsOf(FILLED)).toBe("");
    const manifest = toSubscriptionEnvelope(PROJECT, FILLED) as {
      kind: string;
      metadata: { name: string; namespace: string; labels: Record<string, string> };
      spec: Record<string, unknown>;
    };
    expect(manifest.kind).toBe("Subscription");
    expect(manifest.metadata).toEqual({
      name: "ovzdusie-prekrocenia",
      namespace: PROJECT,
      labels: { "joinedcontext.com/space": "ovzdusie" },
    });
    // The reference is typed in the manifest and a name in the form (SP-08).
    expect(manifest.spec.contextSpaceRef).toEqual({ kind: "ContextSpace", name: "ovzdusie" });
    expect(manifest.spec.notification).toEqual(FILLED.notification);
    expect(manifest.spec.entities).toEqual([{ type: "AirQualityObserved" }]);
    // `isActive: true` is jc-core's default and stays unwritten.
    expect(manifest.spec).not.toHaveProperty("isActive");
  });

  it("refuses a subscription with no receiver address at the field", () => {
    const errors = errorsOf({
      ...FILLED,
      notification: { ...FILLED.notification, endpoint: { ...FILLED.notification.endpoint, uri: "" } },
    });
    expect(errors).toContain(".notification.endpoint.uri");
  });

  it("refuses an address with a password in it, and one that is not http", () => {
    // MF-31: a credential in the URL is a credential in Git; jc-core refuses both as well.
    for (const uri of [
      "https://dispecing:heslo@dispecing.banskabystrica.sk/hooks",
      "ftp://dispecing.banskabystrica.sk/hooks",
      "dispecing.banskabystrica.sk/hooks",
    ]) {
      const errors = errorsOf({
        ...FILLED,
        notification: { ...FILLED.notification, endpoint: { ...FILLED.notification.endpoint, uri } },
      });
      expect(errors, uri).toContain(".notification.endpoint.uri");
    }
    // An address inside the cluster over plain http is a Service name, and allowed.
    expect(
      errorsOf({
        ...FILLED,
        notification: {
          ...FILLED.notification,
          endpoint: { ...FILLED.notification.endpoint, uri: "http://dispatcher.ovzdusie.svc:8080/hook" },
        },
      }),
    ).toBe("");
  });

  it("refuses a credential header, in any spelling, and keeps an ordinary one", () => {
    for (const key of ["Authorization", "authorization", "COOKIE", "Proxy-Authorization"]) {
      const errors = errorsOf({
        ...FILLED,
        notification: {
          ...FILLED.notification,
          endpoint: { ...FILLED.notification.endpoint, receiverInfo: [{ key, value: "Bearer x" }] },
        },
      });
      expect(errors, key).toContain("receiverInfo");
    }
    expect(errorsOf(FILLED)).toBe("");
  });

  it("refuses an entity selector that names nothing, and a throttle of zero", () => {
    expect(errorsOf({ ...FILLED, entities: [{}] })).toContain(".entities");
    expect(errorsOf({ ...FILLED, throttling: 0 })).toContain(".throttling");
    expect(errorsOf({ ...FILLED, expiresAt: "next year" })).toContain(".expiresAt");
  });

  it("knows a subscription that watches nothing before the API refuses it", async () => {
    // CC-72: jc-core refuses it with "a subscription watches something"; the page says so first.
    expect(watchesSomething({ ...FILLED, entities: [], watchedAttributes: [] })).toBe(false);
    expect(watchesSomething({ ...FILLED, entities: [{ type: "" }], watchedAttributes: [" "] })).toBe(false);
    expect(watchesSomething({ ...FILLED, entities: [], watchedAttributes: ["pm10"] })).toBe(true);
    expect(watchesSomething({ ...FILLED, watchedAttributes: [] })).toBe(true);
    await i18n.loadLanguages(["sk", "cs", "de"]);
    for (const locale of ["en", "sk", "cs", "de"] as const) {
      const bundle = i18n.getResourceBundle(locale, "translation") as typeof en;
      expect(bundle.subscriptions.watchesNothing.length, locale).toBeGreaterThan(20);
    }
  });

  it("names every field in all four languages", async () => {
    for (const locale of ["en", "sk", "cs", "de"] as const) {
      await i18n.changeLanguage(locale);
      const bundle = i18n.getResourceBundle(locale, "translation") as typeof en;
      const { unmount } = form();
      expect(screen.getByLabelText(label(bundle.subscriptions.field.name))).toBeInTheDocument();
      expect(screen.getByLabelText(label(bundle.subscriptions.field.uri))).toBeInTheDocument();
      unmount();
    }
    await i18n.changeLanguage("en");
  });

  it("is filled from the keyboard alone", async () => {
    const user = userEvent.setup();
    let held: unknown;
    form(undefined, (data) => {
      held = data;
    });
    const name = screen.getByLabelText(label(en.subscriptions.field.name));
    name.focus();
    await user.keyboard("ovzdusie-prekrocenia");
    expect((held as SubscriptionForm).name).toBe("ovzdusie-prekrocenia");
    const uri = screen.getByLabelText(label(en.subscriptions.field.uri));
    uri.focus();
    await user.keyboard("https://dispecing.banskabystrica.sk/hooks/ovzdusie");
    expect((held as SubscriptionForm).notification.endpoint.uri).toBe(
      "https://dispecing.banskabystrica.sk/hooks/ovzdusie",
    );
  });

  it("has no axe violations", async () => {
    const { container } = form(FILLED);
    const results = await axe.run(container);
    expect(
      results.violations.map((violation) => `${violation.id}: ${violation.description}`),
    ).toEqual([]);
  });
});

describe("the Subscription manifest the form writes", () => {
  it("is the same pair in both directions", () => {
    const back = fromSubscriptionEnvelope(toSubscriptionEnvelope(PROJECT, FILLED));
    expect(back).toEqual({ ...FILLED });
    // A second round trip must not turn the typed reference into `[object Object]`.
    expect(fromSubscriptionEnvelope(toSubscriptionEnvelope(PROJECT, back)).contextSpaceRef).toBe("ovzdusie");
  });

  it("writes no empty member, and a parked subscription says it is parked", () => {
    const manifest = toSubscriptionEnvelope(PROJECT, {
      name: "ovzdusie-prekrocenia",
      contextSpaceRef: "ovzdusie",
      description: "   ",
      entities: [{ type: "AirQualityObserved", id: "" }, {}],
      watchedAttributes: [],
      q: "",
      notification: { endpoint: { uri: "https://dispecing.banskabystrica.sk/hooks", receiverInfo: [] } },
      isActive: false,
    }) as { spec: Record<string, unknown> };
    for (const empty of ["description", "watchedAttributes", "q", "geoQ", "throttling", "expiresAt"]) {
      expect(manifest.spec[empty], `${empty} is left out`).toBeUndefined();
    }
    expect(manifest.spec.entities).toEqual([{ type: "AirQualityObserved" }]);
    expect(manifest.spec.notification).toEqual({
      endpoint: { uri: "https://dispecing.banskabystrica.sk/hooks" },
    });
    expect(manifest.spec.isActive).toBe(false);
  });

  it("keeps the stored title, description and labels through an edit", () => {
    const stored = {
      metadata: {
        name: "ovzdusie-prekrocenia",
        namespace: PROJECT,
        title: "Prekročenia PM10",
        description: "Pre dispečing.",
        labels: { "joinedcontext.com/tier": "demo", "joinedcontext.com/space": "stare" },
      },
      spec: (toSubscriptionEnvelope(PROJECT, FILLED) as { spec: object }).spec,
    };
    const edited = toSubscriptionEnvelope(PROJECT, fromSubscriptionEnvelope(stored), stored) as {
      metadata: Record<string, unknown>;
    };
    expect(edited.metadata.title).toBe("Prekročenia PM10");
    expect(edited.metadata.description).toBe("Pre dispečing.");
    // The space label follows the space the form holds; every other label stays.
    expect(edited.metadata.labels).toEqual({
      "joinedcontext.com/tier": "demo",
      "joinedcontext.com/space": "ovzdusie",
    });
    expect(edited.metadata.name).toBe("ovzdusie-prekrocenia");
  });

  it("opens a manifest written by hand, with the space as a bare name", () => {
    const back = fromSubscriptionEnvelope({
      metadata: { name: "rucne" },
      spec: {
        contextSpaceRef: "ovzdusie",
        watchedAttributes: ["pm10"],
        notification: { endpoint: { uri: "https://dispecing.banskabystrica.sk/hooks" } },
      },
    });
    expect(back.contextSpaceRef).toBe("ovzdusie");
    expect(back.isActive).toBe(true);
    expect(errorsOf(back)).toBe("");
  });
});
