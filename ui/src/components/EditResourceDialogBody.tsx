/** The edit dialog itself, loaded when it opens (T-3316): see EditResourceDialog.tsx. */
import { Suspense, lazy, useMemo, useState } from "react";
import type { JSX, ReactNode } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { parse as parseYaml, stringify as stringifyYaml } from "yaml";
import { api, ApiError, queryKeys, unwrap } from "../api/client";
import { proposeChecked } from "../api/proposal";
import { isChange } from "../api/manifest";
import type { Change, ResourceProposal } from "../api/manifest";
import { ChangeNotice } from "./ChangeNotice";
import { SchemaForm } from "./forms/LazySchemaForm";
import { portalThemeWidgets } from "./forms/theme";
import { arrange, index, mergeUi, paths } from "./forms/uischema";
import { portalWidgets } from "./forms/widgets";
import { shippedForms } from "../schemas/forms";
import { useBranding } from "../branding";
import type { UiSchema } from "./forms/types";
import type { ResourceTarget } from "./DeleteResourceDialog";
import { Alert, Badge, Button, PageFailed, PageLoading } from "./ui";
import { FormFrame, useFormRoute } from "./forms/FormRoute";
import type { EditableForm } from "./EditResourceDialog";
import { describe as describeField, marksOf, parseErrorsOf } from "./yamlManifest";

const MonacoSourceView = lazy(() => import("../pages/models/MonacoSourceView"));

/** The sections in the order `jcctl fmt` writes them (T-5052); the rest after them. */
const SECTIONS = ["apiVersion", "kind", "metadata", "spec"];

/** The manifest as it is written: status is the Portal's to compute and never goes back (MF-04). */
function writable(manifest: Record<string, unknown>): string {
  const rest = { ...manifest };
  delete rest.status;
  const ordered = Object.fromEntries([
    ...SECTIONS.filter((key) => key in rest).map((key) => [key, rest[key]]),
    ...Object.entries(rest).filter(([key]) => !SECTIONS.includes(key)),
  ]);
  return stringifyYaml(ordered, { indentSeq: true, lineWidth: 0 });
}

/** The fields under a native disclosure when a summary reads them first; as they are otherwise. */
function Disclosure({
  folded,
  open,
  label,
  children,
}: {
  folded: boolean;
  open: boolean;
  label: string;
  children: ReactNode;
}): JSX.Element {
  if (!folded) return <>{children}</>;
  return (
    <details open={open} className="rounded-md border border-border">
      <summary className="focus-ring cursor-pointer px-3 py-2 text-body font-medium text-fg">{label}</summary>
      <div className="border-t border-border p-3">{children}</div>
    </details>
  );
}

/**
 * Editing a resource: the kind's form when the page gave one, else its manifest as YAML (AG-77,
 * CC-19), then one `PUT` that opens a change for an approver. The name stays either way; a renamed
 * manifest is a new resource.
 */
export function EditResourceDialogBody({
  target,
  open,
  onOpenChange,
  changed,
  form,
  readOnly = false,
}: {
  target: ResourceTarget;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** The manifest with a change already made, e.g. by the assistant; shown in place of the stored one. */
  changed?: Record<string, unknown> | null;
  /** The kind's form, when its page has one: the fields instead of the YAML (T-2278). */
  form?: EditableForm;
  /**
   * For a person whose role may not propose the kind: the same form, filled and closed, with the
   * reason, instead of a record that does not open (T-2875).
   */
  readOnly?: boolean;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const branding = useBranding();
  const formRoute = useFormRoute();
  const queryClient = useQueryClient();
  const { project, plural, name } = target;
  const home = target.home ?? project;
  const [text, setText] = useState<string | null>(() => (changed ? writable(changed) : null));
  const [edited, setEdited] = useState<Record<string, unknown> | null>(null);
  const [invalid, setInvalid] = useState<string | null>(null);
  const [change, setChange] = useState<Change | null>(null);

  const current = useQuery({
    queryKey: [...queryKeys.list(home, plural), name],
    enabled: open,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}/{name}", {
          params: { path: { project: home, plural, name } },
        }),
      ),
  });
  const source = text ?? (current.data ? writable(current.data as Record<string, unknown>) : "");
  // The kind's schema, for what a field is for on hover (T-5052); the view works without it.
  const kindSchema = useQuery({
    queryKey: ["kind-schema", target.kind],
    enabled: open && form === undefined,
    staleTime: 60 * 60_000,
    retry: false,
    queryFn: async () =>
      unwrap(await api.GET("/api/v1/kinds/{kind}/schema", { params: { path: { kind: target.kind } } })),
  });

  // The help and the examples a new resource's form shows, on its edit as well (T-3220): the
  // edit used to render the bare fields, so changing a project's settings told a person nothing
  // a field was for. Shipped arrangements first, the organization's own after them, as on the
  // create form; every field is shown, since an edit is of what the resource already holds.
  const forms = useQuery({
    queryKey: queryKeys.forms(),
    enabled: open && form !== undefined,
    staleTime: 5 * 60_000,
    retry: false,
    queryFn: async () => unwrap(await api.GET("/api/v1/forms", {})),
  });
  const arrangeable = form ? paths(form.schema).join("\u0000") : "";
  const requiredFields = (form?.schema.required ?? []).join("\u0000");
  const arranged = useMemo<UiSchema | undefined>(() => {
    const manifest = index([...shippedForms, ...(forms.data?.items ?? [])]).forms[target.kind];
    if (!manifest || arrangeable === "") {
      return undefined;
    }
    return arrange(manifest, {
      locale: i18n.language,
      properties: arrangeable.split("\u0000"),
      required: requiredFields === "" ? [] : requiredFields.split("\u0000"),
      widgets: [...Object.keys(portalThemeWidgets), ...Object.keys(portalWidgets)],
      advanced: true,
      examples: { project: home, orgDomain: branding.orgDomain },
    }).uiSchema;
  }, [arrangeable, requiredFields, forms.data, i18n.language, target.kind, home, branding.orgDomain]);
  const uiSchema = arranged ? (mergeUi(arranged, form?.uiSchema) as UiSchema) : form?.uiSchema;

  const propose = useMutation({
    mutationFn: async (body: unknown) =>
      proposeChecked(home, plural, body as ResourceProposal, false),
    onSuccess: (result) => {
      if (isChange(result)) {
        // Routed, the save goes back to the list and the change is shown there (T-2474).
        if (formRoute) {
          formRoute.leave(<ChangeNotice change={result} project={project} />);
          close(false);
        } else {
          setChange(result);
        }
      }
      void queryClient.invalidateQueries({ queryKey: queryKeys.changes(project) });
    },
  });

  /** The name is the resource's path, so a rename is a new resource and not an edit (MF-11). */
  const proposeManifest = (manifest: unknown) => {
    const written = (manifest as { metadata?: { name?: unknown } } | null)?.metadata?.name;
    if (written !== name) {
      setInvalid(t("resourceEdit.renamed", { name }));
      return;
    }
    setInvalid(null);
    propose.mutate(manifest);
  };

  const submit = () => {
    let manifest: unknown;
    try {
      manifest = parseYaml(source);
    } catch (error) {
      setInvalid(error instanceof Error ? error.message : String(error));
      return;
    }
    proposeManifest(manifest);
  };

  // Closing forgets the edit and the answer, so the next opening starts from the stored manifest.
  const close = (next: boolean) => {
    if (!next) {
      setText(null);
      setEdited(null);
      setInvalid(null);
      setChange(null);
      propose.reset();
    }
    onOpenChange(next);
  };

  // What the edit itself was refused for. A manifest that could not be read is its own state
  // below, with the API's sentence and a retry, rather than a red line over a dialog that says
  // "Loading…" for ever (T-1752).
  // Text that does not parse cannot be proposed: the first error, its line and reason, stands in
  // the alert and on the disabled button until the text parses again (T-5052).
  const parseError = form === undefined && !readOnly ? parseErrorsOf(source)[0] : undefined;
  const parseFailure = parseError
    ? t("resourceEdit.parseError", { line: parseError.line, reason: parseError.message })
    : null;
  const failure =
    invalid ??
    parseFailure ??
    (propose.error instanceof ApiError
      ? (propose.error.problem?.detail ?? propose.error.message)
      : propose.error
        ? t("app.error.generic")
        : null);

  const refusals =
    propose.error instanceof ApiError ? (propose.error.problem?.errors ?? []).filter((e): e is string => typeof e === "string") : [];
  const marks = marksOf(source, refusals);
  const yamlView = (readOnlyView: boolean) => (
    <div className="overflow-hidden rounded-md border border-border">
      <Suspense fallback={<PageLoading label={t("models.loadingEditor")} lines={1} className="p-3" />}>
        <MonacoSourceView
          value={source}
          onChange={(next) => {
            setText(next);
            setInvalid(null);
          }}
          onMount={() => undefined}
          height="24rem"
          readOnly={readOnlyView}
          marks={marks}
          describeField={(path) => describeField(kindSchema.data as Record<string, unknown> | undefined, path)}
          ariaLabel={t("resourceEdit.manifest", { name })}
        />
      </Suspense>
    </div>
  );

  return (
    <FormFrame
      open={open}
      onOpenChange={close}
      size="lg"
      // A reader's heading is the record's own name, the reason it is closed beside it (T-5591):
      // "View …" glued to a title read as part of it.
      title={readOnly ? (target.label ?? name) : t("resourceEdit.title", { name: target.label ?? name })}
      description={
        readOnly ? (
          <>
            <Badge tone="neutral">{t("resourceEdit.readOnlyBadge")}</Badge>{" "}
            {t("resourceEdit.readOnly", { kind: target.kind })}
          </>
        ) : (
          t("resourceEdit.lead")
        )
      }
      closeLabel={t("resourceDelete.close")}
      footer={
        change || readOnly ? (
          <Button onClick={() => close(false)}>{t("resourceDelete.close")}</Button>
        ) : form ? (
          // The form has its own submit, and two would be one too many.
          <Button variant="secondary" onClick={() => close(false)}>
            {t("form.cancel")}
          </Button>
        ) : (
          <>
            <Button variant="secondary" onClick={() => close(false)}>
              {t("form.cancel")}
            </Button>
            <Button
              variant="primary"
              loading={propose.isPending}
              disabled={!current.data || parseFailure !== null}
              // Reachable while it is refused, with the reason read out beside it: there is
              // nothing to propose until the stored manifest has arrived (UI-44).
              disabledReason={
                current.data
                  ? (parseFailure ?? undefined)
                  : current.isError
                    ? t("resourceEdit.unreadable", { name })
                    : t("resourceEdit.loading", { name })
              }
              onClick={submit}
            >
              {t("resourceEdit.propose")}
            </Button>
          </>
        )
      }
    >
      {change ? (
        <ChangeNotice change={change} project={project} />
      ) : (
        <div className="flex flex-col gap-4">
          {failure ? (
            <Alert tone="danger" role="alert">
              {failure}
            </Alert>
          ) : null}
          {current.isPending ? (
            // The wait and the failure are two states, not one (T-1752). They used to be the
            // same `current.data ? … : "Loading…"` branch, so a GET that failed left the dialog
            // saying "Loading…" for ever beside a red line, with nothing to press and no way out
            // but closing it — and the word was in no live region, so nobody was told at all.
            <PageLoading label={t("resourceEdit.loading", { name })} lines={1} />
          ) : current.isError ? (
            <PageFailed error={current.error} onRetry={() => void current.refetch()} />
          ) : form ? (
            current.data ? (
              <>
              {form.summary?.(current.data)}
              <Disclosure folded={form.summary !== undefined} open={!readOnly} label={t("resourceEdit.showFields")}>
              <Suspense fallback={<PageLoading label={t("app.loading")} lines={2} />}>
              <SchemaForm<Record<string, unknown>>
                schema={form.schema}
                project={target.home ?? project}
                kind={target.kind}
                // The name is where this manifest lives, so it is read here and changed nowhere.
                uiSchema={{
                  ...uiSchema,
                  name: {
                    ...((uiSchema?.name as Record<string, unknown> | undefined) ?? {}),
                    "ui:readonly": true,
                  },
                  ...(readOnly ? { "ui:submitButtonOptions": { norender: true } } : {}),
                }}
                disabled={readOnly}
                formData={edited ?? form.fromManifest(current.data)}
                submitLabel={t("resourceEdit.propose")}
                submitting={propose.isPending}
                onChange={(next) => {
                  setEdited((next ?? {}) as Record<string, unknown>);
                  setInvalid(null);
                }}
                onSubmit={(next) => proposeManifest(form.toManifest(next, current.data))}
                afterFields={form.afterFields?.(
                  form.toManifest(edited ?? form.fromManifest(current.data), current.data),
                )}
              />
              </Suspense>
              </Disclosure>
              </>
            ) : null
          ) : (
            yamlView(readOnly)
          )}
        </div>
      )}
    </FormFrame>
  );
}
