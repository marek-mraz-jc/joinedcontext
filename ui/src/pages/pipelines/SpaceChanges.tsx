/**
 * "What would change in the space" under the workbench's validation (T-3262): the mapped records
 * against the entities the target space holds now, read with the person's own session through
 * the space surface, so nobody sees more of the space than they may read. Nothing is written.
 */
import { useState } from "react";
import type { JSX } from "react";
import { useTranslation } from "react-i18next";
import { originTransport } from "@joinedcontext/sdk";
import { Alert, Badge, Button, Table, TableBody, TableCell, TableHead, TableHeaderCell, TableRow } from "../../components/ui";
import { READ_PAGE, changesOf, countsOf } from "./spaceChanges";
import type { EntityChange } from "./spaceChanges";

const TONE = { create: "success", update: "warning", unchanged: "neutral" } as const;

/** The entities of `ids` the space holds, by id; an error names the gateway's reason. */
async function currentOf(space: string, ids: string[]): Promise<Map<string, Record<string, unknown>>> {
  const send = originTransport();
  const found = new Map<string, Record<string, unknown>>();
  for (let at = 0; at < ids.length; at += READ_PAGE) {
    const page = ids.slice(at, at + READ_PAGE);
    const answer = await send({
      method: "GET",
      path: `/cs/${encodeURIComponent(space)}/ngsi-ld/v1/entities?id=${page.map(encodeURIComponent).join(",")}&limit=${READ_PAGE}`,
    });
    if (answer.status < 200 || answer.status >= 300) {
      const body = (answer.body ?? {}) as { detail?: unknown; title?: unknown };
      throw new Error(String(body.detail ?? body.title ?? `HTTP ${answer.status}`));
    }
    for (const entity of Array.isArray(answer.body) ? answer.body : []) {
      if (entity && typeof entity === "object" && typeof (entity as { id?: unknown }).id === "string") {
        found.set((entity as { id: string }).id, entity as Record<string, unknown>);
      }
    }
  }
  return found;
}

export function SpaceChanges({ space, records }: { space: string; records: unknown[] }): JSX.Element {
  const { t } = useTranslation();
  const [changes, setChanges] = useState<EntityChange[] | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [reading, setReading] = useState(false);
  const ids = records
    .map((record) => (record && typeof record === "object" ? (record as { id?: unknown }).id : undefined))
    .filter((id): id is string => typeof id === "string");

  const compare = async () => {
    setReading(true);
    setProblem(null);
    try {
      setChanges(changesOf(records, await currentOf(space, [...new Set(ids)])));
    } catch (error) {
      setChanges(null);
      setProblem(t("pipelines.workbench.changes.failed", { reason: error instanceof Error ? error.message : String(error) }));
    } finally {
      setReading(false);
    }
  };

  const counts = changes ? countsOf(changes) : null;
  return (
    <section aria-labelledby="workbench-changes" className="flex flex-col gap-2" data-testid="space-changes">
      <h4 id="workbench-changes" className="text-body font-medium text-fg">
        {t("pipelines.workbench.changes.title", { space })}
      </h4>
      <div className="flex flex-wrap items-center gap-2">
        <Button
          size="sm"
          loading={reading}
          disabled={ids.length === 0}
          disabledReason={ids.length === 0 ? t("pipelines.workbench.changes.noIds") : undefined}
          onClick={() => void compare()}
        >
          {t("pipelines.workbench.changes.compare")}
        </Button>
        <span className="text-caption text-fg-subtle">{t("pipelines.workbench.changes.hint")}</span>
      </div>
      {problem ? (
        <Alert role="alert" tone="danger">
          {problem}
        </Alert>
      ) : null}
      {counts && changes ? (
        <>
          <p role="status" className="text-body">
            {t("pipelines.workbench.changes.counts", counts)}
          </p>
          <Table caption={t("pipelines.workbench.changes.caption", { space })} maxHeight="max-h-80">
            <TableHead>
              <TableHeaderCell>{t("pipelines.workbench.changes.entity")}</TableHeaderCell>
              <TableHeaderCell>{t("pipelines.workbench.changes.outcome")}</TableHeaderCell>
              <TableHeaderCell>{t("pipelines.workbench.changes.what")}</TableHeaderCell>
            </TableHead>
            <TableBody>
              {changes.map((change) => (
                <TableRow key={change.id}>
                  <TableCell className="break-all font-mono text-caption">{change.id}</TableCell>
                  <TableCell>
                    <Badge tone={TONE[change.outcome]}>{t(`pipelines.workbench.changes.outcomes.${change.outcome}`)}</Badge>
                  </TableCell>
                  <TableCell>
                    {change.changes.length === 0 ? (
                      <span className="text-caption text-fg-subtle">—</span>
                    ) : (
                      <ul className="flex flex-col gap-0.5 font-mono text-caption">
                        {change.changes.map((one) => (
                          <li key={`${one.kind}-${one.path}`}>
                            {t(`pipelines.debug.kind.${one.kind}`)} {one.path}
                            {one.kind === "changed" ? `: ${JSON.stringify(one.before)} → ${JSON.stringify(one.after)}` : null}
                            {one.kind === "added" ? `: ${JSON.stringify(one.after)}` : null}
                          </li>
                        ))}
                      </ul>
                    )}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </>
      ) : null}
    </section>
  );
}
