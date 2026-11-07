import { useEffect, useMemo, useState } from "react";
import { columnKind, fieldOf, format, optionLabel, pointOf, Problem, useAccess, useClient, useSave, useSchema } from "@joinedcontext/sdk";
import type { Cell, Field, LanguageMap, Row } from "@joinedcontext/sdk";

export function parseInput(field: Field, text: string): { value: Cell } | { error: string } {
  if (text.trim() === "") {
    return { value: null };
  }
  if (field.input === "number") {
    const num = Number(text);
    if (Number.isNaN(num)) {
      return { error: "must be a number" };
    }
    if (field.min !== undefined && num < field.min) {
      return { error: `must be at least ${field.min}` };
    }
    if (field.max !== undefined && num > field.max) {
      return { error: `must be at most ${field.max}` };
    }
    return { value: num };
  }
  if (field.input === "checkbox") {
    return { value: text === "true" };
  }
  if (field.input === "geo") {
    const parts = text.trim().split(/[,\s]+/);
    if (parts.length === 2) {
      const lat = Number(parts[0]);
      const lon = Number(parts[1]);
      if (!Number.isNaN(lat) && !Number.isNaN(lon) && lat >= -90 && lat <= 90 && lon >= -180 && lon <= 180) {
        return { value: { type: "Point", coordinates: [lon, lat] } };
      }
    }
    return { error: 'must be "lat, lon"' };
  }
  if (field.input === "select") {
    if (field.options && !field.options.some((option) => option.value === text)) {
      return { error: `must be one of ${field.options.map(optionLabel).join(", ")}` };
    }
    return { value: text };
  }
  if (field.input === "date") {
    return { value: text };
  }
  if (field.pattern) {
    try {
      const re = new RegExp("^(?:" + field.pattern + ")$");
      if (!re.test(text)) {
        return { error: "does not match the expected format" };
      }
    } catch {
      // ignore invalid regex pattern
    }
  }
  return { value: text };
}

function getInitialDraft(
  r: Row | null | undefined,
  names: string[],
  specs: Record<string, Field>,
): Record<string, string> {
  const d: Record<string, string> = {};
  for (const name of names) {
    if (!r || r[name] === null || r[name] === undefined) {
      d[name] = "";
      continue;
    }
    const spec = specs[name];
    if (spec?.input === "geo") {
      const pt = pointOf(r[name]);
      d[name] = pt ? `${pt[1]}, ${pt[0]}` : "";
    } else if (spec?.input === "checkbox") {
      d[name] = r[name] === true ? "true" : "false";
    } else {
      d[name] = format(r[name]);
    }
  }
  return d;
}

export function EntityForm({
  type,
  row,
  fields,
  rows,
  title,
  onSaved,
  onCancel,
}: {
  type: string;
  row?: Row | null;
  fields?: string[];
  rows?: Row[];
  title?: string;
  onSaved?: (id: string) => void;
  onCancel?: () => void;
}): React.JSX.Element {
  const { schema, typeSchema } = useSchema(type);
  const save = useSave();
  const { can } = useAccess();
  const client = useClient();
  const language = client.config.language ?? "en";

  const isEdit = Boolean(row);
  const op = isEdit
    ? can("updateAttrs", type).ok
      ? "updateAttrs"
      : can("updateEntity", type).ok
        ? "updateEntity"
        : "updateAttrs"
    : "createEntity";

  const formDecision = can(op, type);

  const fieldNames = useMemo(() => {
    if (fields && fields.length > 0) return fields;
    if (typeSchema?.properties) {
      return Object.keys(typeSchema.properties).filter((k) => k !== "id" && k !== "type");
    }
    if (row) {
      return Object.keys(row).filter((k) => k !== "id" && k !== "type" && k !== "@context");
    }
    return [];
  }, [fields, typeSchema, row]);

  const fieldSpecs = useMemo(() => {
    const contextRows = rows ?? (row ? [row] : []);
    const map: Record<string, Field> = {};
    for (const name of fieldNames) {
      // The merged schema resolves an enum's `$ref`, and the titles come in the app's language (UI-86).
      map[name] = fieldOf(name, typeSchema ?? undefined, columnKind(contextRows, name), schema ?? undefined, language);
    }
    return map;
  }, [fieldNames, typeSchema, schema, language, rows, row]);

  const [draft, setDraft] = useState<Record<string, string>>(() =>
    getInitialDraft(row, fieldNames, fieldSpecs),
  );
  const [initialDraft, setInitialDraft] = useState<Record<string, string>>(() =>
    getInitialDraft(row, fieldNames, fieldSpecs),
  );
  const [localId, setLocalId] = useState("");
  const [fieldErrors, setFieldErrors] = useState<Record<string, string>>({});

  // A LanguageProperty is edited language by language. A row holds one language of it, and a
  // write replaces the attribute whole, so an edit reads every language first and writes them
  // all back (SDK-07). `null` while that read is outstanding; a failed read leaves it null, and
  // the field cannot be written.
  const languageFields = useMemo(() => fieldNames.filter((name) => fieldSpecs[name]?.input === "language"), [fieldNames, fieldSpecs]);
  const [stored, setStored] = useState<Record<string, Record<string, string>> | null>(row ? null : {});
  const [languageDraft, setLanguageDraft] = useState<Record<string, Record<string, string>>>({});
  const [languageProblem, setLanguageProblem] = useState<Error | null>(null);

  const rowId = row?.id ?? null;
  const { clear } = save;
  const languageKey = languageFields.join(",");
  useEffect(() => {
    setLanguageProblem(null);
    if (!rowId || languageFields.length === 0) {
      setStored({});
      setLanguageDraft({});
      return;
    }
    setStored(null);
    let current = true;
    void Promise.all(languageFields.map(async (name) => [name, await client.entities.languages(rowId, name)] as const))
      .then((entries) => {
        if (!current) return;
        const maps = Object.fromEntries(entries);
        setStored(maps);
        setLanguageDraft(maps);
      })
      .catch((err: unknown) => {
        if (current) setLanguageProblem(err instanceof Error ? err : new Error(String(err)));
      });
    return () => {
      current = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rowId, type, languageKey]);

  /** The languages a field offers: the ones it holds, then the application's own. */
  const languagesOf = (name: string) => [...new Set([...Object.keys(stored?.[name] ?? {}), language])];
  /** The map a write sends: every language kept, the ones emptied dropped. */
  const mapOf = (name: string): LanguageMap | null => {
    const merged = Object.entries({ ...(stored?.[name] ?? {}), ...(languageDraft[name] ?? {}) }).filter(([, text]) => text.trim() !== "");
    return merged.length > 0 ? { languageMap: Object.fromEntries(merged) } : null;
  };
  // Another entity, or a new one: start over. Only its identity counts, so a parent that
  // re-renders or a refusal arriving never wipes what the person typed.
  useEffect(() => {
    const init = getInitialDraft(row, fieldNames, fieldSpecs);
    setDraft(init);
    setInitialDraft(init);
    setLocalId("");
    setFieldErrors({});
    clear();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [rowId, type]);

  // Fields that become known later (the schema loads after the first render) join the draft
  // without touching the fields already there.
  useEffect(() => {
    const init = getInitialDraft(row, fieldNames, fieldSpecs);
    const merge = (d: Record<string, string>) => (fieldNames.every((n) => n in d) ? d : { ...init, ...d });
    setDraft(merge);
    setInitialDraft(merge);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fieldNames]);

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    const errors: Record<string, string> = {};

    for (const name of fieldNames) {
      const spec = fieldSpecs[name];
      if (spec.input === "language") {
        if (spec.required && !mapOf(name)) errors[name] = `${name} is required`;
        continue;
      }
      const text = draft[name] ?? "";
      if (spec.required && text.trim() === "") {
        errors[name] = `${name} is required`;
        continue;
      }
      // A value the edit leaves as it was is not sent, so it is not judged either: a stored value
      // an enum no longer lists must not stop a save of the other fields (UI-86).
      if (text.trim() !== "" && !(isEdit && text === (initialDraft[name] ?? ""))) {
        const parsed = parseInput(spec, text);
        if ("error" in parsed) {
          errors[name] = `${name} ${parsed.error}`;
        }
      }
    }

    setFieldErrors(errors);
    if (Object.keys(errors).length > 0) {
      return;
    }

    if (!row) {
      const attrs: Record<string, Cell | LanguageMap> = {};
      for (const name of fieldNames) {
        const spec = fieldSpecs[name];
        if (spec.input === "language") {
          const map = mapOf(name);
          if (map) attrs[name] = map;
          continue;
        }
        const text = draft[name] ?? "";
        if (text.trim() !== "") {
          const parsed = parseInput(spec, text);
          if ("value" in parsed && parsed.value !== null) {
            attrs[name] = parsed.value;
          }
        }
      }
      const newId = await save.create(type, attrs, localId.trim() || undefined);
      if (newId) {
        onSaved?.(newId);
      }
    } else {
      const patch: Record<string, Cell | LanguageMap> = {};
      let changed = false;
      for (const name of fieldNames) {
        const spec = fieldSpecs[name];
        if (spec.input === "language") {
          const map = mapOf(name);
          // ponytail: emptying every language leaves the attribute as it was; removing an
          // attribute is a deleteAttrs the form does not offer.
          if (map && JSON.stringify(map.languageMap) !== JSON.stringify(stored?.[name] ?? {})) {
            changed = true;
            patch[name] = map;
          }
          continue;
        }
        const text = draft[name] ?? "";
        const prev = initialDraft[name] ?? "";
        if (text !== prev) {
          changed = true;
          const parsed = parseInput(spec, text);
          if ("value" in parsed) {
            patch[name] = parsed.value;
          }
        }
      }
      if (!changed) {
        onSaved?.(row.id);
        return;
      }
      const ok = await save.update(row.id, patch);
      if (ok) {
        onSaved?.(row.id);
      }
    }
  };

  const submitDisabled = !formDecision.ok || save.saving || stored === null;
  const submitTitle = !formDecision.ok ? formDecision.reason : undefined;

  return (
    <form
      className="jc-form"
      noValidate
      aria-label={title ?? (row ? "Edit " + type : "New " + type)}
      onSubmit={(e) => void handleSubmit(e)}
    >
      <div className="jc-form-fields">
        {!row && (
          <label className="jc-field">
            <span>Local id</span>
            <input
              name="localId"
              aria-label="Local id"
              value={localId}
              onChange={(e) => setLocalId(e.target.value)}
            />
          </label>
        )}
        {fieldNames.map((name) => {
          const spec = fieldSpecs[name];
          const val = draft[name] ?? "";
          const fieldDecision = can(op, type, name);
          const disabled = !fieldDecision.ok;
          const reason = disabled ? fieldDecision.reason : undefined;

          let inputElement: React.JSX.Element;
          switch (spec.input) {
            case "language":
              inputElement = (
                <span className="jc-languages">
                  {languagesOf(name).map((lang) => (
                    <input
                      key={lang}
                      type="text"
                      lang={lang}
                      aria-label={`${name} (${lang})`}
                      placeholder={lang}
                      value={languageDraft[name]?.[lang] ?? ""}
                      disabled={disabled || stored === null}
                      title={reason}
                      onChange={(e) =>
                        setLanguageDraft((d) => ({ ...d, [name]: { ...(d[name] ?? {}), [lang]: e.target.value } }))
                      }
                    />
                  ))}
                </span>
              );
              break;
            case "number":
              inputElement = (
                <input
                  type="number"
                  aria-label={name}
                  min={spec.min}
                  max={spec.max}
                  step="any"
                  value={val}
                  disabled={disabled}
                  title={reason}
                  onChange={(e) => setDraft((d) => ({ ...d, [name]: e.target.value }))}
                />
              );
              break;
            case "select": {
              // A stored value the enum does not list stays shown and marked, never replaced.
              const outside = val !== "" && !spec.options?.some((opt) => opt.value === val);
              inputElement = (
                <select
                  aria-label={name}
                  aria-invalid={outside || undefined}
                  value={val}
                  disabled={disabled}
                  title={reason}
                  onChange={(e) => setDraft((d) => ({ ...d, [name]: e.target.value }))}
                >
                  <option value="">—</option>
                  {outside && <option value={val}>{`${val} (not in the list)`}</option>}
                  {spec.options?.map((opt) => (
                    <option key={opt.value} value={opt.value} title={opt.description}>
                      {optionLabel(opt)}
                    </option>
                  ))}
                </select>
              );
              break;
            }
            case "date":
              inputElement = (
                <input
                  type="date"
                  aria-label={name}
                  value={val}
                  disabled={disabled}
                  title={reason}
                  onChange={(e) => setDraft((d) => ({ ...d, [name]: e.target.value }))}
                />
              );
              break;
            case "checkbox":
              inputElement = (
                <input
                  type="checkbox"
                  aria-label={name}
                  checked={val === "true"}
                  disabled={disabled}
                  title={reason}
                  onChange={(e) =>
                    setDraft((d) => ({ ...d, [name]: e.target.checked ? "true" : "false" }))
                  }
                />
              );
              break;
            case "geo":
              inputElement = (
                <input
                  type="text"
                  aria-label={name}
                  placeholder="lat, lon"
                  value={val}
                  disabled={disabled}
                  title={reason}
                  onChange={(e) => setDraft((d) => ({ ...d, [name]: e.target.value }))}
                />
              );
              break;
            default:
              inputElement = (
                <input
                  type="text"
                  aria-label={name}
                  value={val}
                  disabled={disabled}
                  title={reason}
                  onChange={(e) => setDraft((d) => ({ ...d, [name]: e.target.value }))}
                />
              );
              break;
          }

          return (
            <label key={name} className="jc-field">
              <span>
                {name}
                {spec.required ? " *" : ""}
              </span>
              {inputElement}
              {fieldErrors[name] && (
                <p className="jc-field-error" role="alert">
                  {fieldErrors[name]}
                </p>
              )}
            </label>
          );
        })}
      </div>
      {languageProblem && <Problem error={languageProblem} />}
      {save.problem && <Problem error={save.problem} />}
      <div className="jc-form-actions">
        {onCancel && (
          <button type="button" onClick={onCancel}>
            Cancel
          </button>
        )}
        <button type="submit" disabled={submitDisabled} title={submitTitle}>
          {save.saving ? "Saving…" : "Save"}
        </button>
      </div>
    </form>
  );
}
