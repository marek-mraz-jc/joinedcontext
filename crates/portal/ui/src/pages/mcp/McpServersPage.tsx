import { useMemo, useState } from "react";
import type { JSX } from "react";
import { useQueries, useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link } from "@tanstack/react-router";
import { api, queryKeys, unwrap, reasonOf } from "../../api/client";
import { asManifests, localized, ORG_NAMESPACE } from "../../api/manifest";
import type { Manifest } from "../../api/manifest";
import { useProjects } from "../../api/projects";
import { useBranding } from "../../branding";
import { DeleteResourceDialog } from "../../components/DeleteResourceDialog";
import { namedServerUrl } from "../../components/endpoints/links";
import { Badge, Button, Card, EmptyState, PageFailed, PageHeader, PermissionGuard } from "../../components/ui";
import { McpServerDialog } from "./McpServerDialog";
import { CopyValue, ToolPreview } from "./McpServerPanels";
import { asChoice, clientConfig, clientId, fromManifest, memberKey } from "./mcp";
import type { MemberChoice } from "./mcp";

/**
 * The project's named MCP servers (ADR-N-043, T-3156): one address an AI client configures once
 * over several Endpoints. Each server shows its address, its sign-in client and the entry to paste,
 * its members' health and the tools this person would get; creating, editing and removing one is
 * a Change like every other manifest.
 */
export function McpServersPage({ project, embedded = false }: { project: string; embedded?: boolean }): JSX.Element {
  const { t, i18n } = useTranslation();
  const branding = useBranding();
  const [editing, setEditing] = useState<Manifest | "new" | null>(null);
  const [removing, setRemoving] = useState<Manifest | null>(null);

  const servers = useQuery({
    queryKey: queryKeys.list(project, "mcpservers"),
    queryFn: async () =>
      unwrap(await api.GET("/api/v1/projects/{project}/{plural}", { params: { path: { project, plural: "mcpservers" } } })),
  });
  // The member choices: every Endpoint of every project the person sees, each list already
  // narrowed to what they may read (SP-20), so nobody is offered an Endpoint they could not open.
  const projects = useProjects();
  const endpointLists = useQueries({
    queries: (projects.data ?? []).map((name) => ({
      queryKey: queryKeys.list(name, "endpoints"),
      queryFn: async () =>
        unwrap(await api.GET("/api/v1/projects/{project}/{plural}", { params: { path: { project: name, plural: "endpoints" } } })),
    })),
  });
  const choices = useMemo<MemberChoice[]>(
    () =>
      (projects.data ?? []).flatMap((name, index) =>
        asManifests(endpointLists[index]?.data?.items ?? [])
          .map((endpoint) => asChoice(name, endpoint, localized(endpoint.metadata.title, i18n.language, endpoint.metadata.name)))
          .filter((choice): choice is MemberChoice => choice !== null),
      ),
    [projects.data, endpointLists, i18n.language],
  );
  const choicesLoading = projects.isLoading || endpointLists.some((list) => list.isLoading);
  const items = asManifests(servers.data?.items ?? []);
  const create = (
    <PermissionGuard project={project} kind="McpServer" verb="propose">
      <Button onClick={() => setEditing("new")}>{t("mcp.new")}</Button>
    </PermissionGuard>
  );

  return (
    <section aria-label={t("mcp.title")} className="space-y-6">
      {embedded ? (
        // A tab of the organization's page, under its own h1.
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div>
            <h2 className="text-title font-semibold text-fg">{t("mcp.title")}</h2>
            <p className="text-body text-fg-muted">{t("mcp.leadOrganization")}</p>
          </div>
          {create}
        </div>
      ) : (
        <PageHeader compact title={t("mcp.title")} description={t("mcp.lead")} actions={create} />
      )}
      {servers.isError ? (
        <PageFailed
          error={servers.error}
          onRetry={() => {
            void servers.refetch();
          }}
        >
          {t("mcp.listFailed", { reason: reasonOf(servers.error, t("app.error.generic")) })}
        </PageFailed>
      ) : null}
      {servers.isLoading ? <p role="status">{t("app.loading")}</p> : null}
      {!servers.isLoading && !servers.isError && items.length === 0 ? (
        <EmptyState title={t(project === ORG_NAMESPACE ? "mcp.emptyOrganization" : "mcp.empty")} description={t("mcp.emptyHint")} icon="endpoints" action={create} />
      ) : null}
      {items.map((server) => (
        <ServerCard
          key={server.metadata.name}
          project={project}
          server={server}
          domain={branding.domain}
          choices={choices}
          onEdit={() => setEditing(server)}
          onRemove={() => setRemoving(server)}
        />
      ))}
      {editing !== null ? (
        <McpServerDialog
          key={editing === "new" ? "new" : editing.metadata.name}
          project={project}
          stored={editing === "new" ? undefined : editing}
          choices={choices}
          choicesLoading={choicesLoading}
          open
          onOpenChange={(open) => (open ? undefined : setEditing(null))}
        />
      ) : null}
      {removing ? (
        <DeleteResourceDialog
          target={{ project, kind: "McpServer", plural: "mcpservers", name: removing.metadata.name }}
          open
          onOpenChange={(open) => (open ? undefined : setRemoving(null))}
        />
      ) : null}
    </section>
  );
}

function ServerCard({
  project,
  server,
  domain,
  choices,
  onEdit,
  onRemove,
}: {
  project: string;
  server: Manifest;
  domain: string | undefined;
  choices: MemberChoice[];
  onEdit: () => void;
  onRemove: () => void;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const name = server.metadata.name;
  const form = fromManifest(project, server);
  const byKey = new Map(choices.map((choice) => [memberKey(choice), choice]));
  const members = form.members.map((key) => byKey.get(key)).filter((choice): choice is MemberChoice => Boolean(choice));
  const unseen = form.members.length - members.length;
  const url = namedServerUrl(domain, project, name);
  const titleId = `mcp-server-${name}`;
  return (
    <Card>
      <section aria-labelledby={titleId} className="flex flex-col gap-4">
        <div className="flex flex-wrap items-start justify-between gap-2">
          <div className="flex flex-col gap-1">
            <h2 id={titleId} className="text-title font-semibold">
              {localized(server.metadata.title, i18n.language, name)}
            </h2>
            <p className="flex flex-wrap items-center gap-2 text-caption text-fg-muted">
              <span className="font-mono">{name}</span>
              <Badge tone={form.audience === "public" ? "accent" : "neutral"}>{t(`mcp.audience.${form.audience}`)}</Badge>
              <span>{t("mcp.memberCount", { count: form.members.length })}</span>
            </p>
            {localized(server.metadata.description, i18n.language, "") ? (
              <p>{localized(server.metadata.description, i18n.language, "")}</p>
            ) : null}
          </div>
          <div className="flex flex-wrap gap-2">
            <PermissionGuard project={project} kind="McpServer" verb="propose">
              <Button variant="secondary" size="sm" aria-label={t("mcp.editOf", { name })} onClick={onEdit}>
                {t("mcp.edit")}
              </Button>
            </PermissionGuard>
            <PermissionGuard project={project} kind="McpServer" verb="delete">
              <Button variant="danger" size="sm" aria-label={t("mcp.removeOf", { name })} onClick={onRemove}>
                {t("mcp.remove")}
              </Button>
            </PermissionGuard>
          </div>
        </div>
        <div className="grid gap-3 lg:grid-cols-2">
          <CopyValue label={t("mcp.address")} value={url} />
          <CopyValue label={t("mcp.client")} value={clientId(project, name)} />
        </div>
        <CopyValue label={t("mcp.config")} value={clientConfig(url, name)} code />
        <p className="text-caption text-fg-muted">{t("mcp.signIn", { client: clientId(project, name) })}</p>
        {form.audience === "public" ? (
          // The page a stranger's AI client setup starts from: tools as anonymous callers see them (T-4871).
          <p className="text-caption text-fg-muted">
            <Link to="/connect/$project/$name" params={{ project, name }} className="text-primary-soft-fg underline hover:no-underline">
              {t("mcp.connectPage")}
            </Link>{" "}
            {t("mcp.connectPageHint")}
          </p>
        ) : null}
        {unseen > 0 ? <p className="text-caption text-fg-muted">{t("mcp.form.unseen", { count: unseen })}</p> : null}
        <ToolPreview members={members} />
      </section>
    </Card>
  );
}
