import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { I18nextProvider } from "react-i18next";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import { SchemaForm } from "../src/components/forms/SchemaForm";
import { errorMessageKey } from "../src/components/forms/errorMessages";
import { DNS1123, ENTITY_TYPE_PATTERN, GIT_URL_PATTERN, HTTPS_URL_PATTERN, SYNC_INTERVAL_PATTERN, ENTITY_URN_PATTERN, NOTIFICATION_URI_PATTERN, RFC3339_PATTERN, HEADER_NAME_PATTERN } from "../src/schemas/kinds";
import en from "../src/locales/en.json";
import type { JsonSchema } from "../src/components/forms/types";

const schema: JsonSchema = {
  type: "object",
  required: ["name"],
  properties: {
    name: {
      type: "string",
      title: "Name",
    },
    count: {
      type: "number",
      title: "Count",
    },
  },
};

interface FormData {
  name: string;
  count?: number;
}

describe("SchemaForm", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  /// T-0960, PF-09: "does not match pattern" leaves a person holding a regular expression.
  it("says what a name may be, rather than that it does not match a pattern", async () => {
    const user = userEvent.setup();
    const named: JsonSchema = {
      type: "object",
      properties: { name: { type: "string", title: "Name", pattern: DNS1123 } },
    };
    render(
      <I18nextProvider i18n={i18n}>
        <SchemaForm<{ name?: string }> schema={named} onSubmit={() => {}} />
      </I18nextProvider>,
    );

    await user.type(screen.getByLabelText(/Name/), "Ovzduší");

    expect(await screen.findByText(en.form.dns1123)).toBeInTheDocument();
    expect(screen.queryByText(en.form.pattern)).toBeNull();
  });

  it("keys the message on the pattern, so every name rule keeps its own words", () => {
    const of = (pattern: string) =>
      errorMessageKey(
        { name: "pattern", schemaPath: "#/properties/name/pattern" } as never,
        { type: "object", properties: { name: { type: "string", pattern } } } as never,
      );
    expect(of(DNS1123)).toBe("form.dns1123");
    expect(of(ENTITY_TYPE_PATTERN)).toBe("form.entityType");
    // T-3219: the formats a person types by hand say which format, with an example.
    expect(of(GIT_URL_PATTERN)).toBe("form.gitUrl");
    expect(of(HTTPS_URL_PATTERN)).toBe("form.httpsUrl");
    expect(of(SYNC_INTERVAL_PATTERN)).toBe("form.interval");
    expect(of(ENTITY_URN_PATTERN)).toBe("form.entityUrn");
    expect(of(NOTIFICATION_URI_PATTERN)).toBe("form.notificationUri");
    expect(of(RFC3339_PATTERN)).toBe("form.instant");
    expect(of(HEADER_NAME_PATTERN)).toBe("form.headerName");
    // A pattern the platform did not write keeps the generic message: inventing words for it
    // would describe a rule this form knows nothing about.
    expect(of("^[0-9]{4}$")).toBe("form.pattern");
    expect(errorMessageKey({ name: "required" } as never)).toBe("form.required");
  });

  it("renders a schema with required string name and number count and asserts both labels exist", () => {
    render(
      <I18nextProvider i18n={i18n}>
        <SchemaForm<FormData> schema={schema} onSubmit={() => {}} />
      </I18nextProvider>
    );

    expect(screen.getByLabelText(/name/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/count/i)).toBeInTheDocument();
  });

  it("types 'hello' into name, submits, and asserts onSubmit received { name: 'hello' }", async () => {
    const onSubmit = vi.fn();
    const user = userEvent.setup();

    render(
      <I18nextProvider i18n={i18n}>
        <SchemaForm<FormData> schema={schema} onSubmit={onSubmit} />
      </I18nextProvider>
    );

    const nameInput = screen.getByLabelText(/name/i);
    await user.type(nameInput, "hello");

    const submitBtn = screen.getByRole("button", { name: en.form.submit });
    await user.click(submitBtn);

    expect(onSubmit).toHaveBeenCalledTimes(1);
    expect(onSubmit).toHaveBeenCalledWith({ name: "hello" });
  });

  it("submits empty, asserts translated required message is shown, field carries aria-invalid='true', and onSubmit was not called", async () => {
    const onSubmit = vi.fn();
    const user = userEvent.setup();

    render(
      <I18nextProvider i18n={i18n}>
        <SchemaForm<FormData> schema={schema} onSubmit={onSubmit} />
      </I18nextProvider>
    );

    const submitBtn = screen.getByRole("button", { name: en.form.submit });
    await user.click(submitBtn);

    expect(screen.getByText("This field is required")).toBeInTheDocument();

    const nameInput = screen.getByLabelText(/name/i);
    expect(nameInput).toHaveAttribute("aria-invalid", "true");

    expect(onSubmit).not.toHaveBeenCalled();
  });
});
