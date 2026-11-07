import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { ApiError, api, queryKeys, unwrap } from "../../api/client";
import { useAuth } from "../../auth/AuthProvider";
import { Badge, Button } from "../ui";
import { useWorkspace } from "./WorkspaceContext";

/** A day in the reader's own language; the raw value when the API sent something unparseable. */
function onDay(value: string, locale: string): string {
  const parsed = new Date(value);
  return Number.isNaN(parsed.getTime()) ? value : parsed.toLocaleDateString(locale);
}

export function WorkspaceBar({ project }: { project: string }): React.JSX.Element | null {
  const { t, i18n } = useTranslation();
  const { name, leave } = useWorkspace();
  const { identity } = useAuth();
  // The language the person reads the Portal in, not the browser's: a Slovak Portal wrote
  // 9/27/2026 because `toLocaleDateString` was called with no locale at all (UI-15).
  const locale = i18n.resolvedLanguage ?? i18n.language;

  const workspace = useQuery({
    queryKey: [...queryKeys.list(project, "workspaces"), name],
    enabled: !!name,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/workspaces/{name}", {
          params: { path: { project, name: name! } },
        }),
      ),
    retry: false,
  });

  if (!name) return null;

  // Gone (404), expired, or not the caller's to see (403): one line and the way out (UI-61).
  //
  // `status`, not the bar's own `region`: this line appears while the person is reading
  // something else on the page — the copy expired under them, or was discarded by its owner —
  // and a landmark nobody is inside is announced to nobody (T-1254, UI-15). The bar that is
  // simply there stays a landmark: a live region wrapping the links and the counter would read
  // the whole bar out again every time the count changed, and read it without its links.
  const notice = (message: string) => (
    <div
      role="status"
      aria-label={t("workspaces.bar.label")}
      className="flex flex-wrap items-center gap-3 border-b border-warning/30 bg-warning-soft px-4 py-2 text-sm"
    >
      <span>{message}</span>
      <Button size="sm" variant="ghost" onClick={leave}>
        {t("workspaces.bar.leave")}
      </Button>
    </div>
  );
  if (workspace.error instanceof ApiError && workspace.error.status === 403) {
    return notice(t("workspaces.bar.refused", { reason: workspace.error.message }));
  }
  if (workspace.isError) return notice(t("workspaces.bar.gone"));

  // UI-61: the copy's name is known before its record is read, so the bar is there from the first
  // frame, busy, instead of appearing late and pushing the page down under a person who had not
  // yet seen that they are in a copy (T-1489). A disabled query (no name) never gets here.
  if (!workspace.data) {
    return (
      <div
        role="region"
        aria-label={t("workspaces.bar.label")}
        aria-busy="true"
        data-testid="workspace-bar-loading"
        className="flex flex-wrap items-center gap-3 border-b border-primary/30 bg-primary-soft px-4 py-2 text-sm"
      >
        <span className="font-medium">{t("workspaces.bar.loading", { name })}</span>
        <div className="ml-auto flex items-center gap-2">
          <Button size="sm" variant="ghost" onClick={leave}>
            {t("workspaces.bar.leave")}
          </Button>
        </div>
      </div>
    );
  }

  const { title, name: wsName, createdAt, expiresAt, owner, changes } = workspace.data;
  // Expiry is judged at the moment the copy was read, so the render stays pure.
  if (new Date(expiresAt).getTime() <= workspace.dataUpdatedAt) {
    return notice(t("workspaces.bar.expired", { date: onDay(expiresAt, locale) }));
  }
  const mine = !!identity && (owner === identity.email || owner === identity.username);
  const display = title ?? wsName;
  const date = onDay(createdAt, locale);
  const changeCount = changes ?? 0;

  return (
    <div
      role="region"
      aria-label={t("workspaces.bar.label")}
      className="flex flex-wrap items-center gap-3 border-b border-primary/30 bg-primary-soft px-4 py-2 text-sm"
    >
      <span className="font-medium">
        {t("workspaces.bar.message", { title: display, date })}
      </span>
      <span className="text-fg-muted" data-testid="workspace-changes">
        {t("workspaces.bar.changes", { count: changeCount })}
      </span>
      {mine ? null : (
        <Badge data-testid="workspace-foreign">{t("workspaces.bar.foreign", { owner })}</Badge>
      )}
      <div className="ml-auto flex items-center gap-2">
        <Link
          to="/projects/$project/workspaces/$name/try-it"
          params={{ project, name: wsName }}
          className="focus-ring rounded-md px-2 py-1 text-body underline hover:no-underline"
        >
          {t("workspaces.bar.tryIt")}
        </Link>
        <Link
          to="/projects/$project/workspaces/$name/compare"
          params={{ project, name: wsName }}
          className="focus-ring rounded-md px-2 py-1 text-body underline hover:no-underline"
        >
          {t("workspaces.bar.compare")}
        </Link>
        {mine && changeCount > 0 ? (
          <Link
            to="/projects/$project/workspaces/$name/bring-back"
            params={{ project, name: wsName }}
            className="focus-ring rounded-md px-2 py-1 text-body underline hover:no-underline"
          >
            {t("workspaces.bar.bringBack")}
          </Link>
        ) : null}
        <Button size="sm" variant="ghost" onClick={leave}>
          {t("workspaces.bar.leave")}
        </Button>
      </div>
    </div>
  );
}
