/**
 * Comments on an entity and the notifications their mentions send (T-3106, API/01 §35). A person
 * who may read the space comments and names colleagues with `@identifier`; only a person who may
 * read the space is notified. A comment changes no data, and only its author removes it.
 */
import { useId, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, unwrap } from "../../api/client";
import type { components } from "../../api/schema";
import { Alert, Button, Icon, Textarea } from "../../components/ui";
import { sameOriginPath } from "../../components/ui/safeHref";
import { Menu, MenuContent, MenuItem, MenuLabel, MenuTrigger } from "../../components/ui/Menu";

type CommentView = components["schemas"]["CommentView"];
type Commented = components["schemas"]["Commented"];
type Notifications = components["schemas"]["Notifications"];

type NotificationItem = Notifications["items"][number];

/** What one notification says: a mention, or a connector event (T-5979) by its kind. */
export function notificationText(item: NotificationItem, t: (key: string, values?: Record<string, string>) => string): string {
  switch (item.kind) {
    case "change.opened":
      return t("notifications.changeOpened", { name: item.authorName, change: item.urn, project: item.project, connector: item.excerpt });
    case "connector.pending":
      return t("notifications.connectorPending", { name: item.authorName, connector: item.excerpt });
    case "connector.expiring":
      return t("notifications.connectorExpiring", { connector: item.excerpt });
    default:
      return t("notifications.mentioned", { name: item.authorName, space: item.space });
  }
}

/** Where one notification leads: a connector event's own Portal page, a mention's space. */
export function notificationHref(item: NotificationItem): string {
  // Only a Portal path: the server writes it, and a link never leaves the Portal.
  const own = item.link ? sameOriginPath(item.link) : undefined;
  if (own) return own;
  return `/projects/${encodeURIComponent(item.project)}/spaces/${encodeURIComponent(item.space)}`;
}

/** The key one entity's comments are cached under. */
export const commentsKey = (project: string, space: string, urn: string) => ["projects", project, "spaces", space, "comments", urn];

/** The key the signed-in person's notifications are cached under. */
export const NOTIFICATIONS_KEY = ["notifications"];

/** How long a comment may be, as the API takes it. */
const MAX_TEXT = 4000;

function when(at: string, language: string): string {
  const date = new Date(at);
  return Number.isNaN(date.getTime()) ? at : date.toLocaleString(language, { dateStyle: "medium", timeStyle: "short" });
}

function problem(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function CommentsPanel({ project, space, urn }: { project: string; space: string; urn: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const id = useId();
  const queryClient = useQueryClient();
  const key = commentsKey(project, space, urn);
  const [text, setText] = useState("");
  const [unknown, setUnknown] = useState<string[]>([]);

  const comments = useQuery({
    queryKey: key,
    retry: false,
    queryFn: async () =>
      (await unwrap(
        await api.GET("/api/v1/projects/{project}/spaces/{space}/comments", {
          params: { path: { project, space }, query: { urn } },
        }),
      )) as CommentView[],
  });

  const post = useMutation({
    mutationFn: async (body: string) =>
      (await unwrap(
        await api.POST("/api/v1/projects/{project}/spaces/{space}/comments", {
          params: { path: { project, space } },
          body: { urn, text: body },
        }),
      )) as Commented,
    onSuccess: async (made) => {
      setText("");
      setUnknown(made.unknownMentions);
      await queryClient.invalidateQueries({ queryKey: key });
    },
  });

  const remove = useMutation({
    mutationFn: async (comment: number) => {
      await unwrap(
        await api.DELETE("/api/v1/projects/{project}/spaces/{space}/comments/{id}", {
          params: { path: { project, space, id: comment } },
        }),
      );
    },
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: key });
    },
  });

  const trimmed = text.trim();
  return (
    <section aria-labelledby={`${id}-heading`} className="flex flex-col gap-2 border-t border-border pt-3">
      <h3 id={`${id}-heading`} className="text-body font-semibold">
        {t("spaces.comments.title")}
      </h3>
      {comments.isPending ? <p role="status">{t("app.loading")}</p> : null}
      {comments.isError ? (
        <Alert role="alert" tone="danger">
          {problem(comments.error)}
        </Alert>
      ) : null}
      {comments.data && comments.data.length === 0 ? <p className="text-body text-fg-muted">{t("spaces.comments.none")}</p> : null}
      {comments.data && comments.data.length > 0 ? (
        <ol className="flex flex-col gap-2" aria-label={t("spaces.comments.title")}>
          {comments.data.map((comment) => (
            <li key={comment.id} className="rounded-md border border-border bg-surface-muted p-2">
              <p className="flex flex-wrap items-baseline justify-between gap-2 text-caption text-fg-muted">
                <span>
                  <span className="font-semibold text-fg">{comment.authorName}</span> · {when(comment.createdAt, i18n.language)}
                </span>
                {comment.mine ? (
                  <Button
                    size="xs"
                    variant="ghost"
                    aria-label={t("spaces.comments.removeOne", { when: when(comment.createdAt, i18n.language) })}
                    disabled={remove.isPending}
                    disabledReason={t("app.busyReason")}
                    onClick={() => remove.mutate(comment.id)}
                  >
                    {t("spaces.comments.remove")}
                  </Button>
                ) : null}
              </p>
              <p className="whitespace-pre-wrap text-body [overflow-wrap:anywhere]">{comment.text}</p>
            </li>
          ))}
        </ol>
      ) : null}
      <form
        className="flex flex-col gap-2"
        onSubmit={(event) => {
          event.preventDefault();
          if (trimmed) post.mutate(trimmed);
        }}
      >
        <label htmlFor={`${id}-text`} className="text-caption font-medium text-fg-muted">
          {t("spaces.comments.label")}
        </label>
        <Textarea
          id={`${id}-text`}
          value={text}
          maxLength={MAX_TEXT}
          rows={3}
          aria-describedby={`${id}-help`}
          onChange={(event) => setText(event.target.value)}
        />
        <p id={`${id}-help`} className="text-caption text-fg-muted">
          {t("spaces.comments.help")}
        </p>
        {post.isError ? (
          <Alert role="alert" tone="danger">
            {problem(post.error)}
          </Alert>
        ) : null}
        {remove.isError ? (
          <Alert role="alert" tone="danger">
            {problem(remove.error)}
          </Alert>
        ) : null}
        {unknown.length > 0 ? (
          <Alert role="status" tone="warning">
            {t("spaces.comments.unknown", { names: unknown.map((name) => `@${name}`).join(", ") })}
          </Alert>
        ) : null}
        <Button type="submit" className="w-fit" disabled={!trimmed || post.isPending} disabledReason={post.isPending ? t("app.busyReason") : t("spaces.comments.writeFirst")}>
          {t("spaces.comments.post")}
        </Button>
      </form>
    </section>
  );
}

/**
 * The signed-in person's notifications in the header: the unread count on the button, the latest
 * in its menu; opening one marks it read and goes to its space.
 */
const ALERT_NOTICES_KEY = ["alerts", "notices"];
const AGENT_EVENTS = new Set<string>(["refusalSpike", "spendShare", "offHoursWrite"]);

export function NotificationsMenu({ project, waiting = 0 }: { project: string; waiting?: number }): JSX.Element {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const inbox = useQuery({
    queryKey: NOTIFICATIONS_KEY,
    retry: false,
    // A mention is not urgent: a minute is soon enough, and the menu reads again when opened.
    refetchInterval: 60_000,
    queryFn: async () => (await unwrap(await api.GET("/api/v1/notifications", {}))) as Notifications,
  });
  const read = useMutation({
    mutationFn: async (id: number) => {
      await unwrap(await api.POST("/api/v1/notifications/{id}/read", { params: { path: { id } } }));
    },
    onSettled: async () => {
      await queryClient.invalidateQueries({ queryKey: NOTIFICATIONS_KEY });
    },
  });
  // The alerts the person chose (API/01 §37, T-3261), beside the mentions.
  const alerts = useQuery({
    queryKey: ALERT_NOTICES_KEY,
    retry: false,
    refetchInterval: 60_000,
    queryFn: async () => unwrap(await api.GET("/api/v1/alerts/notices", {})),
  });
  const readAlert = useMutation({
    mutationFn: async (id: number) => {
      await unwrap(await api.POST("/api/v1/alerts/notices/{id}/read", { params: { path: { id } } }));
    },
    onSettled: async () => {
      await queryClient.invalidateQueries({ queryKey: ALERT_NOTICES_KEY });
    },
  });
  const muteAlert = useMutation({
    mutationFn: async (subscription: number) => {
      await unwrap(await api.POST("/api/v1/alerts/{id}/mute", { params: { path: { id: subscription } }, body: { for: "1d" } }));
    },
  });
  const notices = alerts.data?.items ?? [];
  // What waits beside the mentions: the project's changes this person may decide (T-3273).
  const unread = (inbox.data?.unread ?? 0) + (alerts.data?.unread ?? 0) + waiting;
  const items = inbox.data?.items ?? [];
  return (
    <Menu
      onOpenChange={(open) => {
        if (open) {
          void inbox.refetch();
          void alerts.refetch();
        }
      }}
    >
      <MenuTrigger asChild>
        <Button variant="ghost" className="relative pointer-coarse:max-sm:px-1.5" aria-label={t("notifications.label", { count: unread })}>
          <Icon name="inbox" className="size-5" />
          {unread > 0 ? (
            <span aria-hidden="true" className="absolute -right-0.5 -top-0.5 min-w-4 rounded-full bg-danger px-1 text-center text-caption font-bold text-danger-fg">
              {unread > 99 ? "99+" : unread}
            </span>
          ) : null}
        </Button>
      </MenuTrigger>
      <MenuContent align="end" className="w-80 max-w-full">
        <MenuItem onSelect={() => void navigate({ to: "/projects/$project/inbox", params: { project } })}>
          <span className="font-semibold">{t("inbox.open", { count: unread })}</span>
        </MenuItem>
        <MenuLabel>{t("notifications.title")}</MenuLabel>
        {inbox.isError ? <p className="px-2.5 py-1.5 text-body text-danger">{problem(inbox.error)}</p> : null}
        {items.length === 0 && !inbox.isError ? <p className="px-2.5 py-1.5 text-body text-fg-muted">{t("notifications.none")}</p> : null}
        {items.map((item) => (
          <MenuItem
            key={item.id}
            className="flex-col items-start gap-0.5"
            onSelect={() => {
              if (!item.read) read.mutate(item.id);
              void navigate({ href: notificationHref(item) });
            }}
          >
            <span className={item.read ? "text-fg-muted" : "font-semibold"}>{notificationText(item, t)}</span>
            {item.kind === "mention" ? (
              <span className="line-clamp-2 text-caption text-fg-muted [overflow-wrap:anywhere]">{item.excerpt}</span>
            ) : null}
            <span className="text-caption text-fg-subtle">{when(item.createdAt, i18n.language)}</span>
          </MenuItem>
        ))}
        {notices.length > 0 ? <MenuLabel>{t("alerts.notices")}</MenuLabel> : null}
        {notices.map((notice) => (
          <div key={`alert-${notice.id}`} className="flex flex-col">
            <MenuItem
              className="flex-col items-start gap-0.5"
              onSelect={() => {
                if (!notice.read) readAlert.mutate(notice.id);
                // An agent's alert opens the agent's Activity tab (T-5301), a pipeline's the pipelines.
                void navigate({
                  href: AGENT_EVENTS.has(notice.event)
                    ? `/organization/agents/org/AgentProfile/${encodeURIComponent(notice.pipeline)}#activity`
                    : `/projects/${encodeURIComponent(notice.project)}/pipelines`,
                });
              }}
            >
              <span className={notice.read ? "text-fg-muted" : "font-semibold"}>
                {t(`alerts.notice.${notice.change}`, { pipeline: notice.pipeline, event: t(`alerts.event.${notice.event}`) })}
              </span>
              {notice.detail ? (
                <span className="line-clamp-2 text-caption text-fg-muted [overflow-wrap:anywhere]">{notice.detail}</span>
              ) : null}
              <span className="text-caption text-fg-subtle">{when(notice.createdAt, i18n.language)}</span>
            </MenuItem>
            <MenuItem className="pl-6 text-caption" onSelect={() => muteAlert.mutate(notice.subscription)}>
              {t("alerts.muteDay", { pipeline: notice.pipeline })}
            </MenuItem>
          </div>
        ))}
      </MenuContent>
    </Menu>
  );
}
