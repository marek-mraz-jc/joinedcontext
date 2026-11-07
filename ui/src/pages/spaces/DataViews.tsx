/**
 * What the space data page keeps beside its one grid (ADR-N-042 §3.2, T-6210): the delete that
 * keeps a copy in the trash, the trash itself, and the notice that someone else changed rows.
 */
import { useEffect, useRef, useState } from "react";
import type { JSX } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { cellText, toRichRow } from "@joinedcontext/sdk";
import portalI18n from "../../i18n";
import { api, unwrap } from "../../api/client";
import type { components } from "../../api/schema";
import type { EntitySource, RichRow } from "@joinedcontext/sdk";
import { Alert, Button } from "../../components/ui";

/** One option of an enum slot, as the model titles it (UI-86). */
export interface EnumChoice {
  value: string;
  title?: string;
}

/** The members a broker writes itself; a restore sends the entity without them. */
const SYSTEM = new Set(["createdAt", "modifiedAt", "deletedAt"]);

/** The entity as a create takes it again: the broker's own timestamps left out, at every level. */
export function writable(entity: Record<string, unknown>): Record<string, unknown> {
  const strip = (value: unknown): unknown => {
    if (Array.isArray(value)) return value.map(strip);
    if (value && typeof value === "object") {
      return Object.fromEntries(
        Object.entries(value as Record<string, unknown>)
          .filter(([key]) => !SYSTEM.has(key))
          .map(([key, inner]) => [key, strip(inner)]),
      );
    }
    return value;
  };
  return strip(entity) as Record<string, unknown>;
}

/**
 * Deletes one row the way §31 orders it (T-3107): the person's copy kept first, then the delete
 * through the gateway with their session; a delete the gateway refuses forgets the copy again and
 * says the gateway's words.
 */
export async function deleteRow(project: string, space: string, source: EntitySource, row: RichRow): Promise<void> {
  if (!source.remove) throw new Error("this view cannot delete");
  const path = { project, space };
  const kept = await api.POST("/api/v1/projects/{project}/spaces/{space}/trash", {
    params: { path },
    // The schema calls `entity` an object of no known members: an NGSI-LD entity is any.
    body: { entity: writable(row.raw) as Record<string, never> },
  });
  if (!kept.data) {
    const detail = (kept.error as { detail?: unknown } | undefined)?.detail;
    throw new Error(typeof detail === "string" ? detail : `HTTP ${kept.response.status}`);
  }
  try {
    await source.remove(row.id);
  } catch (error) {
    const forgot = await api.DELETE("/api/v1/projects/{project}/spaces/{space}/trash/{id}", {
      params: { path: { ...path, id: kept.data.id } },
    });
    if (forgot.response.ok) throw error;
    const reason = error instanceof Error ? error.message : String(error);
    throw new Error(portalI18n.t("spaces.views.trashStray", { reason }));
  }
}

/** The key the trash list of one space is cached under. */
export const trashKey = (project: string, space: string) => ["projects", project, "spaces", space, "trash"];

/**
 * What the person deleted from this space's views (T-3107, API/01 §31): their own copies for 30
 * days, each restored with their own create through the gateway, refused like any write.
 */
export function TrashPanel({
  project,
  space,
  restore,
}: {
  project: string;
  space: string;
  /** The create through the gateway: the entity back in the space under its URN. */
  restore: (entity: Record<string, unknown>) => Promise<void>;
}): JSX.Element | null {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const [refused, setRefused] = useState<string | null>(null);
  const [busy, setBusy] = useState<number | null>(null);
  const items = useQuery({
    queryKey: trashKey(project, space),
    retry: false,
    queryFn: async () => {
      const answer: unknown = await unwrap(
        await api.GET("/api/v1/projects/{project}/spaces/{space}/trash", {
          params: { path: { project, space } },
        }),
      );
      // An answer of another shape is no list: the panel stays away rather than break the page.
      return Array.isArray(answer) ? (answer as components["schemas"]["TrashItem"][]) : [];
    },
  });
  if (!items.data || items.data.length === 0) return null;
  const date = new Intl.DateTimeFormat(i18n.language, { dateStyle: "medium", timeStyle: "short" });
  return (
    <details className="rounded-lg border border-border p-3" data-testid="view-trash">
      <summary className="cursor-pointer text-body font-semibold text-fg">
        {t("spaces.views.trash", { count: items.data.length })}
      </summary>
      <p className="mt-1 text-caption text-fg-muted">{t("spaces.views.trashLead")}</p>
      {refused ? (
        <Alert role="alert" tone="danger" className="mt-2">
          {refused}
        </Alert>
      ) : null}
      <ul className="mt-2 flex flex-col gap-2">
        {items.data.map((item) => {
          const name = cellText(toRichRow(item.entity as Record<string, unknown>, i18n.language).cells.name) || item.urn;
          return (
            <li key={item.id} className="flex flex-wrap items-center justify-between gap-2">
              <span className="min-w-0 [overflow-wrap:anywhere]">
                <span className="font-semibold">{name}</span>{" "}
                <span className="text-caption text-fg-muted">
                  {t("spaces.views.deletedAt", { at: date.format(new Date(item.deletedAt)) })}
                </span>
              </span>
              <Button
                size="sm"
                variant="secondary"
                disabled={busy !== null}
                disabledReason={t("app.loading")}
                aria-label={t("spaces.views.restoreOne", { name })}
                onClick={async () => {
                  setRefused(null);
                  setBusy(item.id);
                  try {
                    await restore(item.entity as Record<string, unknown>);
                    const forgot = await api.DELETE("/api/v1/projects/{project}/spaces/{space}/trash/{id}", {
                      params: { path: { project, space, id: item.id } },
                    });
                    if (!forgot.response.ok) {
                      setRefused(t("spaces.views.restoredStray", { name, reason: `HTTP ${forgot.response.status}` }));
                    }
                    await queryClient.invalidateQueries({ queryKey: trashKey(project, space) });
                    await queryClient.invalidateQueries({ queryKey: ["space-view-rows", space] });
                  } catch (error) {
                    setRefused(
                      t("spaces.views.restoreRefused", {
                        name,
                        reason: error instanceof Error ? error.message : String(error),
                      }),
                    );
                  } finally {
                    setBusy(null);
                  }
                }}
              >
                {t("spaces.views.restore")}
              </Button>
            </li>
          );
        })}
      </ul>
    </details>
  );
}

/**
 * That entities of a type changed (API/01 §32). The event names no entity or attribute: the
 * Portal heard it under its own account, and the read again shows what this person may see (T-4876).
 */
export interface LiveChange {
  type: string;
  /** When this window heard it. */
  at: Date;
}

/**
 * Listens for changes of one type of a space while a view is open (T-3105, API/01 §32). An event
 * says only that the type changed, so `onChange` reads the rows again with the person's own
 * session; the browser's EventSource reconnects by itself. A browser without one hears nothing.
 */
export function useLive(project: string, space: string, type: string, onChange: (change: LiveChange) => void): void {
  const latest = useRef(onChange);
  useEffect(() => {
    latest.current = onChange;
  }, [onChange]);
  useEffect(() => {
    if (!type || typeof EventSource === "undefined") return;
    const source = new EventSource(
      `/api/v1/projects/${encodeURIComponent(project)}/spaces/${encodeURIComponent(space)}/live?type=${encodeURIComponent(type)}`,
    );
    const changed = (event: MessageEvent<string>) => {
      try {
        const change = JSON.parse(event.data) as { type?: unknown };
        if (typeof change.type === "string") latest.current({ type: change.type, at: new Date() });
      } catch {
        // Not JSON: no change to act on.
      }
    };
    source.addEventListener("changed", changed as EventListener);
    return () => {
      source.removeEventListener("changed", changed as EventListener);
      source.close();
    };
  }, [project, space, type]);
}

/** The line a live view shows after others changed rows: when, read again just now. */
export function LiveNotice({ change }: { change: LiveChange | null }): JSX.Element {
  const { t, i18n } = useTranslation();
  return (
    <p role="status" aria-live="polite" className="text-caption text-fg-muted" data-testid="view-live">
      {change ? t("spaces.views.liveChanged", { time: change.at.toLocaleTimeString(i18n.resolvedLanguage ?? i18n.language) }) : ""}
    </p>
  );
}
