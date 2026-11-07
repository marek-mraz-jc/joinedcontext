import { useEffect, useId, useRef, useState } from "react";
import { Card, format, Header, Page, Split, useAccess, useEntities, useMe, useSave, useSchema } from "@joinedcontext/sdk";
import type { Field, Row } from "@joinedcontext/sdk";
import { Empty, Loading, Problem } from "./components/states";
import { fieldsOf, ownerQuery, problemsOf, STEPS, valuesOf } from "./steps";

const TYPE = "ServiceRequest";

const LABEL: Record<string, string> = {
  category: "Kind of problem",
  title: "Title",
  description: "What is wrong",
  address: "Street address",
  district: "District",
  contactEmail: "Email",
  mayContact: "You may contact me about this request",
};

/** Always shown under the input, so a person knows the rule before breaking it. */
const HINT: Record<string, string> = {
  title: "5 to 80 characters, for example “Pothole at the tram stop”.",
  description: "At least 20 characters: what you saw and since when.",
  address: "The nearest street address or crossing.",
  contactEmail: "Optional. Only used about this request.",
};

const OPTION: Record<string, string> = {
  pothole: "Pothole",
  streetlight: "Streetlight out",
  graffiti: "Graffiti",
  litter: "Litter or full bin",
  other: "Something else",
  received: "Received",
  inProgress: "In progress",
  done: "Done",
};

const MULTILINE = new Set(["description"]);

function word(value: string): string {
  return OPTION[value] ?? value;
}

function Input({
  field,
  value,
  problem,
  onChange,
}: {
  field: Field;
  value: string;
  problem?: string;
  onChange: (text: string) => void;
}): React.JSX.Element {
  const id = `field-${field.name}`;
  const described = [HINT[field.name] ? `${id}-hint` : "", problem ? `${id}-problem` : ""].filter(Boolean).join(" ") || undefined;
  const common = { id, "aria-invalid": problem ? true : undefined, "aria-describedby": described, "aria-required": field.required || undefined };
  const label = `${LABEL[field.name] ?? field.name}${field.required ? "" : " (optional)"}`;

  if (field.input === "checkbox") {
    return (
      <div className="app-field app-field-check">
        <input {...common} type="checkbox" checked={value === "true"} onChange={(event) => onChange(event.target.checked ? "true" : "")} />
        <label htmlFor={id}>{LABEL[field.name] ?? field.name}</label>
      </div>
    );
  }
  return (
    <div className="app-field">
      <label htmlFor={id}>{label}</label>
      {HINT[field.name] && (
        <p id={`${id}-hint`} className="app-hint">
          {HINT[field.name]}
        </p>
      )}
      {field.input === "select" ? (
        <select {...common} value={value} onChange={(event) => onChange(event.target.value)}>
          <option value="">Choose…</option>
          {field.options?.map((option) => (
            <option key={option.value} value={option.value} title={option.description}>
              {option.title ?? word(option.value)}
            </option>
          ))}
        </select>
      ) : MULTILINE.has(field.name) ? (
        <textarea {...common} rows={5} value={value} onChange={(event) => onChange(event.target.value)} />
      ) : (
        <input
          {...common}
          type={field.name === "contactEmail" ? "email" : "text"}
          autoComplete={field.name === "contactEmail" ? "email" : field.name === "address" ? "street-address" : undefined}
          value={value}
          onChange={(event) => onChange(event.target.value)}
        />
      )}
      {problem && (
        <p id={`${id}-problem`} className="app-problem">
          {LABEL[field.name] ?? field.name} {problem}.
        </p>
      )}
    </div>
  );
}

/** The person's own requests, newest first, with where each one stands. */
function MyRequests({ rows, loading }: { rows: Row[]; loading: boolean }): React.JSX.Element {
  if (loading && rows.length === 0) return <Loading label="Loading your requests…" />;
  if (rows.length === 0) return <Empty>You have not sent a request yet.</Empty>;
  const newest = [...rows].sort((a, b) => String(b.dateSubmitted ?? "").localeCompare(String(a.dateSubmitted ?? "")));
  return (
    <ul className="app-requests">
      {newest.map((row) => (
        <li key={row.id}>
          <span className="app-request-title">{String(row.title ?? "Untitled request")}</span>
          <span className="app-status" data-status={String(row.status ?? "")}>
            {word(String(row.status ?? "")) || "—"}
          </span>
          <span className="app-hint">
            {word(String(row.category ?? ""))} · sent {format(row.dateSubmitted, "date")}
          </span>
        </li>
      ))}
    </ul>
  );
}

/** A request in four steps, checked against the endpoint's schema at each one, and the person's own requests beside it. */
export default function App(): React.JSX.Element {
  const me = useMe();
  const { typeSchema, error: schemaError } = useSchema(TYPE);
  const { can } = useAccess();
  const save = useSave();
  const query = me ? ownerQuery(me.id) : null;
  const mine = useEntities(TYPE, query ? { q: query } : undefined, { enabled: query !== null });
  const [at, setAt] = useState(0);
  const [draft, setDraft] = useState<Record<string, string>>({});
  const [problems, setProblems] = useState<Record<string, string>>({});
  const [sent, setSent] = useState<string | null>(null);
  const summary = useRef<HTMLDivElement>(null);
  const stepHeading = useRef<HTMLHeadingElement>(null);
  const moved = useRef(false);
  const titleId = useId();

  // Focus the new step's heading after a move, never on the first render.
  useEffect(() => {
    if (moved.current) stepHeading.current?.focus();
  }, [at]);

  if (schemaError) {
    return (
      <Shell>
        <Problem error={schemaError} />
      </Shell>
    );
  }
  if (!typeSchema) {
    // The rules come with the schema: no form before it.
    return (
      <Shell>
        <Loading label="Loading the form…" />
      </Shell>
    );
  }

  const fields = fieldsOf(typeSchema);
  const step = STEPS[at];
  const stepFields = step.fields.map((name) => fields[name]);
  const allFields = STEPS.flatMap((s) => s.fields).map((name) => fields[name]);
  const allowed = can("createEntity", TYPE);
  const blocked = !me ? "Sign in to send a request and to see your own." : !allowed.ok ? allowed.reason : undefined;

  const go = (to: number) => {
    moved.current = true;
    setProblems({});
    setAt(to);
  };

  const next = () => {
    const found = problemsOf(stepFields, draft);
    setProblems(found);
    if (Object.keys(found).length > 0) {
      requestAnimationFrame(() => summary.current?.focus());
      return;
    }
    go(at + 1);
  };

  const send = async () => {
    if (!me) return;
    const found = problemsOf(allFields, draft);
    const first = STEPS.findIndex((s) => s.fields.some((name) => found[name]));
    if (first >= 0) {
      go(first);
      setProblems(found);
      return;
    }
    const id = await save.create(TYPE, {
      ...valuesOf(allFields, draft),
      submittedBy: me.id,
      dateSubmitted: new Date().toISOString(),
      status: "received",
    });
    if (!id) return;
    setSent(id.split(":").at(-1) ?? id);
    setDraft({});
    go(0);
    mine.reload();
  };

  const listed = Object.entries(problems);

  return (
    <Shell>
      <Split ratio="2:1">
        <Card title="Your request" label="Your request">
          <ol className="app-steps" aria-label="Steps">
            {STEPS.map((s, index) => (
              <li key={s.id} aria-current={index === at ? "step" : undefined} data-done={index < at || undefined}>
                <span className="app-step-number" aria-hidden="true">
                  {index + 1}
                </span>
                <span>{s.title}</span>
              </li>
            ))}
          </ol>
          {sent && (
            <p className="app-sent" role="status">
              Sent. Your reference is {sent}; it is listed under your requests.
            </p>
          )}
          <form
            noValidate
            aria-labelledby={titleId}
            onSubmit={(event) => {
              event.preventDefault();
              if (step.fields.length > 0) next();
              else void send();
            }}
          >
            <h2 id={titleId} ref={stepHeading} tabIndex={-1}>
              Step {at + 1} of {STEPS.length}: {step.title}
            </h2>
            {listed.length > 0 && (
              <div className="app-summary" role="alert" tabIndex={-1} ref={summary}>
                <p>Please fix {listed.length === 1 ? "this" : `these ${listed.length}`} before going on:</p>
                <ul>
                  {listed.map(([name, problem]) => (
                    <li key={name}>
                      <a
                        href={`#field-${name}`}
                        onClick={(event) => {
                          event.preventDefault();
                          document.getElementById(`field-${name}`)?.focus();
                        }}
                      >
                        {LABEL[name] ?? name} {problem}
                      </a>
                    </li>
                  ))}
                </ul>
              </div>
            )}
            {step.fields.length > 0 ? (
              <div className="app-fields">
                {stepFields.map((field) => (
                  <Input
                    key={field.name}
                    field={field}
                    value={draft[field.name] ?? ""}
                    problem={problems[field.name]}
                    onChange={(text) => {
                      setSent(null);
                      setDraft((before) => ({ ...before, [field.name]: text }));
                    }}
                  />
                ))}
              </div>
            ) : (
              <dl className="app-review">
                {STEPS.slice(0, -1).map((s, index) =>
                  s.fields.map((name) => (
                    <div key={name}>
                      <dt>{LABEL[name] ?? name}</dt>
                      <dd>
                        <span>{name === "mayContact" ? (draft[name] === "true" ? "Yes" : "No") : word(draft[name] ?? "") || "—"}</span>
                        <button type="button" className="app-link" onClick={() => go(index)} aria-label={`Change ${LABEL[name] ?? name}`}>
                          Change
                        </button>
                      </dd>
                    </div>
                  )),
                )}
              </dl>
            )}
            <Problem error={save.problem} />
            {blocked && step.fields.length === 0 && (
              <p className="app-note" role="note">
                {blocked}
              </p>
            )}
            <div className="app-actions">
              {at > 0 && (
                <button type="button" onClick={() => go(at - 1)}>
                  Back
                </button>
              )}
              {step.fields.length > 0 ? (
                <button type="submit" className="app-primary">
                  Next
                </button>
              ) : (
                <button type="submit" className="app-primary" disabled={blocked !== undefined || save.saving} title={blocked}>
                  {save.saving ? "Sending…" : "Send request"}
                </button>
              )}
            </div>
          </form>
        </Card>
        <Card title="Your requests" label="Your requests">
          {!me ? <p className="app-hint">Sign in to see the requests you sent.</p> : !query ? <p className="app-hint">Your account id cannot be used to list your requests.</p> : (
            <>
              <Problem error={mine.error} onRetry={mine.reload} />
              <MyRequests rows={mine.rows} loading={mine.loading} />
            </>
          )}
        </Card>
      </Split>
    </Shell>
  );
}

function Shell({ children }: { children: React.ReactNode }): React.JSX.Element {
  return (
    <div className="jc-shell">
      <main className="jc-main">
        <Page label="Report a problem">
          <Header level={1} title="Report a problem in the street" subtitle="Tell us what is wrong and where; follow what happens to it" />
          {children}
        </Page>
      </main>
    </div>
  );
}
