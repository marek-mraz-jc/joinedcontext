import { useState } from "react";
import type { FormEvent, JSX } from "react";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ApiError, api, unwrap } from "../../api/client";
import type { Change } from "../../api/manifest";
import { ChangeNotice } from "../../components/ChangeNotice";
import { Alert, Button, Card, Field, Input, PageFailed, PageHeader, PageLoading } from "../../components/ui";

interface Row {
  key: string;
  value: string;
}

/**
 * The secret form (CC-06, MF-24, T-5971): where a person types a credential a plan needs, never
 * in a chat. The value goes encrypted into the project's repository as a change for approval;
 * the page never shows a stored value, only the names and keys the repository declares.
 */
export function SecretFormPage({
  project,
  name: askedName = "",
  secretKey: askedKey = "",
}: {
  project: string;
  name?: string;
  secretKey?: string;
}): JSX.Element {
  const { t } = useTranslation();
  const [name, setName] = useState(askedName);
  const [rows, setRows] = useState<Row[]>([{ key: askedKey, value: "" }]);
  const [problems, setProblems] = useState<string[]>([]);
  const [change, setChange] = useState<Change | null>(null);
  const [checked, setChecked] = useState(false);
  const path = { project };

  const stored = useQuery({
    queryKey: ["secrets", project],
    queryFn: async () => unwrap(await api.GET("/api/v1/projects/{project}/secrets", { params: { path } })),
    retry: false,
  });
  const held = stored.data?.items.find((item) => item.name === name);
  const missingKeys = (held?.keys ?? []).filter((key) => !rows.some((row) => row.key === key));

  const save = useMutation({
    mutationFn: async () => {
      const values = Object.fromEntries(rows.map((row) => [row.key.trim(), row.value]));
      return unwrap(
        await api.POST("/api/v1/projects/{project}/secrets", {
          params: { path },
          body: { name: name.trim(), values },
        }),
      );
    },
    onSuccess: (proposed) => {
      setProblems([]);
      setRows((current) => current.map((row) => ({ ...row, value: "" })));
      setChange(proposed as Change);
    },
    onError: (error: unknown) => {
      const lines = error instanceof ApiError ? (error.problem?.errors ?? []) : [];
      setProblems(lines.length ? lines : [error instanceof Error ? error.message : String(error)]);
    },
  });

  // The heading and its purpose line stay through a wait and a refused read (UI-01, T-6048): a
  // viewer the secrets door refuses still sees which page refused them.
  const header = <PageHeader title={t("secrets.title")} description={t("secrets.description")} />;
  if (stored.isPending) return <PageLoading label={t("secrets.title")} />;
  if (stored.isError)
    return (
      <div className="mx-auto max-w-2xl space-y-4">
        {header}
        <PageFailed error={stored.error} onRetry={() => void stored.refetch()} />
      </div>
    );

  // Each key once and none blank: the values are sent as one map, where a second row of a key
  // would replace the first without a word (F-R2UI4-017).
  const keyProblem = (index: number): string | undefined => {
    const key = rows[index]?.key.trim() ?? "";
    if (!key) return t("secrets.keyMissing");
    return rows.findIndex((row) => row.key.trim() === key) < index ? t("secrets.keyTwice", { key }) : undefined;
  };
  const submit = (event: FormEvent) => {
    event.preventDefault();
    setChecked(true);
    if (rows.some((_, index) => keyProblem(index))) return;
    save.mutate();
  };
  const setRow = (index: number, patch: Partial<Row>) =>
    setRows((current) => current.map((row, i) => (i === index ? { ...row, ...patch } : row)));

  return (
    <div className="mx-auto max-w-2xl space-y-4">
      {header}
      <Alert tone="info">{t("secrets.neverInChat")}</Alert>
      {change ? <ChangeNotice change={change} project={project} /> : null}
      {problems.length > 0 ? (
        <Alert tone="danger" role="alert">
          <ul className="list-disc pl-5">
            {problems.map((line) => (
              <li key={line}>{line}</li>
            ))}
          </ul>
        </Alert>
      ) : null}
      <Card>
        <form className="space-y-4" onSubmit={submit} noValidate>
          <Field id="secret-name" label={t("secrets.name")} required help={t("secrets.hint.name")}>
            <Input
              id="secret-name"
              value={name}
              onChange={(event) => setName(event.target.value)}
              list="secret-names"
              autoComplete="off"
              spellCheck={false}
            />
          </Field>
          <datalist id="secret-names">
            {stored.data.items.map((item) => (
              <option key={item.name} value={item.name} />
            ))}
          </datalist>
          {stored.data.items.length === 0 ? (
            <p className="text-sm text-fg-muted">{t("secrets.empty")}</p>
          ) : null}
          {held ? <p className="text-sm text-fg-muted">{t("secrets.replaces", { keys: held.keys.join(", ") })}</p> : null}
          {rows.map((row, index) => (
            <div key={index} className="grid gap-3 sm:grid-cols-2">
              <Field
                id={`secret-key-${index}`}
                label={t("secrets.key")}
                required
                help={t("secrets.hint.key")}
                errors={checked ? [keyProblem(index)].filter((line): line is string => line !== undefined) : undefined}
              >
                <Input
                  id={`secret-key-${index}`}
                  value={row.key}
                  onChange={(event) => setRow(index, { key: event.target.value })}
                  autoComplete="off"
                  spellCheck={false}
                />
              </Field>
              <Field id={`secret-value-${index}`} label={t("secrets.value")} required help={t("secrets.hint.value")}>
                <Input
                  id={`secret-value-${index}`}
                  type="password"
                  value={row.value}
                  onChange={(event) => setRow(index, { value: event.target.value })}
                  autoComplete="new-password"
                  spellCheck={false}
                />
              </Field>
            </div>
          ))}
          <div className="flex flex-wrap gap-2">
            <Button
              type="button"
              variant="secondary"
              onClick={() =>
                setRows((current) => [...current, ...(missingKeys.length ? missingKeys : [""]).map((key) => ({ key, value: "" }))])
              }
            >
              {missingKeys.length ? t("secrets.addKept") : t("secrets.addKey")}
            </Button>
            <Button type="submit" disabled={save.isPending} disabledReason={t("secrets.sending")}>
              {t("secrets.submit")}
            </Button>
          </div>
        </form>
      </Card>
    </div>
  );
}
