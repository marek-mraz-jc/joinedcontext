import type { JSX } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { Link } from "@tanstack/react-router";
import { reasonOf } from "../../api/client";
import { doorAction, type RestAnswer } from "../../api/odata";
import { Alert, Button, InlineError } from "../../components/ui";

/**
 * Moves the project's charts saved one by one into one "Saved charts" dashboard (T-7161). A check
 * runs first and says what the step would do; the step it names is then one click: propose the
 * dashboard, or, once it is merged, remove the charts the merged dashboard draws. Every chart it
 * does not draw stays, with the reason.
 */
export function SavedChartsMove({ project }: { project: string }): JSX.Element {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const run = useMutation({
    mutationFn: async (dryRun: boolean) =>
      (await doorAction("portal_grafana_builder_saved_charts", { body: { dryRun } }, project)) as RestAnswer<"builder_saved_charts">,
    onSuccess: async (answer) => {
      if (!answer.dryRun) await queryClient.invalidateQueries({ queryKey: ["projects", project, "grafana"] });
    },
  });
  const checked = run.data?.dryRun ? run.data : undefined;
  const done = run.data && !run.data.dryRun ? run.data : undefined;
  return (
    <section aria-labelledby="saved-charts-move" className="mt-3 flex flex-col gap-2 border-t border-border pt-3">
      <h3 id="saved-charts-move" className="text-body font-semibold text-fg">
        {t("grafana.move.title")}
      </h3>
      <p className="text-body text-fg-muted">{t("grafana.move.lead")}</p>
      {checked ? (
        <div role="status" className="flex flex-col gap-1 text-body text-fg">
          <p>
            {checked.stage === "propose"
              ? t("grafana.move.willPropose", { before: checked.before, panels: checked.panels })
              : t("grafana.move.willRemove", { count: checked.moved.length })}
          </p>
          {checked.kept.length ? (
            <>
              <p className="text-fg-muted">{t("grafana.move.stay", { count: checked.kept.length })}</p>
              <ul className="list-disc pl-5 text-fg-muted">
                {checked.kept.map((kept) => (
                  <li key={kept.id}>
                    {kept.title}: {kept.reason}
                  </li>
                ))}
              </ul>
            </>
          ) : null}
        </div>
      ) : null}
      {done ? (
        <Alert tone="success" title={done.change ? t("grafana.move.proposedTitle") : t("grafana.move.removed", { count: done.moved.length })}>
          {done.change ? (
            <Link to="/projects/$project/approvals/$id" params={{ project, id: done.change.metadata.name }} className="underline">
              {t("grafana.move.openChange")}
            </Link>
          ) : (
            t("grafana.move.removedLead")
          )}
        </Alert>
      ) : null}
      {run.isError ? <InlineError>{reasonOf(run.error)}</InlineError> : null}
      <div className="flex flex-wrap gap-2">
        <Button variant="secondary" size="sm" onClick={() => run.mutate(true)} disabled={run.isPending}>
          {t("grafana.move.check")}
        </Button>
        {checked && checked.moved.length > 0 ? (
          <Button variant="primary" size="sm" onClick={() => run.mutate(false)} disabled={run.isPending}>
            {checked.stage === "propose" ? t("grafana.move.propose") : t("grafana.move.remove", { count: checked.moved.length })}
          </Button>
        ) : null}
      </div>
    </section>
  );
}
