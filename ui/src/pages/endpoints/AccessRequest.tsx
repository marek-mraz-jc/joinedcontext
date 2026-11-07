import { useId, useState } from "react";
import type { FormEvent, JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, ApiError, unwrap } from "../../api/client";
import type { components } from "../../api/schema";
import { Combobox } from "../../components/pickers/Combobox";
import { Alert, Button, Field, Input, Textarea } from "../../components/ui";
import { Grant, StateBadge } from "../pipelines/AccessRequests";
import type { AccessRequestView } from "../pipelines/AccessRequests";

/**
 * A person's own access request on an Endpoint (EP-55, PF-58, T-5615). Who reads an Endpoint but
 * gets nothing through it asks the space's owners for a read of the types it serves; the grant is
 * for that person only, and another person, an owner of the space, approves it as a Change.
 */

type PersonRequest = components["schemas"]["PersonRequest"];

/** What the page route answers: the types the server lets this person ask for, and their requests. */
export interface MyAccess {
  types: string[];
  refusal: string | null;
  items: AccessRequestView[];
}

type FieldName = "types" | "until" | "reason";

/** The server's 422 lines, sorted to the field each names (`types[0]: …`, `until: …`). */
export function fieldErrors(
  err: unknown,
): Partial<Record<FieldName, string[]>> {
  const lines =
    err instanceof ApiError && Array.isArray(err.problem?.errors)
      ? err.problem.errors
      : [];
  const out: Partial<Record<FieldName, string[]>> = {};
  for (const line of lines) {
    if (typeof line !== "string") continue;
    const field = (["types", "until", "reason"] as const).find((name) =>
      line.startsWith(name),
    );
    if (field)
      out[field] = [
        ...(out[field] ?? []),
        line.replace(/^[a-z]+(\[\d+\])?:\s*/, ""),
      ];
  }
  return out;
}

/** The body from the form, or the field the person still has to fill in. */
export function personRequestBody(form: {
  types: string[];
  until: string;
  reason: string;
}): { body: PersonRequest } | { missing: FieldName } {
  if (form.types.length === 0) return { missing: "types" };
  if (!form.reason.trim()) return { missing: "reason" };
  return {
    body: {
      types: form.types,
      ...(form.until
        ? { until: new Date(`${form.until}T23:59:59`).toISOString() }
        : {}),
      reason: form.reason.trim(),
    },
  };
}

function problem(err: unknown, fallback: string): string {
  return err instanceof ApiError
    ? (err.problem?.detail ?? err.message)
    : err instanceof Error
      ? err.message
      : fallback;
}

export function EndpointAccessRequest({
  project,
  name,
}: {
  project: string;
  name: string;
}): JSX.Element {
  const { t } = useTranslation();
  const ids = useId();
  const queryClient = useQueryClient();
  const key = ["access-requests", "mine", project, name];
  const [types, setTypes] = useState<string[]>([]);
  const [until, setUntil] = useState("");
  const [reason, setReason] = useState("");
  const [missing, setMissing] = useState<FieldName | null>(null);

  const mine = useQuery({
    queryKey: key,
    retry: false,
    queryFn: async () =>
      (await unwrap(
        await api.GET(
          "/api/v1/projects/{project}/endpoints/{name}/access-requests",
          { params: { path: { project, name } } },
        ),
      )) as MyAccess,
  });
  const file = useMutation({
    mutationFn: async (body: PersonRequest) =>
      unwrap(
        await api.POST(
          "/api/v1/projects/{project}/endpoints/{name}/access-requests",
          { params: { path: { project, name } }, body },
        ),
      ),
    onSuccess: () => {
      setTypes([]);
      setUntil("");
      setReason("");
      void queryClient.invalidateQueries({ queryKey: key });
    },
  });

  if (mine.isPending)
    return (
      <p className="text-caption text-fg-muted">
        {t("endpoints.request.reading")}
      </p>
    );
  if (mine.isError) {
    return (
      <Alert tone="danger" role="alert">
        {problem(mine.error, t("app.error.generic"))}{" "}
        <Button
          size="sm"
          variant="secondary"
          onClick={() => void mine.refetch()}
        >
          {t("endpoints.request.retry")}
        </Button>
      </Alert>
    );
  }
  // A page answered by an older Portal, or a stub, may lack a member: read it as empty.
  const items = mine.data.items ?? [];
  const refusal = mine.data.refusal ?? null;
  const offered = mine.data.types ?? [];
  const open = items.find(
    (request) => request.state === "pending" || request.state === "approved",
  );
  const errors = file.isError ? fieldErrors(file.error) : {};
  const errorOf = (field: FieldName) =>
    missing === field
      ? [t(`endpoints.request.missing.${field}`)]
      : errors[field];

  const submit = (event: FormEvent) => {
    event.preventDefault();
    const built = personRequestBody({ types, until, reason });
    if ("missing" in built) {
      setMissing(built.missing);
      return;
    }
    setMissing(null);
    file.mutate(built.body);
  };

  return (
    <div className="flex flex-col gap-3" data-testid="endpoint-access-request">
      {items.length > 0 ? (
        <ul
          className="flex flex-col gap-2"
          aria-label={t("endpoints.request.yours")}
        >
          {items.map((request) => (
            <li
              key={request.id}
              className="flex flex-col gap-1 rounded-md border border-border p-2"
            >
              <div className="flex flex-wrap items-center gap-2">
                <span className="text-body font-medium text-fg">
                  {t("endpoints.request.yourRequest")}
                </span>
                <StateBadge state={request.state} />
              </div>
              <Grant request={request} />
              {request.state === "approved" ? (
                <p className="text-caption text-fg-muted">
                  {t("endpoints.request.approvedHint")}
                </p>
              ) : null}
            </li>
          ))}
        </ul>
      ) : null}
      {file.isSuccess ? (
        <Alert tone="success" role="status">
          {t("endpoints.request.sent")}
        </Alert>
      ) : null}
      {refusal ? (
        <Alert tone="info">{refusal}</Alert>
      ) : open ? null : offered.length === 0 ? (
        <p className="text-caption text-fg-muted">
          {t("endpoints.request.noTypes")}
        </p>
      ) : (
        <form
          onSubmit={submit}
          noValidate
          aria-labelledby={`${ids}-title`}
          className="flex flex-col gap-3 rounded-md border border-border p-3"
        >
          <h3 id={`${ids}-title`} className="text-body font-semibold text-fg">
            {t("endpoints.request.title")}
          </h3>
          <p className="text-caption text-fg-muted">
            {t("endpoints.request.lead")}
          </p>
          <Field
            id={`${ids}-types`}
            label={t("endpoints.request.types")}
            help={t("endpoints.request.hint.types")}
            errors={errorOf("types")}
            required
          >
            <Combobox
              id={`${ids}-types`}
              label={t("endpoints.request.types")}
              labelled
              multiple
              value={types}
              options={offered.map((type) => ({
                value: type,
                label: type,
                group: t("endpoints.request.types"),
              }))}
              empty={t("endpoints.request.noTypes")}
              onChange={(values) => {
                setTypes(values);
                if (missing === "types") setMissing(null);
              }}
            />
          </Field>
          <Field
            id={`${ids}-until`}
            label={t("endpoints.request.until")}
            help={t("endpoints.request.hint.until")}
            errors={errorOf("until")}
          >
            <Input
              id={`${ids}-until`}
              type="date"
              value={until}
              onChange={(e) => setUntil(e.target.value)}
            />
          </Field>
          <Field
            id={`${ids}-reason`}
            label={t("endpoints.request.reason")}
            help={t("endpoints.request.hint.reason")}
            errors={errorOf("reason")}
            required
          >
            <Textarea
              id={`${ids}-reason`}
              value={reason}
              maxLength={500}
              onChange={(e) => {
                setReason(e.target.value);
                if (missing === "reason") setMissing(null);
              }}
            />
          </Field>
          {file.isError && Object.keys(errors).length === 0 ? (
            <Alert tone="danger" role="alert">
              {problem(file.error, t("app.error.generic"))}
            </Alert>
          ) : null}
          <div>
            <Button
              type="submit"
              disabled={file.isPending}
              disabledReason={t("app.busyReason")}
            >
              {t("endpoints.request.send")}
            </Button>
          </div>
        </form>
      )}
    </div>
  );
}
