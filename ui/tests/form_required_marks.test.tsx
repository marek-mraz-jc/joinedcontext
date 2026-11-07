import { render, screen } from "@testing-library/react";
import { I18nextProvider } from "react-i18next";
import { describe, expect, it } from "vitest";
import i18n from "../src/i18n";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import type { JsonSchema } from "../src/components/forms/types";

// T-4245 (T-4163 #5): the star marks what the form asks for now, the fields the footer counts. A
// field required inside an optional block (a data source's credential pair) is asked for only
// once that block holds something.
const SCHEMA = {
  type: "object",
  required: ["name", "http"],
  properties: {
    name: { type: "string", title: "Name" },
    http: {
      type: "object",
      title: "HTTP endpoint",
      required: ["url"],
      properties: {
        url: { type: "string", title: "URL" },
        auth: {
          type: "object",
          title: "Authorization",
          properties: {
            scheme: { type: "string", title: "Scheme" },
            secretRef: {
              type: "object",
              title: "Credential reference",
              required: ["name", "key"],
              properties: {
                name: { type: "string", title: "Secret name" },
                key: { type: "string", title: "Key in the secret" },
              },
            },
          },
        },
      },
    },
    headers: {
      type: "array",
      title: "Headers",
      items: {
        type: "object",
        title: "Header",
        required: ["header"],
        properties: { header: { type: "string", title: "Header name" } },
      },
    },
  },
} as unknown as JsonSchema;

function form(formData?: unknown) {
  return render(
    <I18nextProvider i18n={i18n}>
      <SchemaForm schema={SCHEMA} formData={formData} onSubmit={() => {}} />
    </I18nextProvider>,
  );
}

const control = (label: string) =>
  document.getElementById(screen.getByText(label, { selector: "label" }).getAttribute("for") ?? "");

const starred = (label: string) =>
  screen.getByText(label, { selector: "label" }).querySelector('[aria-hidden="true"]') !== null;

describe("required marks", () => {
  it("stars the fields the form asks for and not those inside an optional, empty block", () => {
    form();
    expect(starred("Name")).toBe(true);
    expect(starred("URL")).toBe(true);
    expect(starred("Secret name")).toBe(false);
    expect(starred("Key in the secret")).toBe(false);
    expect(control("Secret name")).not.toHaveAttribute("aria-required");
    expect(control("URL")).toHaveAttribute("aria-required", "true");
  });

  it("stars the credential pair once the block holds a value", () => {
    form({ name: "a", http: { url: "https://x", auth: { secretRef: { name: "s" } } } });
    expect(starred("Secret name")).toBe(true);
    expect(starred("Key in the secret")).toBe(true);
  });

  it("stars a required field of an item someone added to a list", () => {
    form({ name: "a", http: { url: "https://x" }, headers: [{}] });
    expect(starred("Header name")).toBe(true);
  });
});
