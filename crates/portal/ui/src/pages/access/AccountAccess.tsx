import { useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { api, queryKeys, unwrap } from "../../api/client";
import type { components } from "../../api/schema";
import type { Change } from "../../api/manifest";
import { ChangeNotice } from "../../components/ChangeNotice";
import {
  Badge,
  Button,
  ConfirmDialog,
  InlineError,
  Skeleton,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeaderCell,
  TableRow,
  TableRowHeaderCell,
} from "../../components/ui";
import { PageFailed } from "../../components/ui/PageState";

type AccountAccessAnswer = components["schemas"]["AccountAccess"];
type AccountGrant = components["schemas"]["AccountGrant"];
type GrantRule = components["schemas"]["AccountRule"];

/** The columns of the matrix; `approve` is there to say no account ever holds it (PF-58). */
export const MATRIX_VERBS = ["read", "propose", "delete", "approve"] as const;
export type MatrixVerb = (typeof MATRIX_VERBS)[number];

export interface MatrixCell {
  allowed: boolean;
  /** The grants that give it: "role · scope". */
  sources: string[];
  /** Why not, when not: an ended or not yet started grant, or no grant at all. */
  reason?: { key: string; values?: Record<string, string> };
}

function covers(rule: GrantRule, kind: string, verb: MatrixVerb): boolean {
  const named = rule.kinds.includes(kind) || rule.kinds.includes("*");
  // `propose` on a kind implies `read` on it (jc-core `Rule::grants`).
  return named && (rule.verbs.includes(verb) || (verb === "read" && rule.verbs.includes("propose")));
}

/**
 * The kind × verb matrix of one account (AG-130): each cell from the grants in force, with the
 * grant it comes from, or why it is refused. `approve` is never held by an account (PF-58).
 */
export function accessMatrix(grants: AccountGrant[]): { kind: string; cells: Record<MatrixVerb, MatrixCell> }[] {
  const kinds = [...new Set(grants.flatMap((grant) => grant.rules.flatMap((rule) => rule.kinds ?? [])))].sort();
  return kinds.map((kind) => {
    const cells = {} as Record<MatrixVerb, MatrixCell>;
    for (const verb of MATRIX_VERBS) {
      if (verb === "approve") {
        cells[verb] = { allowed: false, sources: [], reason: { key: "access.agent.reason.neverApproves" } };
        continue;
      }
      const giving = grants.filter((grant) => grant.rules.some((rule) => covers(rule, kind, verb)));
      const active = giving.filter((grant) => grant.state === "active");
      const source = (grant: AccountGrant) => `${grant.role} · ${grant.scope}`;
      if (active.length > 0) {
        cells[verb] = { allowed: true, sources: active.map(source) };
        continue;
      }
      const ended = giving.find((grant) => grant.state === "expired");
      const waiting = giving.find((grant) => grant.state === "pending");
      cells[verb] = {
        allowed: false,
        sources: [],
        reason: ended
          ? { key: "access.agent.reason.ended", values: { grant: source(ended), when: ended.notAfter ?? "" } }
          : waiting
            ? { key: "access.agent.reason.starts", values: { grant: source(waiting), when: waiting.notBefore ?? "" } }
            : { key: "access.agent.reason.noGrant" },
      };
    }
    return { kind, cells };
  });
}

const STATE_TONE = { active: "success", pending: "info", expired: "warning" } as const;

/**
 * What one ServiceAccount may do in the Portal, where each right comes from, and revoke now.
 * `level` keeps the heading order of the page it sits in: h4 in a ServiceAccount card, h2 on an
 * agent's Access tab.
 */
export function AccountAccess({
  project,
  account,
  level = 4,
}: {
  project: string;
  account: string;
  level?: 2 | 4;
}): JSX.Element {
  const Heading = level === 2 ? "h2" : "h4";
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const [revoking, setRevoking] = useState<AccountGrant | null>(null);
  const [change, setChange] = useState<Change | null>(null);
  const key = [...queryKeys.list(project, "serviceaccounts"), account, "access"];
  const access = useQuery({
    queryKey: key,
    queryFn: async (): Promise<AccountAccessAnswer> =>
      unwrap(
        await api.GET("/api/v1/projects/{project}/serviceaccounts/{name}/access", {
          params: { path: { project, name: account } },
        }),
      ),
  });
  const revoke = useMutation({
    mutationFn: async (grant: AccountGrant): Promise<Change> =>
      unwrap(
        await api.POST("/api/v1/projects/{project}/serviceaccounts/{name}/revoke", {
          params: { path: { project, name: account } },
          body: { index: grant.index, role: grant.role },
        }),
      ) as unknown as Change,
    onSuccess: (done) => {
      setChange(done);
      setRevoking(null);
      void queryClient.invalidateQueries({ queryKey: queryKeys.list(project, "serviceaccounts") });
    },
  });

  if (access.isPending) {
    return (
      <div role="status" aria-busy="true" aria-label={t("app.loading")} className="space-y-2">
        <Skeleton className="h-5 w-40" />
        <Skeleton className="h-20" />
      </div>
    );
  }
  if (access.isError) {
    return <PageFailed error={access.error} onRetry={() => void access.refetch()} />;
  }
  const { grants, delegates, mayRevoke } = access.data;
  const rows = accessMatrix(grants);
  const when = (iso: string | null | undefined) =>
    iso ? new Date(iso).toLocaleString(i18n.language, { dateStyle: "medium", timeStyle: "short" }) : "";
  const heading = `access-${account}`;

  return (
    <section className="space-y-3" aria-labelledby={heading}>
      <Heading id={heading} className="font-medium text-fg">
        {t("access.agent.title")}
      </Heading>
      {delegates ? <p className="text-body text-fg-muted">{t("access.agent.delegates")}</p> : null}
      {change ? <ChangeNotice change={change} project={project} /> : null}
      {revoke.isError ? (
        <InlineError>{revoke.error instanceof Error ? revoke.error.message : t("access.agent.revokeFailed")}</InlineError>
      ) : null}

      <ul className="space-y-2" aria-label={t("access.agent.grants")}>
        {grants.map((grant) => (
          <li key={`${grant.index}-${grant.role}`} className="flex flex-wrap items-center gap-2">
            <span className="font-medium text-fg">{grant.role}</span>
            <span className="text-fg-muted">{grant.scope}</span>
            <Badge tone={STATE_TONE[grant.state]}>{t(`access.agent.state.${grant.state}`)}</Badge>
            {grant.notAfter ? (
              <span className="text-caption text-fg-muted">
                {t(grant.state === "expired" ? "access.agent.ended" : "access.agent.ends", { when: when(grant.notAfter) })}
              </span>
            ) : null}
            {mayRevoke && grants.length > 1 ? (
              <Button
                variant="secondary"
                size="sm"
                onClick={() => setRevoking(grant)}
                aria-label={t("access.agent.revokeNamed", { role: grant.role, scope: grant.scope })}
              >
                {t("access.agent.revoke")}
              </Button>
            ) : null}
          </li>
        ))}
      </ul>

      {rows.length === 0 ? (
        <p className="text-body text-fg-muted">{t("access.agent.gatewayOnly")}</p>
      ) : (
        <Table caption={t("access.agent.matrix", { account })}>
            <TableHead>
              <TableHeaderCell scope="col">{t("access.agent.kind")}</TableHeaderCell>
              {MATRIX_VERBS.map((verb) => (
                <TableHeaderCell key={verb} scope="col">
                  {t(`access.agent.verb.${verb}`)}
                </TableHeaderCell>
              ))}
            </TableHead>
            <TableBody>
              {rows.map((row) => (
                <TableRow key={row.kind}>
                  <TableRowHeaderCell>{row.kind === "*" ? t("access.agent.everyKind") : row.kind}</TableRowHeaderCell>
                  {MATRIX_VERBS.map((verb) => {
                    const cell = row.cells[verb];
                    return (
                      <TableCell key={verb}>
                        <span className={cell.allowed ? "font-medium text-success" : "text-fg-muted"}>
                          {t(cell.allowed ? "access.agent.allowed" : "access.agent.refused")}
                        </span>
                        <span className="block text-caption text-fg-muted">
                          {cell.allowed
                            ? cell.sources.join(", ")
                            : t(cell.reason?.key ?? "access.agent.reason.noGrant", {
                                ...cell.reason?.values,
                                when: when(cell.reason?.values?.when),
                              })}
                        </span>
                      </TableCell>
                    );
                  })}
                </TableRow>
              ))}
            </TableBody>
        </Table>
      )}

      <ConfirmDialog
        open={revoking !== null}
        onOpenChange={(open) => (open ? null : setRevoking(null))}
        title={t("access.agent.revokeTitle", { role: revoking?.role ?? "", account })}
        description={t("access.agent.revokeHelp")}
        confirmLabel={t("access.agent.revoke")}
        pending={revoke.isPending}
        onConfirm={() => (revoking ? revoke.mutate(revoking) : undefined)}
      />
    </section>
  );
}
