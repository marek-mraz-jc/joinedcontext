import { useId, useState } from "react";
import type { JSX } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { useTranslation } from "react-i18next";
import { api, ApiError, queryKeys, unwrap, reasonOf } from "../../api/client";
import { ORG_NAMESPACE } from "../../api/manifest";
import type { Change } from "../../api/manifest";
import { mayRead, usePermissions } from "../../api/permissions";
import type { Verb } from "../../api/permissions";
import type { components } from "../../api/schema";
import { ChangeNotice } from "../../components/ChangeNotice";
import { FormFrame, useCreateForm } from "../../components/forms/FormRoute";
import {
  Alert,
  Card,
  Badge,
  Button,
  Checkbox,
  ConfirmDialog,
  Dialog,
  EmptyState,
  Field,
  Input,
  PageHeader,
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
import { ResourcePageFailed, PageFailed } from "../../components/ui/PageState";
import { AskAdmins } from "./AskAdmins";
import { DeniedNote, PermissionGuard } from "../../components/ui/PermissionGuard";
import { asUser } from "../apps/members";
import { proposeChecked } from "../../api/proposal";
import type { ResourceProposal } from "../../api/manifest";
import { useProjects } from "../../api/projects";
import { RadioGroup } from "../../components/ui/RadioGroup";
import { INVITE_ROLES, inviteBinding, invitationState } from "./invite";
import { NameById } from "./NameById";
import { PersonReadable } from "./DataAccess";
import type { InviteRole } from "./invite";

type Person = components["schemas"]["Person"];
type PersonDetail = components["schemas"]["PersonDetail"];

/** The languages a person may be given, the ones the Portal speaks (UI-09). */
const LOCALES = ["en", "sk", "cs", "de"] as const;
const PAGE = 50;

function fullName(person: Person): string {
  return `${person.firstName} ${person.lastName}`.trim() || person.email;
}

function when(iso: string | null | undefined, locale: string): string | null {
  if (!iso) return null;
  const at = new Date(iso);
  return Number.isNaN(at.getTime())
    ? null
    : new Intl.DateTimeFormat(locale, { dateStyle: "medium", timeStyle: "short" }).format(at);
}

/**
 * The one-time temporary password (PF-92): shown once, in a field to copy, with the warning that
 * nobody can show it again. It lives in this dialog's props and nowhere else — no storage, no
 * address, no console — and closing the dialog drops it from the page.
 */
export function OneTimePassword({
  secret,
  onClose,
}: {
  secret: { email: string; password: string } | null;
  onClose: () => void;
}): JSX.Element {
  const { t } = useTranslation();
  const field = useId();
  const [copied, setCopied] = useState(false);
  return (
    <Dialog
      open={secret !== null}
      onOpenChange={(open) => {
        if (!open) {
          setCopied(false);
          onClose();
        }
      }}
      title={t("organization.people.password.title")}
      description={secret ? t("organization.people.password.lead", { email: secret.email }) : undefined}
      closeLabel={t("app.close")}
      footer={
        <Button
          onClick={() => {
            setCopied(false);
            onClose();
          }}
        >
          {t("organization.people.password.done")}
        </Button>
      }
    >
      {secret ? (
        <div className="space-y-3">
          <Alert tone="warning">{t("organization.people.password.warning")}</Alert>
          <Field id={field} label={t("organization.people.password.label")}>
            <Input
              id={field}
              readOnly
              value={secret.password}
              autoComplete="off"
              spellCheck={false}
              className="font-mono"
              onFocus={(event) => event.currentTarget.select()}
            />
          </Field>
          <Button
            variant="secondary"
            size="sm"
            onClick={() => {
              void navigator.clipboard?.writeText(secret.password).then(() => setCopied(true));
            }}
          >
            {t("organization.people.password.copy")}
          </Button>
          <p role="status" className="text-caption text-fg-muted">
            {copied ? t("organization.people.password.copied") : ""}
          </p>
        </div>
      ) : null}
    </Dialog>
  );
}

export interface PersonForm {
  email: string;
  firstName: string;
  lastName: string;
  locale: string;
}

type FormErrors = Partial<Record<keyof PersonForm, string>>;

function validate(form: PersonForm, t: (key: string) => string): FormErrors {
  const errors: FormErrors = {};
  if (asUser(form.email) === null) errors.email = t("organization.people.form.emailInvalid");
  if (form.firstName.trim() === "") errors.firstName = t("organization.people.form.required");
  if (form.lastName.trim() === "") errors.lastName = t("organization.people.form.required");
  return errors;
}

/** The fields of a person, the same in the new-person form and in the edit dialog. */
function PersonFields({
  form,
  errors,
  onChange,
}: {
  form: PersonForm;
  errors: FormErrors;
  onChange: (form: PersonForm) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const id = useId();
  const text = (key: "email" | "firstName" | "lastName", type: string, example: string | undefined, auto: string) => (
    <Field
      id={`${id}-${key}`}
      label={t(`organization.people.form.${key}`)}
      description={t(`organization.people.form.${key}Help`)}
      required
      errors={errors[key] ? [errors[key]] : undefined}
    >
      <Input
        id={`${id}-${key}`}
        type={type}
        autoComplete={auto}
        placeholder={example}
        value={form[key]}
        onChange={(event) => onChange({ ...form, [key]: event.target.value })}
      />
    </Field>
  );
  return (
    <div className="space-y-4">
      {text("email", "email", "firstname.lastname@example.org", "off")}
      {text("firstName", "text", undefined, "off")}
      {text("lastName", "text", undefined, "off")}
      <Field
        id={`${id}-locale`}
        label={t("organization.people.form.locale")}
        description={t("organization.people.form.localeHelp")}
      >
        <Select
          id={`${id}-locale`}
          value={form.locale}
          onChange={(event) => onChange({ ...form, locale: event.target.value })}
        >
          <option value="">{t("organization.people.form.localeRealm")}</option>
          {LOCALES.map((locale) => (
            <option key={locale} value={locale}>
              {t(`organization.people.language.${locale}`)}
            </option>
          ))}
        </Select>
      </Field>
    </div>
  );
}

const EMPTY: PersonForm = { email: "", firstName: "", lastName: "", locale: "" };

/** What an invitation did: the person, and the Change proposing their role or why it failed. */
export interface Invitation {
  created: components["schemas"]["CreatedPerson"];
  project?: string;
  role?: InviteRole;
  change?: Change;
  grantError?: string;
}

/**
 * New person, a routed form (UI-27): the realm e-mails the invitation, or hands a password once.
 * A project and a role make the link lead into that project and propose the role there as a
 * RoleBinding, which holds once approved (PF-108).
 */
function NewPersonForm({
  open,
  onOpenChange,
  onCreated,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onCreated: (invitation: Invitation) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const formId = useId();
  const projectField = useId();
  const queryClient = useQueryClient();
  const projects = useProjects();
  const [form, setForm] = useState<PersonForm>(EMPTY);
  const [project, setProject] = useState("");
  const [role, setRole] = useState<InviteRole>("viewer");
  const [tried, setTried] = useState(false);
  const errors = tried ? validate(form, t) : {};

  const create = useMutation({
    // The answer may carry the temporary password: it is not kept in the mutation cache.
    gcTime: 0,
    mutationFn: async (person: PersonForm): Promise<Invitation> => {
      const created = await unwrap(
        await api.POST("/api/v1/organization/people", {
          body: {
            email: person.email.trim(),
            firstName: person.firstName.trim(),
            lastName: person.lastName.trim(),
            ...(person.locale ? { locale: person.locale } : {}),
            ...(project ? { project } : {}),
          },
        }),
      );
      if (!project) return { created };
      try {
        const change = (await proposeChecked(
          ORG_NAMESPACE,
          "rolebindings",
          inviteBinding(created.person.id, role, project) as ResourceProposal,
          true,
        )) as Change;
        return { created, project, role, change };
      } catch (error) {
        // The person exists and the e-mail went: the role is what is left to give.
        return { created, project, role, grantError: reasonOf(error, t("app.error.generic")) };
      }
    },
    onSuccess: (invitation) => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.people() });
      if (invitation.change) void queryClient.invalidateQueries({ queryKey: queryKeys.changes(ORG_NAMESPACE) });
      close(false);
      onCreated(invitation);
    },
  });

  const close = (next: boolean) => {
    if (!next) {
      setForm(EMPTY);
      setProject("");
      setRole("viewer");
      setTried(false);
      create.reset();
    }
    onOpenChange(next);
  };

  return (
    <FormFrame
      open={open}
      onOpenChange={close}
      title={t("organization.people.newTitle")}
      description={t("organization.people.newLead")}
      closeLabel={t("app.close")}
      footer={
        <>
          <Button variant="secondary" onClick={() => close(false)}>
            {t("app.cancel")}
          </Button>
          <Button type="submit" form={formId} disabled={create.isPending} disabledReason={t("app.busyReason")}>
            {t("organization.people.create")}
          </Button>
        </>
      }
    >
      <form
        id={formId}
        noValidate
        className="space-y-4"
        onSubmit={(event) => {
          event.preventDefault();
          setTried(true);
          if (Object.keys(validate(form, t)).length === 0) create.mutate(form);
        }}
      >
        <PersonFields form={form} errors={errors} onChange={setForm} />
        <Field
          id={projectField}
          label={t("organization.people.invite.project")}
          description={t("organization.people.invite.projectHelp")}
        >
          <Select id={projectField} value={project} onChange={(event) => setProject(event.target.value)}>
            <option value="">{t("organization.people.invite.noProject")}</option>
            {(projects.data ?? []).map((name) => (
              <option key={name} value={name}>
                {name}
              </option>
            ))}
          </Select>
        </Field>
        {project ? (
          <RadioGroup<InviteRole>
            name={`${formId}-role`}
            legend={t("organization.people.invite.role")}
            description={t("organization.people.invite.roleHelp", { project })}
            value={role}
            onChange={setRole}
            options={INVITE_ROLES.map((value) => ({
              value,
              label: t(`organization.people.invite.roles.${value}.label`),
              description: t(`organization.people.invite.roles.${value}.may`),
            }))}
          />
        ) : null}
        {create.error ? (
          <Alert tone="danger" role="alert">
            {reasonOf(create.error, t("app.error.generic"))}
          </Alert>
        ) : null}
      </form>
    </FormFrame>
  );
}

/**
 * Organization → People (PF-90, UI-75, ADR-N-031): the people of the realm, searched and paged,
 * and the form that invites a new one. A person is not a manifest: these requests go to the
 * realm's admin API through the Portal, each held to its verb on `Person`, and what a person may
 * do stays with the bindings and groups that name them.
 */
export function People(): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
  const permissions = usePermissions(ORG_NAMESPACE);
  const reads = mayRead(permissions.data, "Person");
  const searchField = useId();
  const [typed, setTyped] = useState("");
  const [search, setSearch] = useState("");
  const [first, setFirst] = useState(0);
  const [writing, setWriting] = useCreateForm();
  const [secret, setSecret] = useState<{ email: string; password: string } | null>(null);
  const [invited, setInvited] = useState<Invitation | null>(null);
  // The people ticked on this page; a new search or page starts with nobody ticked.
  const [selected, setSelected] = useState<Person[]>([]);

  const people = useQuery({
    queryKey: queryKeys.peoplePage(search, first),
    enabled: reads === true,
    queryFn: async () =>
      unwrap(
        await api.GET("/api/v1/organization/people", {
          params: { query: { ...(search ? { search } : {}), first, max: PAGE } },
        }),
      ),
  });

  if (permissions.isLoading) {
    return (
      <p role="status" className="text-body text-fg-muted">
        {t("app.loading")}
      </p>
    );
  }

  return (
    <section className="space-y-4" aria-labelledby="people-heading">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h2 id="people-heading" className="text-title font-semibold text-fg">
            {t("organization.people.title")}
          </h2>
          <p className="text-body text-fg-muted">{t("organization.people.lead")}</p>
        </div>
        <PermissionGuard project={ORG_NAMESPACE} kind="Person" verb="create">
          <Button onClick={() => setWriting(true)}>{t("organization.people.new")}</Button>
        </PermissionGuard>
      </div>

      {invited ? <InvitedNotice invitation={invited} /> : null}

      {reads !== true ? (
        <Alert tone="info">{t("organization.people.hidden")}</Alert>
      ) : (
        <>
          <form
            role="search"
            className="flex flex-wrap items-end gap-2"
            onSubmit={(event) => {
              event.preventDefault();
              setFirst(0);
              setSearch(typed.trim());
              setSelected([]);
            }}
          >
            <Field id={searchField} label={t("organization.people.search")}>
              <Input
                id={searchField}
                type="search"
                value={typed}
                placeholder={t("organization.people.searchPlaceholder")}
                onChange={(event) => setTyped(event.target.value)}
              />
            </Field>
            <Button type="submit" variant="secondary" size="sm">
              {t("organization.people.searchButton")}
            </Button>
          </form>

          <PendingInvitations people={people.data?.items ?? []} locale={locale} />

          <BulkActions selected={selected} onDone={() => setSelected([])} />

          {people.error ? (
            <PageFailed error={people.error} onRetry={() => void people.refetch()} />
          ) : (
            <Table
              data-records=""
              caption={t("organization.people.caption")}
              status={people.isPending ? t("app.loading") : undefined}
            >
              <TableHead>
                <TableHeaderCell>
                  <SelectAll items={people.data?.items ?? []} selected={selected} onChange={setSelected} />
                </TableHeaderCell>
                <TableHeaderCell>{t("organization.people.name")}</TableHeaderCell>
                <TableHeaderCell>{t("organization.people.email")}</TableHeaderCell>
                <TableHeaderCell>{t("organization.people.state")}</TableHeaderCell>
                <TableHeaderCell>{t("organization.people.lastSeen")}</TableHeaderCell>
              </TableHead>
              {people.isPending ? (
                <TableSkeleton columns={5} />
              ) : (
                <TableBody>
                  {(people.data?.items ?? []).length === 0 ? (
                    <TableEmpty columns={5}>
                      <EmptyState
                        bare
                        title={search ? t("organization.people.noMatch", { search }) : t("organization.people.empty")}
                        description={search ? undefined : t("organization.people.emptyHint")}
                      />
                    </TableEmpty>
                  ) : (
                    (people.data?.items ?? []).map((person) => (
                      <TableRow key={person.id}>
                        <TableCell>
                          <Checkbox
                            label={<span className="sr-only">{t("organization.people.bulk.select", { name: fullName(person) })}</span>}
                            checked={selected.some((one) => one.id === person.id)}
                            onChange={(event) =>
                              setSelected((now) =>
                                event.target.checked ? [...now, person] : now.filter((one) => one.id !== person.id),
                              )
                            }
                          />
                        </TableCell>
                        <TableCell primary>
                          <Link
                            data-row-link=""
                            to="/organization/$tab/$"
                            params={{ tab: "people", _splat: encodeURIComponent(person.id) }}
                            className="underline"
                          >
                            {fullName(person)}
                          </Link>
                        </TableCell>
                        <TableCell>
                          <span className="font-mono text-caption">{person.email}</span>
                        </TableCell>
                        <TableCell>
                          <PersonState person={person} />
                        </TableCell>
                        <TableCell>
                          {when(person.lastSeen, locale) ?? (
                            <span className="text-fg-subtle">{t("organization.people.noSession")}</span>
                          )}
                        </TableCell>
                      </TableRow>
                    ))
                  )}
                </TableBody>
              )}
            </Table>
          )}

          {first > 0 || people.data?.next != null ? (
            <nav aria-label={t("organization.people.pages")} className="flex gap-2">
              <Button
                variant="secondary"
                size="sm"
                disabled={first === 0}
                disabledReason={t("app.firstPage")}
                onClick={() => {
                  setFirst(Math.max(0, first - PAGE));
                  setSelected([]);
                }}
              >
                {t("organization.people.previous")}
              </Button>
              <Button
                variant="secondary"
                size="sm"
                disabled={people.data?.next == null}
                disabledReason={t("app.lastPage")}
                onClick={() => {
                  setFirst(people.data?.next ?? first);
                  setSelected([]);
                }}
              >
                {t("organization.people.next")}
              </Button>
            </nav>
          ) : null}
        </>
      )}

      <NewPersonForm
        open={writing}
        onOpenChange={setWriting}
        onCreated={(invitation) => {
          const { created } = invitation;
          setInvited(invitation);
          if (created.temporaryPassword) {
            setSecret({ email: created.person.email, password: created.temporaryPassword });
          }
        }}
      />
      <OneTimePassword secret={secret} onClose={() => setSecret(null)} />
      <NameById />
    </section>
  );
}

/** The header box: ticks everyone on the page, or nobody when everyone already is; none on an empty page. */
function SelectAll({
  items,
  selected,
  onChange,
}: {
  items: Person[];
  selected: Person[];
  onChange: (people: Person[]) => void;
}): JSX.Element | null {
  const { t } = useTranslation();
  if (items.length === 0) return null;
  const all = items.every((person) => selected.some((one) => one.id === person.id));
  return (
    <Checkbox
      label={<span className="sr-only">{t("organization.people.bulk.selectAll")}</span>}
      checked={all}
      onChange={() => onChange(all ? [] : items)}
    />
  );
}

type Bulk = "disable" | "delete";

type BulkOutcome = { text: string; failed?: boolean } | { change: Change };

/**
 * Disables or deletes the ticked people at once (T-5606): a department leaving is one confirm
 * naming the count, and a deletion is ONE Change taking all of them out of every Group and
 * RoleBinding. The server holds each person to the single route's rules and says who it refused.
 */
function BulkActions({ selected, onDone }: { selected: Person[]; onDone: () => void }): JSX.Element {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [asking, setAsking] = useState<Bulk | null>(null);
  const [outcome, setOutcome] = useState<BulkOutcome | null>(null);
  const count = selected.length;
  const act = useMutation({
    mutationFn: async (bulk: Bulk): Promise<BulkOutcome> => {
      if (bulk === "delete") {
        const result = await api.POST("/api/v1/organization/people/remove", {
          body: { ids: selected.map((person) => person.id) },
        });
        const change = await unwrap(result);
        return result.response.status === 204
          ? { text: t("organization.people.bulk.deleted", { count }) }
          : { change: change as Change };
      }
      // A disable changes no manifest: each person is their own call, one after another.
      const failed: string[] = [];
      for (const person of selected) {
        try {
          await perform(person.id, "disable");
        } catch (error) {
          failed.push(`${person.email} (${reasonOf(error, t("app.error.generic"))})`);
        }
      }
      return failed.length === 0
        ? { text: t("organization.people.bulk.disabled", { count }) }
        : {
            text: t("organization.people.bulk.partly", { done: count - failed.length, count, failed: failed.join(", ") }),
            failed: true,
          };
    },
    onMutate: () => setOutcome(null),
    // The refusal names the person; the dialog closes so the alert beside the ticks is read.
    onError: () => setAsking(null),
    onSuccess: (done) => {
      setAsking(null);
      setOutcome(done);
      onDone();
      void queryClient.invalidateQueries({ queryKey: queryKeys.people() });
    },
  });
  return (
    <div className="space-y-2">
      <p className="text-caption text-fg-muted">{t("organization.people.bulk.hint")}</p>
      {outcome && "change" in outcome ? <ChangeNotice change={outcome.change} project={ORG_NAMESPACE} /> : null}
      {outcome && "text" in outcome ? (
        <Alert tone={outcome.failed ? "warning" : "success"} role="status">
          {outcome.text}
        </Alert>
      ) : null}
      {act.error ? (
        <Alert tone="danger" role="alert">
          {reasonOf(act.error, t("app.error.generic"))}
        </Alert>
      ) : null}
      {count === 0 ? null : (
        <div role="toolbar" aria-label={t("organization.people.bulk.selected", { count })} className="flex flex-wrap items-center gap-2">
          <span className="text-body text-fg">{t("organization.people.bulk.selected", { count })}</span>
          <PermissionGuard project={ORG_NAMESPACE} kind="Person" verb="disable">
            <Button variant="secondary" size="sm" disabled={act.isPending} disabledReason={t("app.busyReason")} onClick={() => setAsking("disable")}>
              {t("organization.people.bulk.disable")}
            </Button>
          </PermissionGuard>
          <PermissionGuard project={ORG_NAMESPACE} kind="Person" verb="delete">
            <Button variant="danger" size="sm" disabled={act.isPending} disabledReason={t("app.busyReason")} onClick={() => setAsking("delete")}>
              {t("organization.people.bulk.delete")}
            </Button>
          </PermissionGuard>
          <Button variant="ghost" size="sm" onClick={onDone}>
            {t("organization.people.bulk.clear")}
          </Button>
        </div>
      )}
      <ConfirmDialog
        open={asking !== null}
        onOpenChange={(open) => {
          if (!open) setAsking(null);
        }}
        title={asking === "delete" ? t("organization.people.bulk.confirmDelete.title", { count }) : t("organization.people.bulk.confirmDisable.title", { count })}
        description={asking === "delete" ? t("organization.people.bulk.confirmDelete.body") : t("organization.people.bulk.confirmDisable.body")}
        confirmLabel={asking === "delete" ? t("organization.people.bulk.delete") : t("organization.people.bulk.disable")}
        tone={asking === "delete" ? "danger" : "primary"}
        pending={act.isPending}
        onConfirm={() => {
          if (asking) act.mutate(asking);
        }}
      />
    </div>
  );
}

/** What an invitation did, said once above the list: the e-mail, the role proposed, or why not. */
function InvitedNotice({ invitation }: { invitation: Invitation }): JSX.Element {
  const { t } = useTranslation();
  const { created, project, role } = invitation;
  const email = created.person.email;
  const roleName = role ? t(`organization.people.invite.roles.${role}.label`) : "";
  return (
    <div className="space-y-2">
      {created.emailSent ? (
        <Alert tone="success" role="status">
          {project
            ? t("organization.people.invitedInto", { email, project })
            : created.memberOf
              ? t("organization.people.invitedMember", { email, organization: created.memberOf })
              : t("organization.people.invited", { email })}
        </Alert>
      ) : null}
      {invitation.change ? (
        <>
          <Alert tone="info">{t("organization.people.invite.roleProposed", { email, role: roleName, project })}</Alert>
          <ChangeNotice change={invitation.change} project={ORG_NAMESPACE} />
        </>
      ) : null}
      {invitation.grantError ? (
        <Alert tone="danger" role="alert">
          {t("organization.people.invite.roleFailed", { role: roleName, project, reason: invitation.grantError })}
        </Alert>
      ) : null}
    </div>
  );
}

/**
 * The invitations of this page nobody has accepted yet (PF-108): when each link expires, and
 * sending it again or revoking it. Revoking deletes the person, which a person no manifest
 * names yet undergoes at once; one a binding already names gets the removal Change instead.
 */
function PendingInvitations({ people, locale }: { people: Person[]; locale: string }): JSX.Element | null {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [asking, setAsking] = useState<{ person: Person; action: "resend-invitation" | "delete" } | null>(null);
  const [done, setDone] = useState<{ text?: string; change?: Change } | null>(null);
  const [secret, setSecret] = useState<{ email: string; password: string } | null>(null);
  const act = useMutation({
    // A resend's answer may carry the temporary password: it is not kept in the mutation cache.
    gcTime: 0,
    mutationFn: ({ person, action }: { person: Person; action: "resend-invitation" | "delete" }) => perform(person.id, action),
    onMutate: () => setDone(null),
    onSuccess: (outcome, { person }) => {
      setAsking(null);
      void queryClient.invalidateQueries({ queryKey: queryKeys.people() });
      if (outcome.action === "deleted") {
        setDone({ text: t("organization.people.pending.revoked", { email: person.email }) });
      } else if (outcome.password) {
        setSecret({ email: person.email, password: outcome.password });
      } else if (outcome.change) {
        setDone({ change: outcome.change });
      } else {
        setDone({ text: t("organization.people.pending.resent", { email: person.email }) });
      }
      act.reset();
    },
  });
  const waiting = people.filter((person) => person.requiredActions.length > 0 && !person.pendingDeletion);
  if (waiting.length === 0 && !done) return null;
  const now = new Date();
  return (
    <section aria-labelledby="pending-invitations" className="space-y-3 rounded border border-border p-4">
      <h3 id="pending-invitations" className="text-body font-semibold text-fg">
        {t("organization.people.pending.title")}
      </h3>
      {done?.text ? (
        <Alert tone="success" role="status">
          {done.text}
        </Alert>
      ) : null}
      {done?.change ? <ChangeNotice change={done.change} project={ORG_NAMESPACE} /> : null}
      {act.error ? (
        <Alert tone="danger" role="alert">
          {reasonOf(act.error, t("app.error.generic"))}
        </Alert>
      ) : null}
      {waiting.length === 0 ? null : (
        <ul className="divide-y divide-border">
          {waiting.map((person) => {
            const standing = invitationState(person, now);
            const at = standing.state === "accepted" ? null : when(standing.expires?.toISOString(), locale);
            return (
              <li key={person.id} className="flex flex-wrap items-center justify-between gap-2 py-2">
                <span className="min-w-0">
                  <span className="block text-body text-fg">{fullName(person)}</span>
                  <span className="block font-mono text-caption text-fg-muted">{person.email}</span>
                  <span className={standing.state === "expired" ? "block text-caption text-danger" : "block text-caption text-fg-muted"}>
                    {standing.state === "expired"
                      ? t("organization.people.pending.expired", { at })
                      : at
                        ? t("organization.people.pending.expires", { at })
                        : t("organization.people.pending.expiryUnknown")}
                  </span>
                </span>
                <span className="flex flex-wrap gap-2">
                  <PermissionGuard project={ORG_NAMESPACE} kind="Person" verb="create">
                    <Button
                      variant="secondary"
                      size="sm"
                      disabled={act.isPending}
                      disabledReason={t("app.busyReason")}
                      aria-label={t("organization.people.pending.resendFor", { email: person.email })}
                      onClick={() => setAsking({ person, action: "resend-invitation" })}
                    >
                      {t("organization.people.action.resend-invitation")}
                    </Button>
                  </PermissionGuard>
                  <PermissionGuard project={ORG_NAMESPACE} kind="Person" verb="delete">
                    <Button
                      variant="danger"
                      size="sm"
                      disabled={act.isPending}
                      disabledReason={t("app.busyReason")}
                      aria-label={t("organization.people.pending.revokeFor", { email: person.email })}
                      onClick={() => setAsking({ person, action: "delete" })}
                    >
                      {t("organization.people.pending.revoke")}
                    </Button>
                  </PermissionGuard>
                </span>
              </li>
            );
          })}
        </ul>
      )}
      <ConfirmDialog
        open={asking !== null}
        onOpenChange={(open) => {
          if (!open) setAsking(null);
        }}
        title={
          asking
            ? asking.action === "delete"
              ? t("organization.people.pending.confirmRevoke.title", { email: asking.person.email })
              : t("organization.people.confirm.resend-invitation.title", { name: fullName(asking.person) })
            : ""
        }
        description={
          asking
            ? asking.action === "delete"
              ? t("organization.people.pending.confirmRevoke.body")
              : t("organization.people.confirm.resend-invitation.body", { name: fullName(asking.person) })
            : undefined
        }
        confirmLabel={
          asking
            ? asking.action === "delete"
              ? t("organization.people.pending.revoke")
              : t("organization.people.action.resend-invitation")
            : ""
        }
        tone={asking?.action === "delete" ? "danger" : "primary"}
        pending={act.isPending}
        onConfirm={() => {
          if (asking) act.mutate(asking);
        }}
      />
      <OneTimePassword secret={secret} onClose={() => setSecret(null)} />
    </section>
  );
}

function PersonState({ person }: { person: Person }): JSX.Element {
  const { t } = useTranslation();
  return (
    <span className="inline-flex flex-wrap gap-1">
      <Badge tone={person.enabled ? "success" : "danger"}>
        {person.enabled ? t("organization.people.enabled") : t("organization.people.disabled")}
      </Badge>
      <Badge tone={person.emailVerified ? "neutral" : "warning"}>
        {person.emailVerified ? t("organization.people.verified") : t("organization.people.unverified")}
      </Badge>
      {person.pendingDeletion ? (
        <Badge tone="warning">{t("organization.people.removalPending")}</Badge>
      ) : null}
    </span>
  );
}

/** The lifecycle actions of a person's page, each with the verb on `Person` it needs (PF-91). */
type Action = "disable" | "enable" | "reset-password" | "resend-invitation" | "remove-second-factor" | "sign-out" | "delete";

const NEEDS: Record<Action, Verb> = {
  disable: "disable",
  enable: "disable",
  "reset-password": "update",
  "resend-invitation": "create",
  "remove-second-factor": "disable",
  "sign-out": "disable",
  delete: "delete",
};

type Outcome =
  | { action: Action; change?: Change; password?: string; emailSent?: boolean }
  | { action: "deleted" };

async function perform(id: string, action: Action): Promise<Outcome> {
  const path = { params: { path: { id } } };
  switch (action) {
    case "disable":
      await unwrap(await api.POST("/api/v1/organization/people/{id}/disable", path));
      return { action };
    case "enable":
      await unwrap(await api.POST("/api/v1/organization/people/{id}/enable", path));
      return { action };
    case "reset-password": {
      const reset = await unwrap(await api.POST("/api/v1/organization/people/{id}/reset-password", path));
      return { action, emailSent: reset.emailSent, password: reset.temporaryPassword ?? undefined };
    }
    case "resend-invitation": {
      const sent = await unwrap(await api.POST("/api/v1/organization/people/{id}/resend-invitation", path));
      return { action, emailSent: sent.emailSent, password: sent.temporaryPassword ?? undefined };
    }
    case "remove-second-factor":
      await unwrap(await api.POST("/api/v1/organization/people/{id}/remove-second-factor", path));
      return { action };
    case "sign-out":
      await unwrap(await api.POST("/api/v1/organization/people/{id}/sign-out", path));
      return { action };
    case "delete": {
      const result = await api.DELETE("/api/v1/organization/people/{id}", path);
      const change = await unwrap(result);
      return result.response.status === 204 ? { action: "deleted" } : { action, change: change as Change };
    }
  }
}

/**
 * A person page that did not open. A 403 is whose page this is, under the section's own title: the
 * id in the address names nobody to a caller the server refuses (T-6423).
 */
export function PersonFailed({
  id,
  anchor,
  error,
  onRetry,
}: {
  id: string;
  anchor?: string;
  error: unknown;
  onRetry: () => void;
}): JSX.Element {
  const { t } = useTranslation();
  if (error instanceof ApiError && error.status === 403) {
    return (
      <div className="flex flex-col gap-4">
        <PageHeader title={t("organization.people.title")} description={t("organization.people.lead")} />
        <AskAdmins message={t("organization.adminOnly")} back={anchor} />
      </div>
    );
  }
  return (
    <ResourcePageFailed
      title={id}
      description={t("organization.people.lead")}
      error={error}
      onRetry={onRetry}
      back={
        <Link to="/organization/$tab" params={{ tab: "people" }} className="text-body underline">
          {t("organization.people.back")}
        </Link>
      }
    >
      {error instanceof ApiError && error.status === 404 ? t("organization.people.notFound") : undefined}
    </ResourcePageFailed>
  );
}

/**
 * A person's page (PF-93, PF-94): who they are, where the realm has them, the groups they are
 * in and the roles those give them, and every lifecycle action. Each action asks first, names
 * what it does, and is disabled with the reason when the caller's role does not allow it; the
 * server still refuses anything the caller may not do to this person, and says why.
 */
export function PersonPage({ id, anchor }: { id: string; anchor?: string }): JSX.Element {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language ?? "en";
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [asking, setAsking] = useState<Action | null>(null);
  const [editing, setEditing] = useState(false);
  const [done, setDone] = useState<{ text?: string; change?: Change } | null>(null);
  const [secret, setSecret] = useState<{ email: string; password: string } | null>(null);

  const detail = useQuery({
    queryKey: queryKeys.person(id),
    retry: false,
    queryFn: async () =>
      unwrap(await api.GET("/api/v1/organization/people/{id}", { params: { path: { id } } })),
  });

  const act = useMutation({
    // A reset's answer may carry the temporary password: it is not kept in the mutation cache.
    gcTime: 0,
    mutationFn: (action: Action) => perform(id, action),
    onMutate: () => setDone(null),
    onSuccess: (outcome) => {
      setAsking(null);
      void queryClient.invalidateQueries({ queryKey: queryKeys.people() });
      if (outcome.action === "deleted") {
        void navigate({ to: "/organization/$tab", params: { tab: "people" } });
        return;
      }
      if (outcome.password && detail.data) {
        setSecret({ email: detail.data.person.email, password: outcome.password });
      } else if (outcome.change) {
        setDone({ change: outcome.change });
        void queryClient.invalidateQueries({ queryKey: queryKeys.changes(ORG_NAMESPACE) });
      } else {
        setDone({ text: t(`organization.people.done.${outcome.action}`) });
      }
      act.reset();
    },
  });

  if (detail.isPending) {
    return (
      <p role="status" className="text-body text-fg-muted">
        {t("app.loading")}
      </p>
    );
  }
  if (detail.error || !detail.data) {
    return <PersonFailed id={id} anchor={anchor} error={detail.error} onRetry={() => void detail.refetch()} />;
  }

  const { person, groups, platformRoles, appRoles } = detail.data as PersonDetail;
  const name = fullName(person);
  const actions: Action[] = [
    person.enabled ? "disable" : "enable",
    ...(person.requiredActions.length > 0 ? (["resend-invitation"] as const) : []),
    "reset-password",
    "remove-second-factor",
    "sign-out",
    "delete",
  ];

  return (
    <div className="space-y-6">
      <PageHeader
        title={name}
        description={person.email}
        actions={
          <Link to="/organization/$tab" params={{ tab: "people" }} className="text-body underline">
            {t("organization.people.back")}
          </Link>
        }
      />
      {done?.change ? <ChangeNotice change={done.change} project={ORG_NAMESPACE} /> : null}
      {done?.text ? (
        <Alert tone="success" role="status">
          {done.text}
        </Alert>
      ) : null}
      {act.error ? (
        <Alert tone="danger" role="alert">
          {reasonOf(act.error, t("app.error.generic"))}
        </Alert>
      ) : null}

      <Card as="section" padding="md" aria-labelledby="person-account" className="space-y-3">
        <h2 id="person-account" className="text-title font-semibold text-fg">
          {t("organization.people.account")}
        </h2>
        <PersonState person={person} />
        <dl className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 text-body">
          <dt className="text-fg-muted">{t("organization.people.form.locale")}</dt>
          <dd>{person.locale ? t(`organization.people.language.${person.locale}`) : t("organization.people.form.localeRealm")}</dd>
          <dt className="text-fg-muted">{t("organization.people.created")}</dt>
          <dd>{when(person.createdAt, locale) ?? "—"}</dd>
          <dt className="text-fg-muted">{t("organization.people.lastSeen")}</dt>
          <dd>{when(person.lastSeen, locale) ?? t("organization.people.noSession")}</dd>
          {person.requiredActions.length > 0 ? (
            <>
              <dt className="text-fg-muted">{t("organization.people.pendingSteps")}</dt>
              <dd>
                {person.requiredActions
                  .map((step) => t(`organization.people.step.${step}`, { defaultValue: step }))
                  .join(", ")}
              </dd>
              <dt className="text-fg-muted">{t("organization.people.pending.link")}</dt>
              <dd>
                {(() => {
                  const standing = invitationState(person, new Date());
                  const at = standing.state === "accepted" ? null : when(standing.expires?.toISOString(), locale);
                  if (standing.state === "expired") return t("organization.people.pending.expired", { at });
                  return at ? t("organization.people.pending.expires", { at }) : t("organization.people.pending.expiryUnknown");
                })()}
              </dd>
            </>
          ) : null}
        </dl>
        {/* The id manifests name the person by, folded away: people read the name (T-6423). */}
        <details className="text-caption text-fg-muted">
          <summary className="focus-ring w-fit cursor-pointer rounded-sm">{t("organization.people.id")}</summary>
          <p className="pt-1">
            {t("organization.people.idHint")} <code className="font-mono text-fg [overflow-wrap:anywhere]">{person.id}</code>
          </p>
        </details>
        <DeniedNote project={ORG_NAMESPACE} kind="Person" verb="update" />
        <div className="flex flex-wrap gap-2">
          <PermissionGuard project={ORG_NAMESPACE} kind="Person" verb="update">
            <Button variant="secondary" size="sm" onClick={() => setEditing(true)}>
              {t("organization.people.edit")}
            </Button>
          </PermissionGuard>
          {actions.map((action) => (
            <PermissionGuard key={action} project={ORG_NAMESPACE} kind="Person" verb={NEEDS[action]}>
              <Button
                variant={action === "delete" ? "danger" : "secondary"}
                size="sm"
                disabled={act.isPending || (action === "delete" && Boolean(person.pendingDeletion))}
                disabledReason={
                  action === "delete" && person.pendingDeletion
                    ? t("organization.people.removalPending")
                    : undefined
                }
                onClick={() => setAsking(action)}
              >
                {t(`organization.people.action.${action}`)}
              </Button>
            </PermissionGuard>
          ))}
        </div>
      </Card>

      {/* The catalogue as this person would see it, names only (ADR-N-067 §2.5, T-6412). */}
      <PersonReadable id={person.id} name={name} />

      <Card as="section" padding="md" aria-labelledby="person-groups" className="space-y-3">
        <h2 id="person-groups" className="text-title font-semibold text-fg">
          {t("organization.people.groups")}
        </h2>
        {groups.length === 0 ? (
          <p className="text-body text-fg-muted">{t("organization.people.noGroups")}</p>
        ) : (
          <ul className="space-y-1">
            {groups.map((group) => (
              <li key={group.name}>
                <Link
                  to="/organization/$tab/$"
                  params={{ tab: "groups", _splat: encodeURIComponent(group.name) }}
                  className="underline"
                >
                  {group.name}
                </Link>
              </li>
            ))}
          </ul>
        )}
      </Card>

      <Card as="section" padding="md" aria-labelledby="person-platform" className="space-y-3">
        <h2 id="person-platform" className="text-title font-semibold text-fg">
          {t("organization.people.platformRoles")}
        </h2>
        {platformRoles.length === 0 ? (
          <p className="text-body text-fg-muted">{t("organization.people.noRoles")}</p>
        ) : (
          <ul className="space-y-1">
            {platformRoles.map((role) => (
              <li key={`${role.binding}/${role.role}`} className="text-body">
                <Link to="/organization/$tab" params={{ tab: "roles" }} className="font-semibold underline">
                  {role.role}
                </Link>{" "}
                <span className="text-fg-muted">
                  {scopeText(role.scope as Scope, t)} · {viaText(role.via as Via, t)}
                </span>
              </li>
            ))}
          </ul>
        )}
      </Card>

      <Card as="section" padding="md" aria-labelledby="person-apps" className="space-y-3">
        <h2 id="person-apps" className="text-title font-semibold text-fg">
          {t("organization.people.appRoles")}
        </h2>
        {appRoles.length === 0 ? (
          <p className="text-body text-fg-muted">{t("organization.people.noRoles")}</p>
        ) : (
          <ul className="space-y-1">
            {appRoles.map((role) => (
              <li key={`${role.project}/${role.app}/${role.role}`} className="text-body">
                <Link
                  to="/projects/$project/$plural/$name"
                  params={{ project: role.project, plural: "apps", name: role.app }}
                  className="font-semibold underline"
                >
                  {role.app}
                </Link>{" "}
                <span>{role.role}</span>{" "}
                <span className="text-fg-muted">
                  {t("access.roles.project", { name: role.project })} · {viaText(role.via as Via, t)}
                </span>
              </li>
            ))}
          </ul>
        )}
      </Card>

      <ConfirmDialog
        open={asking !== null}
        onOpenChange={(open) => {
          if (!open) setAsking(null);
        }}
        title={asking ? t(`organization.people.confirm.${asking}.title`, { name }) : ""}
        description={asking ? t(`organization.people.confirm.${asking}.body`, { name }) : undefined}
        confirmLabel={asking ? t(`organization.people.action.${asking}`) : ""}
        tone={asking === "delete" || asking === "disable" ? "danger" : "primary"}
        pending={act.isPending}
        onConfirm={() => {
          if (asking) act.mutate(asking);
        }}
      />
      {/* Mounted while open, so it starts from the person as they are now. */}
      {editing ? <EditPersonDialog person={person} open onOpenChange={setEditing} /> : null}
      <OneTimePassword secret={secret} onClose={() => setSecret(null)} />
    </div>
  );
}

interface Scope {
  organization?: string;
  project?: string;
  contextSpace?: string;
}

interface Via {
  user?: string;
  group?: string;
}

function scopeText(scope: Scope | undefined, t: (key: string, options?: Record<string, unknown>) => string): string {
  if (scope?.contextSpace) return t("access.roles.space", { name: scope.contextSpace });
  if (scope?.project) return t("access.roles.project", { name: scope.project });
  return t("access.groupPage.organization");
}

function viaText(via: Via | undefined, t: (key: string, options?: Record<string, unknown>) => string): string {
  return via?.group
    ? t("organization.people.viaGroup", { group: via.group })
    : t("organization.people.direct");
}

/** Edit a person's name, e-mail and language; a changed e-mail is verified again (PF-92). */
/** What an edit changes, and nothing else: an address differing only in case is the same one, so it asks for no new verification. */
export function changedFields(person: Person, next: PersonForm): components["schemas"]["EditPerson"] {
  const body: components["schemas"]["EditPerson"] = {};
  if (next.email.trim().toLowerCase() !== person.email.toLowerCase()) body.email = next.email.trim();
  if (next.firstName.trim() !== person.firstName) body.firstName = next.firstName.trim();
  if (next.lastName.trim() !== person.lastName) body.lastName = next.lastName.trim();
  if (next.locale && next.locale !== (person.locale ?? "")) body.locale = next.locale;
  return body;
}

function EditPersonDialog({
  person,
  open,
  onOpenChange,
}: {
  person: Person;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}): JSX.Element {
  const { t } = useTranslation();
  const formId = useId();
  const queryClient = useQueryClient();
  const [form, setForm] = useState<PersonForm>({
    email: person.email,
    firstName: person.firstName,
    lastName: person.lastName,
    locale: person.locale ?? "",
  });
  const [tried, setTried] = useState(false);
  const errors = tried ? validate(form, t) : {};

  const edit = useMutation({
    mutationFn: async (next: PersonForm) => {
      const body = changedFields(person, next);
      return unwrap(
        await api.PATCH("/api/v1/organization/people/{id}", { params: { path: { id: person.id } }, body }),
      );
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: queryKeys.people() });
      close(false);
    },
  });

  const close = (next: boolean) => {
    if (!next) edit.reset();
    onOpenChange(next);
  };

  return (
    <Dialog
      open={open}
      onOpenChange={close}
      title={t("organization.people.editTitle", { name: fullName(person) })}
      description={t("organization.people.editLead")}
      closeLabel={t("app.close")}
      footer={
        <>
          <Button variant="secondary" onClick={() => close(false)}>
            {t("app.cancel")}
          </Button>
          <Button type="submit" form={formId} disabled={edit.isPending} disabledReason={t("app.busyReason")}>
            {t("organization.people.save")}
          </Button>
        </>
      }
    >
      <form
        id={formId}
        noValidate
        className="space-y-4"
        onSubmit={(event) => {
          event.preventDefault();
          setTried(true);
          if (Object.keys(validate(form, t)).length === 0) edit.mutate(form);
        }}
      >
        <PersonFields form={form} errors={errors} onChange={setForm} />
        {edit.error ? (
          <Alert tone="danger" role="alert">
            {reasonOf(edit.error, t("app.error.generic"))}
          </Alert>
        ) : null}
      </form>
    </Dialog>
  );
}
