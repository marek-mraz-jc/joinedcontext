import type { JSX, ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap, whilePending } from "../api/client";
import { localized } from "../api/manifest";
import type { Manifest } from "../api/manifest";
import { LifecycleBadge } from "../components/status/LifecycleBadge";
import { Button, Card, InlineError, PageHeader, PermissionGuard, Term } from "../components/ui";
import { ResourcePageFailed } from "../components/ui/PageState";
import { fromRegistrationEnvelope, targetOf } from "./RegistrationsPage";

const LINK =
  "text-sm text-primary-soft-fg underline hover:no-underline focus:outline-none focus:ring-2 focus:ring-border-focus";

/** A condition the reconciler set, as the status carries it. */
interface Condition {
  type?: string;
  status?: string;
  message?: string;
  lastTransitionTime?: string;
}

/** The last thing that went wrong writing this registration into its hub's broker, if anything did. */
export function lastError(manifest: Manifest): Condition | undefined {
  const conditions = (manifest.status as { conditions?: Condition[] } | undefined)?.conditions ?? [];
  return conditions.find((condition) => condition.status === "False" && condition.message);
}

function Row({ label, children }: { label: ReactNode; children: ReactNode }): JSX.Element {
  return (
    <div className="grid gap-1 border-b border-border py-3 last:border-b-0 @lg:grid-cols-[12rem_1fr] @lg:gap-4">
      <dt className="text-caption font-medium text-fg-muted">{label}</dt>
      <dd className="min-w-0 break-words text-body">{children}</dd>
    </div>
  );
}

const mono = (items: string[], none: string): ReactNode =>
  items.length > 0 ? <span className="font-mono text-caption">{items.join(", ")}</span> : <span className="text-fg-muted">{none}</span>;

/**
 * One context source registration (T-4819, MF-36, PF-48): which space's broker forwards what to
 * which source, and whether the broker took it. The list said only the space and the types; what a
 * hub's audience reads from a member is decided by the attributes and operations, so they are here.
 */
export function RegistrationPage({ project, name }: { project: string; name: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
  const navigate = useNavigate();
  const registration = useQuery({
    queryKey: queryKeys.resource(project, "csrs", name),
    retry: false,
    refetchInterval: whilePending,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/{plural}/{name}", {
          params: { path: { project, plural: "csrs", name } },
        }),
      ),
  });
  const back = (
    <Link to="/projects/$project/$plural" params={{ project, plural: "csrs" }} className={LINK}>
      {t("registrations.page.back")}
    </Link>
  );

  if (registration.isPending) {
    return <p role="status">{t("app.loading")}</p>;
  }
  if (registration.isError) {
    return (
      <ResourcePageFailed
        title={name}
        description={t("registrations.lead")}
        error={registration.error}
        onRetry={() => {
          void registration.refetch();
        }}
        back={back}
      />
    );
  }

  const manifest = registration.data as Manifest;
  const shape = fromRegistrationEnvelope(manifest);
  const target = targetOf(shape);
  const entities = shape.information.flatMap((entry) => entry.entities ?? []);
  const types = [...new Set(entities.map((entity) => entity.type))];
  const ids = entities.flatMap((entity) => [entity.id, entity.idPattern].filter((v): v is string => Boolean(v)));
  const attributes = [
    ...new Set(shape.information.flatMap((entry) => [...(entry.propertyNames ?? []), ...(entry.relationshipNames ?? [])])),
  ];
  const error = lastError(manifest);
  const none = t("registrations.page.none");

  return (
    <div className="space-y-6">
      {/* One header row: the title with its id and state, Edit on the same row; the breadcrumb leads back. */}
      <PageHeader
        title={localized(manifest.metadata.title, locale, name)}
        titleAction={
          <span className="ms-2 flex flex-wrap items-center gap-2">
            {/* On a phone the breadcrumb already ends with the id. */}
            <code className="hidden font-mono text-caption text-fg-muted sm:inline">{name}</code>
            <LifecycleBadge kind="phase" value={manifest.status?.phase} />
          </span>
        }
        actions={
          <PermissionGuard project={project} kind="ContextSourceRegistration" verb="propose">
            <Button
              onClick={() => {
                void navigate({ to: "/projects/$project/$plural/$name/edit", params: { project, plural: "csrs", name } });
              }}
            >
              {t("registrations.page.edit")}
            </Button>
          </PermissionGuard>
        }
      />

      <Card as="dl" flush className="@container px-4">
        <Row label={t("registrations.page.lastErrorLabel")}>
          {error ? (
            <InlineError>
              {error.message}
              {error.lastTransitionTime ? (
                <span className="block text-caption text-fg-muted">
                  {new Date(error.lastTransitionTime).toLocaleString(locale)}
                </span>
              ) : null}
            </InlineError>
          ) : (
            <span className="text-fg-muted">{none}</span>
          )}
        </Row>
        <Row label={<Term name="contextSpace">{t("registrations.field.space")}</Term>}>
          {shape.contextSpaceRef ? (
            <Link
              to="/projects/$project/$plural/$name"
              params={{ project, plural: "spaces", name: shape.contextSpaceRef }}
              className={LINK}
            >
              {shape.contextSpaceRef}
            </Link>
          ) : (
            none
          )}
        </Row>
        <Row label={t(`registrations.targetKind.${target}`)}>
          {target === "endpointRef" && shape.endpointRef ? (
            <Link
              to="/projects/$project/$plural/$name"
              params={{ project, plural: "endpoints", name: shape.endpointRef }}
              className={LINK}
            >
              {shape.endpointRef}
            </Link>
          ) : (
            <span className="break-all font-mono text-caption">{shape.endpoint ?? none}</span>
          )}
        </Row>
        <Row label={t("registrations.page.types")}>{mono(types, none)}</Row>
        <Row label={t("registrations.page.ids")}>{mono(ids, t("registrations.page.allIds"))}</Row>
        <Row label={t("registrations.page.attributes")}>{mono(attributes, t("registrations.page.allAttributes"))}</Row>
        <Row label={t("registrations.page.operations")}>
          {mono(shape.operations ?? [], t("registrations.page.defaultOperations"))}
        </Row>
        {shape.mode ? <Row label={t("registrations.page.mode")}>{shape.mode}</Row> : null}
        {shape.expiresAt ? <Row label={t("registrations.page.expires")}>{shape.expiresAt}</Row> : null}
      </Card>
    </div>
  );
}
