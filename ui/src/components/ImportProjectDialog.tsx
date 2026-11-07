import { useId, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys } from "../api/client";
import type { Change } from "../api/manifest";
import { ChangeNotice } from "./ChangeNotice";
import { nameProblem } from "./layout/NewProject";
import { Alert, Button, buttonClass, Dialog, Field, FilePicker, Input, Select } from "./ui";

/** One parameter `project.yaml` declares (CC-88). */
export interface Declaration {
  type: "string" | "integer" | "number" | "boolean" | "secret";
  default?: string | number | boolean;
  description?: string;
  enum?: (string | number | boolean)[];
}

/**
 * What the dry run of a git import answers: the repositories, the declarations, and what the
 * import does with each organization model the archive carries (MF-50).
 */
interface Plan {
  repositories: { name: string; role: string; repository: string; head: string }[];
  parameters: Record<string, Declaration>;
  models?: { name: string; version: string; action: "map" | "land" }[];
}

/**
 * The values the form holds, as the registry entry takes them: a field left empty takes the
 * declaration's default and is not sent, a number field is a number, a secret is a name.
 */
export function parameterValues(
  declarations: Record<string, Declaration>,
  typed: Record<string, string>,
): Record<string, string | number | boolean> {
  const values: Record<string, string | number | boolean> = {};
  for (const [name, declaration] of Object.entries(declarations)) {
    const raw = (typed[name] ?? "").trim();
    if (raw === "") {
      continue;
    }
    if (declaration.type === "integer" || declaration.type === "number") {
      values[name] = Number(raw);
    } else if (declaration.type === "boolean") {
      values[name] = raw === "true";
    } else {
      values[name] = raw;
    }
  }
  return values;
}

/** Whether a typed value is one a number parameter refuses: `Number("abc")` is NaN, sent as null. */
function notANumber(declaration: Declaration, raw: string): boolean {
  const value = raw.trim();
  if (value === "" || (declaration.type !== "integer" && declaration.type !== "number")) return false;
  const number = Number(value);
  return !Number.isFinite(number) || (declaration.type === "integer" && !Number.isInteger(number));
}

/** The number parameters whose typed value is not a number of their kind, refused before the import (T-6230). */
export function numberProblems(declarations: Record<string, Declaration>, typed: Record<string, string>): string[] {
  return Object.entries(declarations)
    .filter(([name, declaration]) => notANumber(declaration, typed[name] ?? ""))
    .map(([name]) => name);
}

/**
 * One field per declared parameter (CC-88): a select for an enum or a boolean, a text field
 * otherwise, each with its description, its default and, for a secret, that it takes a name.
 * An empty field keeps the default; `parameterValues` turns what was typed into values.
 */
export function ParameterFields({
  idPrefix,
  declarations,
  typed,
  onChange,
}: {
  idPrefix: string;
  declarations: [string, Declaration][];
  typed: Record<string, string>;
  onChange: (typed: Record<string, string>) => void;
}): JSX.Element {
  const { t } = useTranslation();
  return (
    <>
      {declarations.map(([parameter, declaration]) => {
        const id = `${idPrefix}-${parameter}`;
        const help = [
          declaration.description,
          declaration.type === "secret" ? t("projectImport.secretHint") : undefined,
          declaration.default === undefined
            ? undefined
            : t("projectImport.default", { value: String(declaration.default) }),
        ]
          .filter(Boolean)
          .join(" ");
        const choices =
          declaration.type === "boolean"
            ? ["true", "false"]
            : (declaration.enum ?? []).map((value) => String(value));
        return (
          <Field
            key={parameter}
            id={id}
            label={parameter}
            help={help || undefined}
            errors={notANumber(declaration, typed[parameter] ?? "") ? [t(declaration.type === "integer" ? "projectImport.wholeNumber" : "projectImport.number")] : undefined}
          >
            {choices.length > 0 ? (
              <Select
                id={id}
                value={typed[parameter] ?? ""}
                onChange={(event) => onChange({ ...typed, [parameter]: event.target.value })}
              >
                <option value="">{t("projectImport.useDefault")}</option>
                {choices.map((choice) => (
                  <option key={choice} value={choice}>
                    {choice}
                  </option>
                ))}
              </Select>
            ) : (
              <Input
                id={id}
                inputMode={declaration.type === "integer" || declaration.type === "number" ? "decimal" : undefined}
                value={typed[parameter] ?? ""}
                onChange={(event) => onChange({ ...typed, [parameter]: event.target.value })}
              />
            )}
          </Field>
        );
      })}
    </>
  );
}

async function post(
  project: string,
  file: File,
  displayName: string,
  parameters: Record<string, string | number | boolean>,
  dryRun: boolean,
): Promise<unknown> {
  const body = new FormData();
  body.set("file", file);
  body.set("parameters", JSON.stringify(parameters));
  if (displayName.trim() !== "") {
    body.set("displayName", displayName.trim());
  }
  // Through the typed client (UI-07), which adds the csrf header. The document types a
  // multipart body as a string; the FormData passes through so the browser writes the boundary.
  const { data, error, response } = await api.POST("/api/v1/projects/{project}/import", {
    params: { path: { project }, query: dryRun ? { format: "git", dryRun: "All" } : { format: "git" } },
    body: body as unknown as string,
  });
  if (data === undefined) {
    const answered = (error ?? {}) as { detail?: string; title?: string };
    throw new Error(answered.detail ?? answered.title ?? `HTTP ${response.status}`);
  }
  return data;
}

/**
 * Importing a project from its git export (MF-45, MF-46, CC-88, T-2644): the archive and the new
 * slug are checked first, which answers the repositories it creates and the parameters the
 * project declares; the form drawn from them sets this deployment's values; then the same check
 * with those values and the import, which proposes the registry entry.
 */
export function ImportProjectDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const ids = useId();
  const queryClient = useQueryClient();
  const [file, setFile] = useState<File | null>(null);
  const [name, setName] = useState("");
  const [displayName, setDisplayName] = useState("");
  const [plan, setPlan] = useState<Plan | null>(null);
  const [typed, setTyped] = useState<Record<string, string>>({});
  const [change, setChange] = useState<Change | null>(null);
  const problem = nameProblem(name);

  const check = useMutation({
    mutationFn: async () => {
      if (!file) {
        throw new Error(t("projectImport.noFile"));
      }
      return (await post(name, file, displayName, {}, true)) as Plan;
    },
    onSuccess: (answered) => setPlan(answered),
  });

  const land = useMutation({
    mutationFn: async () => {
      if (!file || !plan) {
        throw new Error(t("projectImport.noFile"));
      }
      const wrong = numberProblems(plan.parameters, typed);
      if (wrong.length > 0) {
        throw new Error(t("projectImport.notNumber", { names: wrong.join(", ") }));
      }
      const values = parameterValues(plan.parameters, typed);
      // The dry run is the check the import is held to, over these very values (PF-57).
      await post(name, file, displayName, values, true);
      return (await post(name, file, displayName, values, false)) as Change;
    },
    onSuccess: (result) => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.projects() });
      setChange(result);
    },
  });

  const close = (next: boolean) => {
    if (!next) {
      setFile(null);
      setName("");
      setDisplayName("");
      setPlan(null);
      setTyped({});
      setChange(null);
      check.reset();
      land.reset();
    }
    onOpenChange(next);
  };

  const failure = (check.error ?? land.error)?.message ?? null;
  const declarations = Object.entries(plan?.parameters ?? {});

  return (
    <Dialog
      open={open}
      onOpenChange={close}
      size="lg"
      title={t("projectImport.title")}
      description={t("projectImport.lead")}
      closeLabel={t("resourceDelete.close")}
      footer={
        change ? (
          <Button onClick={() => close(false)}>{t("resourceDelete.close")}</Button>
        ) : (
          <>
            <Button variant="secondary" onClick={() => close(false)}>
              {t("form.cancel")}
            </Button>
            {plan ? (
              <Button variant="primary" loading={land.isPending} onClick={() => land.mutate()}>
                {t("projectImport.import")}
              </Button>
            ) : (
              <Button
                variant="primary"
                loading={check.isPending}
                disabled={problem !== null || file === null}
                disabledReason={
                  file === null
                    ? t("projectImport.noFile")
                    : problem === null
                      ? undefined
                      : t("projects.nameNeeded")
                }
                onClick={() => check.mutate()}
              >
                {t("projectImport.check")}
              </Button>
            )}
          </>
        )
      }
    >
      {change ? (
        <ChangeNotice change={change} project={name} />
      ) : (
        <div className="flex flex-col gap-4">
          {failure ? (
            <Alert tone="danger" role="alert">
              {failure}
            </Alert>
          ) : null}
          <div className="flex flex-col gap-1">
            {plan === null ? (
              <FilePicker label={t("projectImport.file")} accept=".zip,application/zip" onFile={setFile}>
                <span className={buttonClass("secondary", "sm")}>{t("projectImport.file")}</span>
              </FilePicker>
            ) : null}
            <p className="text-caption text-fg-muted">
              {file ? t("projectImport.chosen", { name: file.name }) : t("projectImport.fileHint")}
            </p>
          </div>
          <Field
            id={`${ids}-name`}
            label={t("projects.nameLabel")}
            help={t("projects.nameHint")}
            errors={
              problem === "label"
                ? [t("projects.nameInvalid")]
                : problem === "reserved"
                  ? [t("projects.nameReserved")]
                  : undefined
            }
            required
          >
            <Input
              id={`${ids}-name`}
              value={name}
              autoComplete="off"
              spellCheck={false}
              disabled={plan !== null}
              title={plan !== null ? t("projects.planLocked") : undefined}
              onChange={(event) => setName(event.target.value)}
            />
          </Field>
          <Field id={`${ids}-display`} label={t("projects.displayNameLabel")}>
            <Input
              id={`${ids}-display`}
              value={displayName}
              disabled={plan !== null}
              title={plan !== null ? t("projects.planLocked") : undefined}
              onChange={(event) => setDisplayName(event.target.value)}
            />
          </Field>
          {plan ? (
            <section className="flex flex-col gap-3" aria-labelledby={`${ids}-plan`}>
              <h3 id={`${ids}-plan`} className="text-body font-semibold text-fg">
                {t("projectImport.repositories")}
              </h3>
              <ul className="list-disc pl-5 text-body text-fg">
                {plan.repositories.map((repository) => (
                  <li key={repository.repository}>
                    {t("projectImport.repository", {
                      repository: repository.repository,
                      head: repository.head.slice(0, 7),
                    })}
                  </li>
                ))}
              </ul>
              {plan.models?.length ? (
                <>
                  <h3 className="text-body font-semibold text-fg">{t("projectImport.models")}</h3>
                  <ul className="list-disc pl-5 text-body text-fg">
                    {plan.models.map((model) => (
                      <li key={model.name}>
                        {t(model.action === "map" ? "projectImport.modelMap" : "projectImport.modelLand", {
                          name: model.name,
                          version: model.version,
                        })}
                      </li>
                    ))}
                  </ul>
                </>
              ) : null}
              <h3 className="text-body font-semibold text-fg">{t("projectImport.parameters")}</h3>
              {declarations.length === 0 ? (
                <p className="text-body text-fg-muted">{t("projectImport.noParameters")}</p>
              ) : (
                <ParameterFields
                  idPrefix={`${ids}-param`}
                  declarations={declarations}
                  typed={typed}
                  onChange={setTyped}
                />
              )}
            </section>
          ) : null}
        </div>
      )}
    </Dialog>
  );
}
