/**
 * The pages behind an MCP write's question (AG-63, T-5699, T-5829): the person whose AI client
 * asked reads what the call would do and accepts or declines it here with their own session. The
 * client's next call reads this decision; nothing the client says accepts. A Portal call's
 * question lives in a project; a context broker's (T-5829) on the broker, read through the Portal.
 */
import type { JSX, ReactNode } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, ApiError, unwrap } from "../../api/client";
import { Alert, Button, EmptyState, PageHeader, PageLoading, safeHref } from "../../components/ui";
import { ResourcePageFailed } from "../../components/ui/PageState";

type Decision = "accept" | "decline";

interface Asked {
  message: string;
  expiresAt: number;
  decision?: Decision | null;
}

function McpDecision<Q extends Asked>({
  queryKey,
  read,
  decide: send,
  lead,
  rows,
  look,
  home,
}: {
  queryKey: readonly unknown[];
  read: () => Promise<Q>;
  decide: (decision: Decision) => Promise<Q>;
  lead: string;
  /** The facts of the call, label and value, in the order they are shown. */
  rows: (asked: Q) => [string, ReactNode][];
  /** A link to what the call changes, when the Portal has a page of it. */
  look?: (asked: Q) => string;
  home: ReactNode;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const question = useQuery({ queryKey, queryFn: read, retry: false });
  const decide = useMutation({
    mutationFn: send,
    onSuccess: (answered) => queryClient.setQueryData(queryKey, answered),
  });
  const title = t("mcpElicitation.title");
  const header = <PageHeader title={title} description={lead} />;

  if (question.isPending) {
    return (
      <section aria-label={title} className="space-y-6">
        {header}
        <PageLoading label={t("app.loading")} />
      </section>
    );
  }
  if (question.isError) {
    if (question.error instanceof ApiError && question.error.status === 404) {
      // The client's call used up the question this person answered here: say what they answered,
      // not that it is gone (a paired tab re-reads its page after every call, T-6501).
      const answered = decide.data?.decision;
      return (
        <section aria-label={title} className="space-y-6">
          {header}
          {answered ? (
            <Alert
              tone={answered === "accept" ? "success" : "info"}
              title={t(answered === "accept" ? "mcpElicitation.accepted" : "mcpElicitation.declined")}
            >
              {t("mcpElicitation.backToClient")}
            </Alert>
          ) : (
            <EmptyState
              heading={2}
              icon="inbox"
              title={t("mcpElicitation.gone")}
              description={t("mcpElicitation.goneHint")}
            />
          )}
          {home}
        </section>
      );
    }
    return (
      <section aria-label={title}>
        <ResourcePageFailed
          title={title}
          description={lead}
          error={question.error}
          onRetry={() => {
            void question.refetch();
          }}
          back={home}
        />
      </section>
    );
  }

  const asked = question.data;
  const expires = new Date(asked.expiresAt * 1000).toLocaleTimeString(i18n.resolvedLanguage ?? "sk", {
    hour: "2-digit",
    minute: "2-digit",
  });
  return (
    <section aria-label={title} className="space-y-6">
      {header}
      <div className="space-y-4 rounded-lg border border-border p-4">
        <p className="text-body break-words" data-testid="mcp-elicitation-message">
          {asked.message}
        </p>
        <dl className="grid gap-2 text-body sm:grid-cols-[auto_1fr]">
          {rows(asked).map(([label, value]) => (
            <div key={label} className="contents">
              <dt className="text-fg-muted">{label}</dt>
              <dd className="break-words">{value}</dd>
            </div>
          ))}
          <dt className="text-fg-muted">{t("mcpElicitation.expires")}</dt>
          <dd>{expires}</dd>
        </dl>
        {look ? (
          <a href={safeHref(look(asked))} className="focus-ring text-body text-primary-soft-fg underline hover:no-underline">
            {t("mcpElicitation.look")}
          </a>
        ) : null}
      </div>
      {asked.decision ? (
        <Alert
          tone={asked.decision === "accept" ? "success" : "info"}
          title={t(asked.decision === "accept" ? "mcpElicitation.accepted" : "mcpElicitation.declined")}
        >
          {t("mcpElicitation.backToClient")}
        </Alert>
      ) : null}
      {decide.isError ? (
        <Alert tone="danger" title={t("mcpElicitation.failed")}>
          {decide.error.message}
        </Alert>
      ) : null}
      <div className="space-y-2">
        <p className="text-body text-fg-muted">{t("mcpElicitation.hint")}</p>
        <div className="flex flex-wrap gap-3">
          <Button
            variant="primary"
            onClick={() => decide.mutate("accept")}
          >
            {t("mcpElicitation.accept")}
          </Button>
          <Button
            onClick={() => decide.mutate("decline")}
          >
            {t("mcpElicitation.decline")}
          </Button>
        </div>
      </div>
      {home}
    </section>
  );
}

const homeLink = "focus-ring text-body text-primary-soft-fg underline hover:no-underline";

export function McpElicitationPage({ project, id }: { project: string; id: string }): JSX.Element {
  const { t } = useTranslation();
  return (
    <McpDecision
      queryKey={["mcp-elicitation", project, id]}
      read={async () =>
        unwrap(
          await api.GET("/api/v1/projects/{project}/mcp-elicitations/{id}", {
            params: { path: { project, id } },
          }),
        )
      }
      decide={async (decision) =>
        unwrap(
          await api.POST("/api/v1/projects/{project}/mcp-elicitations/{id}/decision", {
            params: { path: { project, id } },
            body: { decision },
          }),
        )
      }
      lead={t("mcpElicitation.lead")}
      rows={(asked) => [[t("mcpElicitation.operation"), <code key="op">{asked.operation}</code>]]}
      look={(asked) => asked.resource}
      home={
        <Link to="/projects/$project/home" params={{ project }} className={homeLink}>
          {t("mcpElicitation.home")}
        </Link>
      }
    />
  );
}

/** A context broker's MCP write (T-5829): its view and tool, decided on the broker by the Portal. */
export function McpConfirmationPage({ id }: { id: string }): JSX.Element {
  const { t } = useTranslation();
  return (
    <McpDecision
      queryKey={["mcp-confirmation", id]}
      read={async () =>
        unwrap(await api.GET("/api/v1/mcp-confirmations/{id}", { params: { path: { id } } }))
      }
      decide={async (decision) =>
        unwrap(
          await api.POST("/api/v1/mcp-confirmations/{id}/decision", {
            params: { path: { id } },
            body: { decision },
          }),
        )
      }
      lead={t("mcpConfirmation.lead")}
      rows={(asked) => [
        [t("mcpConfirmation.view"), <code key="view">{asked.view}</code>],
        [t("mcpConfirmation.tool"), <code key="tool">{asked.tool}</code>],
      ]}
      home={
        <Link to="/" className={homeLink}>
          {t("mcpConfirmation.home")}
        </Link>
      }
    />
  );
}
