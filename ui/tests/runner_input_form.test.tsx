/**
 * UI-15, UI-16 (T-2137): the form a data source's runner input is configured with.
 *
 * The form itself is `SchemaForm`; what this file is about is the translation from a runner's
 * field tree into it, because that translation decides one thing that matters beyond the page: a
 * field the catalog marks `secret: true` must never carry the credential a person types. It
 * carries the reference `${DS_<NAME>_<KEY>}`, and the page is handed the `secretRef` to write
 * into the manifest (MF-35, PL-16, PL-50).
 */
import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import i18n from "../src/i18n";
import type { CatalogInput } from "../src/schemas/kinds";
import { RunnerInputForm } from "../src/pages/datasources/RunnerInputForm";
import { expectNoAxeViolations, renderPart } from "./page_contract";
import { expectDenied } from "./checks";
import type { ErrorSchema } from "@rjsf/utils";

/** One field's refusal in the shape rjsf takes, the way `form_schema_form.test.tsx` builds it. */
function errorOn(field: string, message: string): ErrorSchema {
  const root: ErrorSchema = {};
  (root as Record<string, unknown>)[field] = { __errors: [message] };
  return root;
}

const INPUT: CatalogInput = {
  name: "postgres_cdc",
  group: "database",
  summary: "Reads a PostgreSQL table as it changes.",
  fields: [
    {
      path: "dsn",
      type: "string",
      kind: "scalar",
      secret: false,
      advanced: false,
      default: null,
      description: "The connection string, without the password.",
    },
    {
      path: "password",
      type: "string",
      kind: "scalar",
      secret: true,
      advanced: false,
      default: null,
      description: "The role's password.",
    },
    {
      path: "batch_size",
      type: "int",
      kind: "scalar",
      secret: false,
      advanced: true,
      default: 100,
      description: "How many rows are read at once.",
    },
  ],
};

function show(over: Partial<Parameters<typeof RunnerInputForm>[0]> = {}) {
  const onSubmit = vi.fn();
  const onSecretRef = vi.fn();
  const view = renderPart(
    <RunnerInputForm
      input={INPUT}
      submitLabel="Add the source"
      knownSecretNames={["city-warehouse"]}
      onSecretRef={onSecretRef}
      onSubmit={onSubmit}
      {...over}
    />,
  );
  return { ...view, onSubmit, onSecretRef };
}

describe("what the form forwards to SchemaForm (T-1766)", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  /// UI-48: a run that is starting says so on the button that started it, and takes no second
  /// press. Before this the three props existed on `SchemaForm` and this form forwarded none of
  /// them, so no call site could ever show any of the three states.
  it("shows_the_submit_working_while_the_run_is_starting", () => {
    show({ submitting: true });
    const submit = screen.getByRole("button", { name: "Add the source" });
    expect(submit).toHaveAttribute("aria-busy", "true");
  });

  /// UI-44: refused with the reason on the control, reachable and readable, rather than a 403
  /// after the form has been filled in.
  it("refuses_the_submit_with_its_reason_to_a_person_who_may_not_run", () => {
    show({ submitting: false, submitDisabledReason: "You may not start a run in this project." });
    expectDenied(
      screen.getByRole("button", { name: "Add the source" }),
      "You may not start a run in this project.",
    );
  });

  /// UI-04: the server's refusal belongs beside the field it is about, not in a banner above a
  /// form whose fields look fine.
  it("shows_the_servers_refusal_beside_the_field_it_names", async () => {
    show({ extraErrors: errorOn("dsn", "This database refused the connection string.") });
    expect(
      await screen.findByText("This database refused the connection string."),
    ).toBeInTheDocument();
    const dsn = screen.getByLabelText(/dsn/i);
    const described = (dsn.getAttribute("aria-describedby") ?? "").split(/\s+/);
    const message = described
      .map((id) => document.getElementById(id))
      .find((node) => node?.textContent?.includes("This database refused the connection string."));
    expect(message, "the refusal is tied to the field it is about").toBeTruthy();
  });
});

describe("a runner input's form", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("asks for each field the runner documents, with the runner's own words", () => {
    show();
    expect(screen.getByLabelText(/dsn/i)).toBeInTheDocument();
    expect(screen.getByText("The connection string, without the password.")).toBeInTheDocument();
    expect(screen.getByText("How many rows are read at once.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Add the source" })).toBeInTheDocument();
  });

  it("hands the typed configuration to the page when it is submitted", async () => {
    const user = userEvent.setup();
    const { onSubmit } = show();
    await user.type(screen.getByLabelText(/dsn/i), "postgres://reader@warehouse/city");
    await user.type(screen.getByPlaceholderText(/secret name/i), "city-warehouse");
    await user.click(screen.getByRole("button", { name: "Add the source" }));
    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    expect(onSubmit.mock.calls[0][0]).toMatchObject({
      dsn: "postgres://reader@warehouse/city",
      // The advanced field keeps the runner's own default rather than being sent empty.
      batch_size: 100,
    });
  });

  it("does not submit a configuration the runner would refuse, and says which field", async () => {
    const user = userEvent.setup();
    const { onSubmit } = show();
    await user.type(screen.getByLabelText(/dsn/i), "postgres://reader@warehouse/city");
    await user.click(screen.getByRole("button", { name: "Add the source" }));
    await waitFor(() => expect(screen.getByRole("alert")).toBeInTheDocument());
    expect(onSubmit).not.toHaveBeenCalled();
  });

  it("never carries a secret in the form: the field holds a reference, the page gets the secretRef", async () => {
    const user = userEvent.setup();
    const { onSubmit, onSecretRef, container } = show();

    // The secret field is two halves — the name of the secret and the key in it — and neither
    // is the credential itself. There is nowhere on this form to type one.
    const name = screen.getByPlaceholderText(/secret name/i);
    await user.type(name, "city-warehouse");
    await user.type(screen.getByLabelText(/dsn/i), "postgres://reader@warehouse/city");
    await user.click(screen.getByRole("button", { name: "Add the source" }));

    await waitFor(() => expect(onSubmit).toHaveBeenCalledTimes(1));
    const data = onSubmit.mock.calls[0][0] as Record<string, unknown>;
    expect(data.password).toBe("${DS_CITY_WAREHOUSE_PASSWORD}");
    expect(onSecretRef).toHaveBeenCalledWith(
      "DS_CITY_WAREHOUSE_PASSWORD",
      expect.objectContaining({ name: "city-warehouse", key: "password" }),
    );
    // And no input on the page took a password: the two halves are both plain text fields.
    expect(container.querySelectorAll('input[type="password"]')).toHaveLength(0);
  });

  it("offers the secrets the project already holds rather than asking for the spelling", () => {
    const { container } = show();
    const options = [...container.querySelectorAll("datalist option")].map((o) =>
      o.getAttribute("value"),
    );
    expect(options).toContain("city-warehouse");
  });

  it("takes no input while the page is busy", () => {
    show({ disabled: true });
    expect(screen.getByLabelText(/dsn/i)).toBeDisabled();
    expect(screen.getByRole("button", { name: "Add the source" })).toBeDisabled();
  });

  it("has no axe violation", async () => {
    const { container } = show();
    await expectNoAxeViolations(container);
  });
});
