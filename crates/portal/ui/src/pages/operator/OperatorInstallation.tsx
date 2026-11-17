import { useState } from "react";
import type { FormEvent, JSX } from "react";
import { Link } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { ApiError, reasonOf } from "../../api/client";
import { Alert, Badge, Button, Card, CardHeader, Field, Input, PageFailed, PageLoading, Select } from "../../components/ui";
import { operatorInstallation, operatorKeys, proposeChange, sentence, sendTestEmail } from "./operatorApi";
import type { SmtpServer } from "./operatorApi";

/**
 * Administration → Installation (T-7152): what the installation provides every Organization, set
 * by an operator without the cluster. The e-mail server is an operator Change: a second operator
 * approves it, and the password goes to the sign-in service sealed and is never shown again, only
 * whether one is stored. The test e-mail goes to the operator's own address.
 */
export function OperatorInstallation(): JSX.Element {
  const { t } = useTranslation();
  const installation = useQuery({ queryKey: operatorKeys.installation, queryFn: operatorInstallation });
  if (installation.isPending) return <PageLoading label={t("operator.installation.loading")} />;
  if (installation.isError) return <PageFailed error={installation.error} onRetry={() => void installation.refetch()} />;
  return (
    <section className="space-y-6" aria-labelledby="operator-installation-heading">
      <div>
        <h2 id="operator-installation-heading" className="text-title font-semibold text-fg">
          {t("operator.installation.title")}
        </h2>
        <p className="text-body text-fg-muted">{t("operator.installation.lead")}</p>
      </div>
      {/* Keyed by what is saved: an approved change starts the form again from it. */}
      <EmailServer
        key={JSON.stringify(installation.data.smtp ?? null)}
        smtp={installation.data.smtp ?? undefined}
        open={installation.data.smtpChange ?? undefined}
      />
    </section>
  );
}

type Tls = "starttls" | "ssl";

function EmailServer({ smtp, open }: { smtp?: SmtpServer; open?: string }): JSX.Element {
  const { t } = useTranslation();
  const client = useQueryClient();
  const [host, setHost] = useState(smtp?.host ?? "");
  const [port, setPort] = useState(String(smtp?.port ?? 587));
  const [tls, setTls] = useState<Tls>(smtp?.tls === "ssl" ? "ssl" : "starttls");
  const [from, setFrom] = useState(smtp?.from ?? "");
  const [user, setUser] = useState(smtp?.user ?? "");
  const [password, setPassword] = useState("");
  const refresh = () => client.invalidateQueries({ queryKey: ["operator"] });
  const propose = useMutation({ mutationFn: proposeChange, onSuccess: () => setPassword(""), onSettled: refresh });
  const test = useMutation({ mutationFn: sendTestEmail, onSettled: refresh });
  // The server names the field it refuses ("smtp.host: …"): shown under that field.
  const said = (error: unknown) => (error ? reasonOf(error, t("operator.tryAgain")) : "");
  const fieldOf = (field: string): string[] | undefined => {
    const name = `smtp.${field}:`;
    const lines = propose.error instanceof ApiError ? (propose.error.problem?.errors ?? []) : [];
    const line = [...lines, said(propose.error)].find((text) => text.startsWith(name));
    return line ? [sentence(line.slice(name.length).trim())] : undefined;
  };
  const fields = ["host", "port", "tls", "from", "user", "password"];
  const elsewhere = propose.isError && !fields.some((field) => fieldOf(field));
  const changed =
    !smtp ||
    password !== "" ||
    host.trim().toLowerCase() !== smtp.host ||
    Number(port) !== smtp.port ||
    tls !== smtp.tls ||
    from.trim() !== smtp.from ||
    user.trim() !== (smtp.user ?? "");
  const onTls = (value: Tls) => {
    setTls(value);
    // The port that goes with each protection, unless one of the operator's own is typed.
    if (port === "587" || port === "465" || port === "") setPort(value === "ssl" ? "465" : "587");
  };
  const onSave = (event: FormEvent) => {
    event.preventDefault();
    propose.mutate({
      kind: "SmtpSet",
      smtp: {
        host: host.trim().toLowerCase(),
        port: Number(port),
        tls,
        from: from.trim(),
        user: user.trim(),
        password: password === "" ? undefined : password,
      },
    });
  };

  return (
    <Card>
      <CardHeader
        title={t("operator.installation.email.title")}
        description={t("operator.installation.email.lead")}
        actions={
          open ? (
            <Badge tone="info">{t("operator.installation.email.waitingBadge")}</Badge>
          ) : (
            <Badge tone={smtp ? "success" : "warning"}>
              {smtp ? t("operator.installation.email.set") : t("operator.installation.email.unset")}
            </Badge>
          )
        }
      />
      {open ? (
        <Alert tone="info" className="mt-4">
          {t("operator.installation.email.waiting")}{" "}
          <Link to="/organization/$tab" params={{ tab: "operator-changes" }} className="underline">
            {t("operator.installation.email.toChanges")}
          </Link>
        </Alert>
      ) : null}
      {!smtp && !open ? (
        <p className="mt-4 text-body text-fg-muted">{t("operator.installation.email.empty")}</p>
      ) : null}
      <form className="mt-4 grid max-w-3xl gap-4 sm:grid-cols-2" onSubmit={onSave} aria-label={t("operator.installation.email.title")}>
        {/* Read-only while a change waits: it is decided first, then the next one is asked for. */}
        <fieldset className="contents" disabled={Boolean(open)}>
        <Field id="smtp-host" label={t("operator.installation.email.host")} help={t("operator.installation.email.hint.host")} errors={fieldOf("host")} required>
          <Input id="smtp-host" value={host} onChange={(e) => setHost(e.target.value)} autoComplete="off" spellCheck={false} required />
        </Field>
        <Field id="smtp-tls" label={t("operator.installation.email.tls")} help={t("operator.installation.email.hint.tls")} errors={fieldOf("tls")}>
          <Select id="smtp-tls" value={tls} onChange={(e) => onTls(e.target.value as Tls)}>
            <option value="starttls">{t("operator.installation.email.tlsStarttls")}</option>
            <option value="ssl">{t("operator.installation.email.tlsSsl")}</option>
          </Select>
        </Field>
        <Field id="smtp-port" label={t("operator.installation.email.port")} help={t("operator.installation.email.hint.port")} errors={fieldOf("port")} required>
          <Input id="smtp-port" type="number" inputMode="numeric" min={1} max={65535} value={port} onChange={(e) => setPort(e.target.value)} required />
        </Field>
        <Field id="smtp-from" label={t("operator.installation.email.from")} help={t("operator.installation.email.hint.from")} errors={fieldOf("from")} required>
          <Input id="smtp-from" type="email" value={from} onChange={(e) => setFrom(e.target.value)} autoComplete="off" spellCheck={false} required />
        </Field>
        <Field id="smtp-user" label={t("operator.installation.email.user")} help={t("operator.installation.email.hint.user")} errors={fieldOf("user")}>
          <Input id="smtp-user" value={user} onChange={(e) => setUser(e.target.value)} autoComplete="off" spellCheck={false} />
        </Field>
        <Field
          id="smtp-password"
          label={t("operator.installation.email.password")}
          help={smtp?.passwordSet ? t("operator.installation.email.hint.passwordKept") : t("operator.installation.email.hint.password")}
          errors={fieldOf("password")}
        >
          <Input
            id="smtp-password"
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            autoComplete="new-password"
            placeholder={smtp?.passwordSet ? t("operator.installation.email.passwordStored") : undefined}
          />
        </Field>
        </fieldset>
        <div className="flex flex-wrap items-center gap-3 sm:col-span-2">
          <Button
            type="submit"
            loading={propose.isPending}
            disabled={Boolean(open) || !changed}
            disabledReason={open ? t("operator.installation.email.waitingShort") : t("operator.installation.email.unchanged")}
          >
            {t("operator.installation.email.propose")}
          </Button>
          <Button
            type="button"
            variant="secondary"
            loading={test.isPending}
            disabled={!smtp}
            disabledReason={t("operator.installation.email.testFirst")}
            onClick={() => test.mutate()}
          >
            {t("operator.installation.email.test")}
          </Button>
          {!smtp && !open ? <p className="text-caption text-fg-muted">{t("operator.installation.email.testFirst")}</p> : null}
        </div>
      </form>
      {elsewhere ? (
        <Alert tone="danger" className="mt-4">
          {sentence(said(propose.error))}
        </Alert>
      ) : null}
      {propose.isSuccess && !open ? (
        <Alert tone="success" className="mt-4">
          {t("operator.installation.email.proposed")}{" "}
          <Link to="/organization/$tab" params={{ tab: "operator-changes" }} className="underline">
            {t("operator.installation.email.toChanges")}
          </Link>
        </Alert>
      ) : null}
      {test.isError ? (
        <Alert tone="danger" className="mt-4">
          {sentence(said(test.error))}
        </Alert>
      ) : null}
      {test.isSuccess ? (
        <Alert tone={test.data.sent ? "success" : "danger"} className="mt-4" title={test.data.sent ? undefined : t("operator.installation.email.testFailed")}>
          {test.data.sent ? t("operator.installation.email.testSent") : sentence(test.data.reason ?? t("operator.tryAgain"))}
        </Alert>
      ) : null}
    </Card>
  );
}
