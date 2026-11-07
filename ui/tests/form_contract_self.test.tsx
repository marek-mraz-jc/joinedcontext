/**
 * T-1730: the form contract bites (UI-04, UI-15, UI-16, UI-44, UI-48).
 *
 * One form that meets the contract passes; one broken form per rule fails.
 *
 * A checklist nobody tested is a checklist that can stop checking. Each case below breaks exactly
 * one rule of `checkForm` and asserts that the rule it broke is the one that fails, so a helper
 * that silently stops asserting something is caught on the commit that does it.
 */
import { useRef, useState } from "react";
import { describe, expect, it } from "vitest";
import { Alert } from "../src/components/ui/Alert";
import { Button } from "../src/components/ui/Button";
import { Field } from "../src/components/ui/Field";
import { checkForm, screen, type FormSpec } from "./formContract";

const SUBMIT_PATH = "/api/v1/projects/helsinki/policies";
const REFUSAL = "A policy of this name is already there; give this one another name.";

type Break =
  | "none"
  | "label"
  | "required"
  | "describedby"
  | "empty-submit"
  | "browser-rule"
  | "status-code"
  | "lose-input"
  | "double-send"
  | "discard"
  | "secret"
  | "denied"
  | "two-primaries"
  | "primary-first"
  | "hostile-html";

interface FormProps {
  broken: Break;
  /** A form that proposes through a callback rather than a request, as the KPI card does. */
  propose?: () => void;
}

/** The smallest form that meets the contract, with one switch per rule to break. */
function DemoForm({ broken, propose: handOver }: FormProps): React.JSX.Element {
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [kind, setKind] = useState("");
  const [secret, setSecret] = useState("");
  const [errors, setErrors] = useState<Record<string, string[]>>({});
  const [refused, setRefused] = useState<string | null>(null);
  const [asking, setAsking] = useState(false);
  const [pending, setPending] = useState(false);
  const running = useRef(false);

  const nameIsBad = name.length === 0 || !/^[a-z-]+$/.test(name);
  const urlIsBad = url.length > 0 && !/^https?:\/\//.test(url);

  async function propose(): Promise<void> {
    if (broken !== "empty-submit") {
      if (nameIsBad) {
        setErrors({ name: ["A name is lower case letters and hyphens."] });
        document.getElementById("policy-name")?.focus();
        return;
      }
      if (kind === "") {
        setErrors({ kind: ["Choose what the policy applies to."] });
        document.getElementById("policy-kind")?.focus();
        return;
      }
      if (broken !== "browser-rule" && urlIsBad) {
        setErrors({ url: ["An address starts with http:// or https://."] });
        return;
      }
    }
    if (running.current && broken !== "double-send") return;
    running.current = true;
    if (handOver) {
      // Handed over once. The form does not reopen itself, which is how the KPI card answers
      // the same rule: its keep-form closes the moment the message is on its way.
      handOver();
      return;
    }
    setPending(true);
    const answer = await fetch(SUBMIT_PATH, {
      method: "POST",
      body: JSON.stringify({ name, url }),
    });
    running.current = false;
    setPending(false);
    if (!answer.ok) {
      const body = (await answer.json()) as { detail?: string; status?: number };
      if (broken === "status-code") setRefused(`Request failed with status ${body.status}`);
      else setRefused(body.detail ?? "The change was refused.");
      if (broken === "lose-input") {
        setName("");
        setUrl("");
      }
    }
  }

  return (
    <form
      aria-label="New policy"
      noValidate
      onSubmit={(event) => {
        event.preventDefault();
        void propose();
      }}
    >
      <h2>New policy</h2>
      {refused ? <Alert tone="danger">{refused}</Alert> : null}
      <Field
        id="policy-name"
        label={broken === "label" ? undefined : "Name"}
        required={broken !== "required"}
        help={broken === "describedby" ? undefined : "Lower case letters and hyphens."}
        errors={errors.name}
      >
        <input
          id="policy-name"
          name="policy-name"
          value={name}
          onChange={(event) => setName(event.target.value)}
          className="border border-border"
        />
      </Field>
      {broken === "describedby" ? (
        <p id="policy-name__help">Lower case letters and hyphens.</p>
      ) : null}
      <Field id="policy-url" label="Address" errors={errors.url}>
        <input
          id="policy-url"
          name="policy-url"
          type="url"
          value={url}
          onChange={(event) => setUrl(event.target.value)}
          className="border border-border"
        />
      </Field>
      <Field id="policy-kind" label="Applies to" required errors={errors.kind}>
        <select
          id="policy-kind"
          name="policy-kind"
          value={kind}
          onChange={(event) => setKind(event.target.value)}
          className="border border-border"
        >
          <option value="">Choose a kind</option>
          <option value="Endpoint">Endpoint</option>
          <option value="ContextSpace">Context space</option>
        </select>
      </Field>
      <Field id="policy-secret" label="Token">
        <input
          id="policy-secret"
          name="policy-secret"
          type={broken === "secret" ? "text" : "password"}
          autoComplete={broken === "secret" ? "on" : "new-password"}
          value={secret}
          onChange={(event) => setSecret(event.target.value)}
          className="border border-border"
        />
      </Field>
      {broken === "hostile-html" ? (
        <div data-testid="preview" dangerouslySetInnerHTML={{ __html: name }} />
      ) : (
        <p data-testid="preview">{name}</p>
      )}
      {asking ? (
        <div role="alertdialog" aria-label="Discard this draft?">
          <p>Discard what you typed?</p>
          <Button onClick={() => setAsking(false)}>Keep editing</Button>
        </div>
      ) : null}
      {broken === "primary-first" ? (
        <>
          <Button variant="primary" type="submit">
            Propose the change
          </Button>
          <Button
            variant="secondary"
            onClick={() => {
              if (name.length > 0 || url.length > 0) setAsking(true);
            }}
          >
            Cancel
          </Button>
        </>
      ) : (
        <>
          <Button
            variant={broken === "two-primaries" ? "primary" : "secondary"}
            onClick={() => {
              if (broken === "discard") {
                setName("");
                setUrl("");
                return;
              }
              if (name.length > 0 || url.length > 0) setAsking(true);
            }}
          >
            Cancel
          </Button>
          <Button variant="primary" type="submit" loading={pending && broken !== "double-send"}>
            Propose the change
          </Button>
        </>
      )}
    </form>
  );
}

const spec: FormSpec = {
  fields: [
    { id: "policy-name", label: "Name", value: "air-quality", required: true },
    { id: "policy-kind", label: "Applies to", value: "Endpoint", required: true },
    {
      id: "policy-url",
      label: "Address",
      value: "https://example.org/policy",
      browser: { refuses: "not-an-address", accepts: "https://example.org/policy" },
    },
    { id: "policy-secret", label: "Token", secret: true },
  ],
  submit: "Propose the change",
  cancel: "Cancel",
  submitPath: SUBMIT_PATH,
  refusal: REFUSAL,
};

describe("the form contract (T-1730)", () => {
  it("passes a form that meets it", async () => {
    await checkForm(() => <DemoForm broken="none" />, spec);
  });

  it("counts what a form proposes through a callback, with no request to see", async () => {
    // What the KPI card does: it hands its message to the conversation instead of posting it.
    // Without this the counting rules would wait for a request that never comes.
    await checkForm(
      (proposed) => <DemoForm broken="none" propose={proposed} />,
      { ...spec, submitPath: undefined, refusal: undefined },
    );
  });

  it("opens a form that lives behind a button before it holds it to anything", async () => {
    function Behind(): React.JSX.Element {
      const [open, setOpen] = useState(false);
      return open ? <DemoForm broken="none" /> : <Button onClick={() => setOpen(true)}>Add a policy</Button>;
    }
    await checkForm(() => <Behind />, {
      ...spec,
      open: async (user) => {
        await user.click(screen.getByRole("button", { name: "Add a policy" }));
      },
    });
  });

  it("reads the ids off the controls when the spec names none", async () => {
    // What every dialog that mints its ids with `useId` passes: labels and values, no ids.
    await checkForm(() => <DemoForm broken="none" />, {
      ...spec,
      fields: spec.fields.map(({ id: _id, ...field }) => field),
    });
  });

  for (const [broken, rule] of [
    ["label", /visible label/],
    ["required", /required is announced/],
    ["describedby", /aria-describedby/],
    ["empty-submit", /empty submit/],
    ["browser-rule", /own rules answer before the server/],
    ["status-code", /in words/],
    ["lose-input", /in words/],
    ["double-send", /second click/],
    ["discard", /asks first/],
    ["secret", /secret never echoes/],
    ["two-primaries", /one primary button/],
    ["primary-first", /one primary button/],
    ["hostile-html", /hostile text/],
  ] as [Break, RegExp][]) {
    it(`fails a form that breaks "${broken}"`, async () => {
      await expect(checkForm(() => <DemoForm broken={broken} />, spec)).rejects.toThrow(rule);
    });
  }
});
