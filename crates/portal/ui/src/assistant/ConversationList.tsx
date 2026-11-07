import { useState } from "react";
import type { JSX } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, unwrap, reasonOf } from "../api/client";
import { Button, Input } from "../components/ui";
import { ListFailed } from "../components/forms/widgets/ListFailed";

/** How many conversations the list shows before "Show more". */
export const CONVERSATIONS_PER_PAGE = 15;

interface Conversation {
  id: string;
  prompt: string;
  createdAt?: string;
}

/**
 * The person's conversations in this project, beside the full-screen assistant (T-2773, UI-84):
 * newest first, searchable by what was asked, a page at a time. Picking one opens it in place;
 * the conversation open now is marked as the current one.
 */
export function ConversationList({
  project,
  current,
  onOpen,
}: {
  project: string;
  current: string | null;
  onOpen: (runId: string) => void;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const [words, setWords] = useState("");
  const [shown, setShown] = useState(CONVERSATIONS_PER_PAGE);
  const conversations = useQuery({
    queryKey: ["projects", project, "agent-runs", { kind: "conversation", mine: true }],
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/agent-runs", {
          params: { path: { project }, query: { limit: 100, kind: "conversation", mine: true } },
        }),
      ),
  });
  const wanted = words.trim().toLocaleLowerCase(i18n.language);
  const all = (conversations.data?.items ?? []) as Conversation[];
  const matching = wanted === "" ? all : all.filter((one) => one.prompt.toLocaleLowerCase(i18n.language).includes(wanted));

  return (
    <nav aria-label={t("assistant.full.conversations")} className="flex min-h-0 flex-col gap-2">
      <Input
        type="search"
        aria-label={t("assistant.full.search")}
        placeholder={t("assistant.full.search")}
        value={words}
        onChange={(event) => {
          setWords(event.target.value);
          setShown(CONVERSATIONS_PER_PAGE);
        }}
      />
      {conversations.isPending ? (
        <p role="status" className="text-caption text-fg-muted">
          {t("app.loading")}
        </p>
      ) : conversations.isError ? (
        <ListFailed
          what={t("assistant.full.conversations")}
          reason={reasonOf(conversations.error, t("app.error.generic"))}
          onRetry={() => void conversations.refetch()}
        />
      ) : matching.length === 0 ? (
        <p className="text-caption text-fg-muted">
          {wanted === "" ? t("assistant.full.none") : t("assistant.full.noMatch", { words: words.trim() })}
        </p>
      ) : (
        <ul className="-mx-1 min-h-0 flex-1 space-y-0.5 overflow-y-auto px-1">
          {matching.slice(0, shown).map((one) => {
            const asked = one.prompt.trim() === "" ? t("assistant.full.untitled") : one.prompt;
            return (
              <li key={one.id}>
                <Button
                  variant="ghost"
                  size="sm"
                  aria-current={one.id === current ? "true" : undefined}
                  title={asked}
                  onClick={() => {
                    onOpen(one.id);
                  }}
                  className="w-full justify-start text-left aria-[current=true]:bg-primary-soft aria-[current=true]:text-primary-soft-fg"
                >
                  <span className="truncate">{asked}</span>
                </Button>
              </li>
            );
          })}
        </ul>
      )}
      {matching.length > shown ? (
        <Button
          size="sm"
          onClick={() => {
            setShown((count) => count + CONVERSATIONS_PER_PAGE);
          }}
        >
          {t("assistant.full.more", { count: Math.min(CONVERSATIONS_PER_PAGE, matching.length - shown) })}
        </Button>
      ) : null}
    </nav>
  );
}
