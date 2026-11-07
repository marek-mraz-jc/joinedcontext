import { useId, useRef, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ApiError, queryKeys, reasonOf } from "../../api/client";
import { useChosenOrganizationName } from "../../api/projects";
import { proposeChecked } from "../../api/proposal";
import { asManifests, isChange, localized, ORG_NAMESPACE } from "../../api/manifest";
import type { Change, Manifest, ResourceProposal } from "../../api/manifest";
import { takePrefill } from "../../assistant/state";
import { ChangeNotice } from "../../components/ChangeNotice";
import { DeleteResourceAction } from "../../components/DeleteResourceDialog";
import { FormRecordLink } from "../../components/RecordLink";
import { EditResourceAction } from "../../components/EditResourceDialog";
import { FormFrame, useCreateForm, useFormRoute } from "../../components/forms/FormRoute";
import { dns1123 } from "../../components/endpoints/sharing";
import {
  Alert,
  Button,
  ConfirmDialog,
  EmptyState,
  Field,
  Input,
  PageFailed,
  Select,
  Table,
  TableBody,
  TableCell,
  TableEmpty,
  TableHead,
  TableHeaderCell,
  TableRow,
  TableSkeleton,
} from "../../components/ui";
import { PermissionGuard } from "../../components/ui/PermissionGuard";
import { useReadableList } from "../../api/permissions";
import { ResourceNamePicker } from "../../components/pickers/ResourceNamePicker";
import { useOrganizationName } from "../../api/projects";
import { PersonPicker } from "../../components/pickers/PersonPicker";
import { isPersonId, personLabel, usePersonNames } from "../../api/personNames";

interface Scope {
  organization?: string;
  project?: string;
  contextSpace?: string;
}

interface BindingSpec {
  subjects?: { user?: string; group?: string }[];
  role?: string;
  scope?: Scope;
  validity?: { notAfter?: string };
}

/** Where a grant applies, as the form holds it: `organization`, `project`, or `space:<name>`. */
type Place = string;

/**
 * Which bindings a page shows and grants (Architecture/09 §14.3): the organization's own on the
 * Organization page, this project's and its spaces' in Project settings, all of them where both
 * meet. A page never offers a place it does not show.
 */
export type BindingScope = "organization" | "project" | "all";

export interface GrantForm {
  subjectKind: "user" | "group";
  subject: string;
  role: string;
  place: Place;
  until: string;
}

function placeOf(scope: Scope | undefined, project: string): Place {
  if (scope?.contextSpace) {
    return `space:${scope.contextSpace}`;
  }
  return scope?.project === project ? "project" : "organization";
}

/**
 * The RoleBinding a filled form asks for. An organization scope names the Organization by its own
 * name, the project's `organizationRef` (ADR-N-058), never a guess from the domain (F-UI1-010).
 */
export function bindingOf(form: GrantForm, project: string, organizationName: string | undefined): Manifest {
  const organization = organizationName ?? ORG_NAMESPACE;
  const scope: Scope =
    form.place === "organization"
      ? { organization }
      : form.place === "project"
        ? { project }
        : { contextSpace: form.place.slice("space:".length) };
  const where = scope.organization ?? scope.project ?? scope.contextSpace ?? "";
  const who = form.subject.trim();
  const spec: BindingSpec = {
    subjects: [form.subjectKind === "user" ? { user: who } : { group: who }],
    role: form.role,
    scope,
  };
  if (form.until) {
    spec.validity = { notAfter: new Date(`${form.until}T23:59:59Z`).toISOString() };
  }
  return {
    apiVersion: "joinedcontext.com/v1alpha1",
    kind: "RoleBinding",
    // A person's id is unique where a name or an address's local part is not (`jan@a.sk` and
    // `jan@b.sk`); its first block keeps the name short.
    metadata: { name: dns1123(isPersonId(who) ? who.slice(0, 8) : who.split("@")[0], form.role, where), namespace: ORG_NAMESPACE },
    spec,
  } as Manifest;
}

/** The form a manifest fills: the assistant's draft, or an empty one. */
function formOf(
  manifest: Record<string, unknown> | null,
  project: string,
  scope: BindingScope = "all",
): GrantForm {
  const spec = (manifest?.spec ?? {}) as BindingSpec;
  const subject = spec.subjects?.[0];
  return {
    subjectKind: subject?.group ? "group" : "user",
    subject: subject?.group ?? subject?.user ?? "",
    role: spec.role ?? "",
    place: manifest ? placeOf(spec.scope, project) : scope === "organization" ? "organization" : "project",
    until: spec.validity?.notAfter?.slice(0, 10) ?? "",
  };
}

/**
 * What the chosen role lets a person do, under the role picker (T-3216): its own description when
 * it has one, else its rules in one line, so a new user need not leave the form for Roles.
 */
export function roleSummary(role: Manifest, locale: string, t: (key: string, options?: Record<string, unknown>) => string): string {
  const described = localized(role.metadata.description, locale, "");
  if (described) return described;
  const rules = ((role.spec ?? {}) as { rules?: { verbs?: string[]; kinds?: string[] }[] }).rules ?? [];
  const rights = rules
    .map((rule) => `${(rule.verbs ?? []).join(", ")} ${t("access.projectRoles.on")} ${(rule.kinds ?? []).join(", ")}`)
    .join("; ");
  return rights ? t("access.roles.grants", { rights }) : t("access.roles.grantsNothing");
}

/**
 * Giving people or a group a role (AG-77, PF-52): who, which role, where and until when, proposed
 * as a red change. The Portal refuses a grant of anything its proposer does not hold there, and
 * that refusal is shown as it comes.
 */
export function GrantRoleDialog({
  project,
  open,
  onOpenChange,
  prefill,
  scope = "all",
}: {
  project: string;
  /** The places the dialog offers; the organization page grants at organization scope only. */
  scope?: BindingScope;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** The binding the assistant drafted, when the page was opened on one. */
  prefill: Record<string, unknown> | null;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const formRoute = useFormRoute();
  const ids = useId();
  const queryClient = useQueryClient();
  const organizationName = useOrganizationName(project);
  const [form, setForm] = useState<GrantForm>(() => formOf(prefill, project, scope));
  const [change, setChange] = useState<Change | null>(null);
  // The fields an empty Propose marked, so the person is told which one is missing instead of
  // meeting a button that does nothing (UI-04, T-1492).
  const [missing, setMissing] = useState<{ subject?: boolean; role?: boolean }>({});
  const [discarding, setDiscarding] = useState(false);
  const roleRef = useRef<HTMLSelectElement | null>(null);
  const roles = useReadableList(ORG_NAMESPACE, "roles", "Role");
  // A binding in a project may name that project's own role as well as an organization one
  // (PF-69); the organization page binds organization roles only.
  const projectRoles = useReadableList(project, "roles", "Role", scope !== "organization" && project !== ORG_NAMESPACE);
  const spaces = useReadableList(project, "spaces", "ContextSpace", scope !== "organization");

  const propose = useMutation({
    mutationFn: async () =>
      proposeChecked(
        ORG_NAMESPACE,
        "rolebindings",
        bindingOf(form, project, organizationName) as ResourceProposal,
        true,
      ),
    onSuccess: (result) => {
      if (isChange(result)) {
        // Routed, the save goes back to the list and the change is shown there (T-2474).
        if (formRoute) {
          formRoute.leave(<ChangeNotice change={result} project={project} />);
          leave();
        } else {
          setChange(result);
        }
      }
      void queryClient.invalidateQueries({ queryKey: queryKeys.changes(project) });
    },
  });

  const empty = formOf(null, project, scope);
  // Something typed is something to lose: the subject, a chosen role, a place other than the
  // default or an end date. The proposed change itself is not: it is already saved.
  const typed =
    form.subject.trim() !== "" || form.role !== "" || form.place !== empty.place || form.until !== "";

  const leave = () => {
    setForm(formOf(null, project, scope));
    setChange(null);
    setMissing({});
    setDiscarding(false);
    propose.reset();
    onOpenChange(false);
  };
  const close = (next: boolean) => {
    if (next) {
      onOpenChange(true);
      return;
    }
    // Escape and the header's close arrive here too, so the question stands in the way of
    // every way out of the dialog (UI-47).
    if (change === null && typed) {
      setDiscarding(true);
      return;
    }
    leave();
  };
  const set = (patch: Partial<GrantForm>) => {
    setForm((current) => ({ ...current, ...patch }));
    setMissing({});
    propose.reset();
  };

  /** Propose, or say which field is missing and put the focus there; never both. */
  const submit = () => {
    const gaps = { subject: form.subject.trim() === "", role: form.role === "" };
    if (gaps.subject || gaps.role) {
      setMissing(gaps);
      // Each subject picker is its own control, found by the id its field gives it.
      (gaps.subject ? document.getElementById(`${ids}-subject`) : roleRef.current)?.focus();
      return;
    }
    setMissing({});
    propose.mutate();
  };

  // One entry per name, the project's own first (PF-69), with what a person reads for each.
  const roleManifests = [
    ...new Map(
      [...asManifests(projectRoles.data?.items ?? []), ...asManifests(roles.data?.items ?? [])].map(
        (role) => [role.metadata.name, role] as const,
      ),
    ).values(),
  ];
  const roleNames = roleManifests.map((role) => role.metadata.name);
  const chosenRole = roleManifests.find((role) => role.metadata.name === form.role);
  const spaceNames = asManifests(spaces.data?.items ?? []).map((space) => space.metadata.name);
  const failure =
    propose.error instanceof ApiError
      ? (propose.error.problem?.detail ?? propose.error.message)
      : propose.error
        ? t("app.error.generic")
        : null;
  const rolesFailure = roles.isError ? t("form.listFailed", { reason: reasonOf(roles.error, t("app.error.generic")) }) : null;
  const spacesFailure = spaces.isError
    ? t("form.listFailed", { reason: reasonOf(spaces.error, t("app.error.generic")) })
    : null;

  return (
    <>
    <FormFrame
      open={open}
      onOpenChange={close}
      title={t("access.roles.grantTitle")}
      description={t("access.roles.grantLead")}
      closeLabel={t("resourceDelete.close")}
      footer={
        change ? (
          <Button onClick={() => close(false)}>{t("resourceDelete.close")}</Button>
        ) : (
          <>
            <Button variant="secondary" onClick={() => close(false)}>
              {t("form.cancel")}
            </Button>
            <PermissionGuard project={ORG_NAMESPACE} kind="RoleBinding" verb="propose">
              <Button variant="primary" loading={propose.isPending} onClick={submit}>
                {t("access.roles.propose")}
              </Button>
            </PermissionGuard>
          </>
        )
      }
    >
      {change ? (
        <ChangeNotice change={change} project={project} />
      ) : (
        <div className="flex flex-col gap-4">
          <Field
            id={`${ids}-kind`}
            label={t("access.roles.subjectKind")}
            description={t("access.roles.subjectKindHelp")}
          >
            <Select
              id={`${ids}-kind`}
              value={form.subjectKind}
              onChange={(event) => set({ subjectKind: event.target.value as GrantForm["subjectKind"] })}
            >
              <option value="user">{t("access.roles.person")}</option>
              <option value="group">{t("access.roles.groupOption")}</option>
            </Select>
          </Field>
          <Field
            id={`${ids}-subject`}
            label={form.subjectKind === "user" ? t("access.roles.personLabel") : t("access.roles.groupLabel")}
            description={
              form.subjectKind === "user"
                ? t("access.roles.personHelp")
                : t("access.roles.groupHelp")
            }
            required
            errors={missing.subject ? [t("form.required")] : undefined}
          >
            {form.subjectKind === "group" ? (
              // A group is one the organization has (ADR-N-033); a person one of the realm's people
              // the caller may list, or a full e-mail of someone not signed in yet (T-3216, T-4947).
              <ResourceNamePicker
                id={`${ids}-subject`}
                label={t("access.roles.groupLabel")}
                labelled
                from={{ project: ORG_NAMESPACE, plural: "groups" }}
                value={form.subject}
                onChange={(subject) => set({ subject })}
                required
                invalid={missing.subject}
              />
            ) : (
              <PersonPicker
                id={`${ids}-subject`}
                label={t("access.roles.personLabel")}
                labelled
                value={form.subject}
                onChange={(subject) => set({ subject })}
                required
                invalid={missing.subject}
              />
            )}
          </Field>
          <Field
            id={`${ids}-role`}
            label={t("access.roles.roleLabel")}
            description={t("access.roles.roleHelp")}
            required
            // In flight, failed and empty are three different sentences. Before this they were
            // one silence: a dropdown with only "choose a role" in it and a dead button.
            help={
              roles.isPending
                ? t("app.loading")
                : !roles.isError && roleNames.length === 0
                  ? t("access.roles.rolesEmpty")
                  : chosenRole
                    ? roleSummary(chosenRole, i18n.language, t)
                    : undefined
            }
            errors={
              rolesFailure ? [rolesFailure] : missing.role ? [t("form.required")] : undefined
            }
          >
            <Select
              id={`${ids}-role`}
              ref={roleRef}
              value={form.role}
              onChange={(event) => set({ role: event.target.value })}
            >
              <option value="">{t("access.roles.chooseRole")}</option>
              {roleManifests.map((role) => {
                const name = role.metadata.name;
                const title = localized(role.metadata.title, i18n.language, name);
                return (
                  <option key={name} value={name}>
                    {title === name ? name : `${title} (${name})`}
                  </option>
                );
              })}
            </Select>
          </Field>
          <Field
            id={`${ids}-place`}
            label={t("access.roles.whereLabel")}
            description={t("access.roles.whereHelp")}
            // A grant for the project or the organization is still correct without the space
            // list, so this is said under the control rather than as a refusal of the field.
            help={spaces.isPending ? t("app.loading") : (spacesFailure ?? undefined)}
          >
            <Select id={`${ids}-place`} value={form.place} onChange={(event) => set({ place: event.target.value })}>
              {scope !== "organization" ? (
                <option value="project">{t("access.roles.project", { name: project })}</option>
              ) : null}
              {scope !== "project" ? (
                <option value="organization">{t("access.roles.organization")}</option>
              ) : null}
              {spaceNames.map((name) => (
                <option key={name} value={`space:${name}`}>
                  {t("access.roles.space", { name })}
                </option>
              ))}
            </Select>
          </Field>
          <Field
            id={`${ids}-until`}
            label={t("access.roles.untilLabel")}
            description={t("access.roles.untilHelp")}
          >
            <Input
              id={`${ids}-until`}
              type="date"
              value={form.until}
              onChange={(event) => set({ until: event.target.value })}
            />
          </Field>
          {failure ? (
            <Alert tone="danger" role="alert">
              {failure}
            </Alert>
          ) : null}
        </div>
      )}
    </FormFrame>
    <ConfirmDialog
      open={discarding}
      onOpenChange={(next) => {
        if (!next) setDiscarding(false);
      }}
      title={t("access.roles.discardTitle")}
      description={t("access.roles.discardConfirm")}
      confirmLabel={t("access.roles.discard")}
      onConfirm={leave}
    />
    </>
  );
}

/**
 * Who holds which role: over the organization, this project or one of its spaces. `scope` narrows
 * it to the organization's own bindings (Organization → Members) or to this project's and its
 * spaces' (Project settings → Members), T-2605, T-2606.
 */
export function RoleBindings({
  project,
  scope = "all",
}: {
  project: string;
  scope?: BindingScope;
}): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
  const bindings = useReadableList(ORG_NAMESPACE, "rolebindings", "RoleBinding");
  const chosen = useChosenOrganizationName();
  const spaces = useReadableList(project, "spaces", "ContextSpace", scope !== "organization");
  const [prefill] = useState(() =>
    typeof window !== "undefined" && new URLSearchParams(window.location.search).get("grant")
      ? takePrefill(window.location.pathname)
      : null,
  );
  // The routed `…/members/new` opens the same form the button does (T-2750, T-2474); a `?grant=`
  // hand-off opens it on its draft without an address of its own.
  const [routedOpen, setRoutedOpen] = useCreateForm();
  const [fromGrant, setFromGrant] = useState(prefill !== null);
  const granting = routedOpen || fromGrant;
  const setGranting = (open: boolean) => {
    if (!open) {
      setFromGrant(false);
    }
    setRoutedOpen(open);
  };

  const spaceNames = new Set(asManifests(spaces.data?.items ?? []).map((space) => space.metadata.name));
  // Every space-scoped grant is filtered against this set, so while the space list is missing
  // the table under-reports: the rows do not appear and nothing says they are missing. An
  // access review that quietly shows fewer grants than exist is worse than one that fails, so
  // the gap is named and the rows that are still trustworthy are shown.
  const loading = bindings.isPending || (scope !== "organization" && spaces.isPending);
  const here = asManifests(bindings.data?.items ?? []).filter((binding) => {
    const at = (binding.spec as BindingSpec).scope;
    // With several Organizations the page shows the chosen one's bindings (ADR-N-058 2.7).
    const organizational = Boolean(at?.organization) && (chosen === undefined || at?.organization === chosen);
    const inProject =
      at?.project === project || (at?.contextSpace !== undefined && spaceNames.has(at.contextSpace));
    return scope === "organization" ? organizational : scope === "project" ? inProject : organizational || inProject;
  });
  const names = usePersonNames(here.flatMap((binding) => ((binding.spec as BindingSpec).subjects ?? []).flatMap((s) => (s.user ? [s.user] : []))));

  const where = (scope: Scope | undefined) =>
    scope?.contextSpace
      ? t("access.roles.space", { name: scope.contextSpace })
      : scope?.project
        ? t("access.roles.project", { name: scope.project })
        : t("access.roles.organization");
  const who = (spec: BindingSpec) =>
    (spec.subjects ?? [])
      .map((subject) => (subject.group ? t("access.roles.group", { name: subject.group }) : personLabel(subject.user ?? "", names)))
      .join(", ");

  return (
    <section className="space-y-4" aria-labelledby="role-bindings-heading">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          {/* The organization's members read "in this project" (T-2759): its own words. */}
          <h2 id="role-bindings-heading" className="text-title font-semibold text-fg">
            {scope === "organization" ? t("organization.members.title") : t("access.roles.title")}
          </h2>
          <p className="text-body text-fg-muted">
            {scope === "organization" ? t("organization.members.lead") : t("access.roles.lead")}
          </p>
        </div>
        <PermissionGuard project={ORG_NAMESPACE} kind="RoleBinding" verb="propose">
          <Button variant="primary" onClick={() => setGranting(true)}>
            {t("access.roles.grant")}
          </Button>
        </PermissionGuard>
      </div>

      {scope !== "organization" && spaces.isError ? (
        <PageFailed
          error={spaces.error}
          onRetry={() => {
            void spaces.refetch();
          }}
        >
          {t("access.roles.spacesFailed", { reason: reasonOf(spaces.error, t("app.error.generic")) })}
        </PageFailed>
      ) : null}

      {bindings.hidden ? (
        <Alert tone="info">{t("access.roles.hidden")}</Alert>
      ) : bindings.isError ? (
        <PageFailed
          error={bindings.error}
          onRetry={() => {
            void bindings.refetch();
          }}
        />
      ) : (
        <Table
          data-records=""
          caption={
            scope === "organization" ? t("organization.members.caption") : t("access.roles.caption", { project })
          }
          status={loading ? t("app.loading") : undefined}
        >
          <TableHead>
            <TableHeaderCell>{t("access.roles.who")}</TableHeaderCell>
            <TableHeaderCell>{t("access.roles.role")}</TableHeaderCell>
            <TableHeaderCell>{t("access.roles.where")}</TableHeaderCell>
            <TableHeaderCell>{t("access.roles.until")}</TableHeaderCell>
            <TableHeaderCell align="right">{t("approvals.actions")}</TableHeaderCell>
          </TableHead>
          {loading ? (
            <TableSkeleton columns={5} />
          ) : (
            <TableBody>
              {here.length === 0 ? (
                <TableEmpty columns={5}>
                  <EmptyState bare
                    title={t("access.roles.empty")}
                    description={
                      scope === "organization" ? t("organization.members.emptyHint") : t("access.roles.emptyHint")
                    }
                    action={
                      <PermissionGuard project={ORG_NAMESPACE} kind="RoleBinding" verb="propose">
                        <Button variant="primary" onClick={() => setGranting(true)}>
                          {t("access.roles.grant")}
                        </Button>
                      </PermissionGuard>
                    } />
                </TableEmpty>
              ) : (
                here.map((binding) => {
                  const spec = binding.spec as BindingSpec;
                  const target = {
                    project,
                    home: ORG_NAMESPACE,
                    kind: "RoleBinding",
                    plural: "rolebindings",
                    name: binding.metadata.name,
                    label: `${who(spec)}: ${spec.role ?? ""}`,
                  };
                  const until = spec.validity?.notAfter;
                  return (
                    <TableRow key={binding.metadata.name}>
                      <TableCell primary>
                        <FormRecordLink name={binding.metadata.name}>{who(spec)}</FormRecordLink>
                      </TableCell>
                      <TableCell>{spec.role}</TableCell>
                      <TableCell>{where(spec.scope)}</TableCell>
                      <TableCell>
                        {until ? new Date(until).toLocaleDateString(locale) : t("access.roles.noEnd")}
                      </TableCell>
                      <TableCell align="right">
                        <span className="inline-flex items-center gap-1.5">
                          <EditResourceAction target={target} addressed />
                          <DeleteResourceAction target={target} />
                        </span>
                      </TableCell>
                    </TableRow>
                  );
                })
              )}
            </TableBody>
          )}
        </Table>
      )}

      <GrantRoleDialog
        project={project}
        open={granting}
        onOpenChange={setGranting}
        prefill={prefill}
        scope={scope}
      />
    </section>
  );
}
