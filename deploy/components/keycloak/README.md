# Keycloak

Central identity management of the platform

Next steps:
1. Configure helm charts in `components/keycloak/charts.yaml`
2. Configure values for every release in `components/keycloak/values`
3. Update your README.md
4. Update component to `defaults/environment/global.yaml` - `components` list

Deploy the component with:
$ # navigate to `deployment/` and run:
$ helmfile apply -i --selector component=keycloak

## Login hardening (T-5120)

The realm import (`values/config/base-values.yaml.gotmpl`) sets the same rules on dev and production;
`tests/test_keycloak_login_hardening.py` asserts them for the dev, production and smartcity renders.

| Setting | Value |
|---|---|
| Second factor | TOTP required for every holder of realm role `mfa-required`: portal-approver and platform-admin carry it as a composite, and it can be granted to anyone directly. A holder without TOTP enrols at the next login (Keycloak's CONFIGURE_TOTP page). Everyone else is asked for TOTP or WebAuthn only once they configured one. |
| Step-up (T-5567) | The password is level 1 (`acr` `pwd`); a login asking `acr_values=mfa` runs the level-2 subflow `custom browser step-up`, which asks anyone for TOTP and enrols who has none, and the token then carries `acr` `mfa`. Every OTP step has its own `otp reference …` config, so `amr` names `otp` whichever subflow asked the code (one alias per step: keycloak-config-cli binds a config to one execution). Every platform client carries the `oidc-amr-mapper`. The Portal refuses every decision without one of the two (`403 second-factor-required`) and offers `GET /api/v1/auth/login?step_up=true`. |
| Master realm | Browser flow `master browser`: password and TOTP for every administrator, CONFIGURE_TOTP at the first login after the change. The direct grant keycloak-config-cli uses is Keycloak's own and stays password-only. |
| Password grant | Flow `custom direct grant` asks the same: a holder of `mfa-required` sends `totp=<code>` with the password, and one who has not enrolled gets no token (Keycloak answers `unknown_error` and logs an AuthenticationFlowException of direct-grant-validate-otp; enrolment happens in the browser). Everyone else sends a code only once they configured TOTP. |
| TOTP | HMAC-SHA256, 6 digits, 30 s, look-ahead 1. Use an app that honours the algorithm (FreeOTP, Aegis, 1Password, KeePassXC); some versions of Google Authenticator compute SHA-1 codes and every code fails. |
| Brute force | The 10th wrong password within 12 h locks the account for 60 s; every further 10 add 60 s, up to 15 min. A retry faster than 1 s after a failure waits 60 s. Never a permanent lockout. |
| Password policy | 12+ characters with an upper, a lower and a digit; not the username, not the e-mail, none of the last 5. |
| SSO session | 60 min idle, 10 h at most; no remember-me. |
| Access token | 5 min (production profile); 60 min where the development values apply (dev, and smartcity through `valuesProfile: development`). Refresh tokens live as long as the SSO session. |

Demo people holding an MFA role (demo.steward, demo.approver) get a generated TOTP seed beside their
password in Secret `keycloak-user-<name>`, key `totp`. The journeys compute the code from it
(`scripts/smoke-forge-login.sh`, `scripts/keycloak-login-journey.py`, the Portal's live journeys);
a real person's factor is never seeded. Read a seed only into an environment variable, never into
a file, log or task.

Check the rules in a browser after an apply (the throwaway checks create and delete their own user):

```bash
export JC_LOGIN_PASSWORD=... JC_LOGIN_TOTP=...           # keycloak-user-demo-steward: password, totp
scripts/keycloak-login-journey.py --base https://<keycloak host> --realm <slug> --user demo.steward@<org> steward
export KC_ADMIN_USER=... KC_ADMIN_PASSWORD=...           # keycloak-admin-user
scripts/keycloak-login-journey.py --base https://<keycloak host> --admin-base http://localhost:8080 --realm <slug> enrol
scripts/keycloak-login-journey.py --base https://<keycloak host> --admin-base http://localhost:8080 --realm <slug> lockout
scripts/keycloak-login-journey.py --base https://<keycloak host> --admin-base http://localhost:8080 --realm <slug> stepup
export JC_GRANT_CLIENT_ID=apisix-gateway JC_GRANT_CLIENT_SECRET=...  # its client Secret
scripts/keycloak-login-journey.py --base https://<keycloak host> --admin-base http://localhost:8080 --realm <slug> grant
```

### Break-glass

The `master` realm's admin (Secret `keycloak-admin-user`) is the way back in when a steward loses
their phone or locks themselves out. It is reached only from inside the cluster with the kubeconfig,
which lives outside every repository; the password is never written down anywhere else.

```bash
kubectl port-forward -n <slug> svc/keycloak-app-keycloakx-http 8080:80 &
# -it and no --password: kcadm prompts for it, so the password stays out of every process list
# and shell history.
kubectl exec -it -n <slug> deploy/keycloak-app-keycloakx -- /opt/keycloak/bin/kcadm.sh config credentials \
  --server http://localhost:8080 --realm master --user admin
# lift a lockout:            kcadm.sh delete attack-detection/brute-force/users/<user-id> -r <slug>
# make a lost TOTP re-enrol: kcadm.sh delete users/<user-id>/credentials/<otp-credential-id> -r <slug>
```

Each step is in the admin event log (`adminEventsEnabled`); name the person and the reason in the
task or incident that needed it.

## People under fixed ids (PF-126)

Manifests name a person by their Keycloak user id, never by e-mail (ADR-N-061). The seeds name
the demo people of the development profile by the fixed ids in `demo-users.yaml`, and a
production realm's owner by `global.initialUserId`. keycloak-config-cli drops an id it is given,
so the `people` part (`charts/people`) runs before the realm import and creates each of them
through the realm's partial import, which keeps it. A demo person under another id is deleted
and created again under theirs (the import then sets their password and second factor from
their Secrets again); an owner under another id is never deleted: the Job fails and names the id
to set in `global.initialUserId`. `tests/test_keycloak_people.py` runs the script in the pinned
Keycloak.

## The presenter (T-6542)

`presenter@hel.fi` is the one login the owner presents with on dev, stage and smartcity: the same
id (`global.presenterUserId`), groups and no realm role on all three (`presenter.yaml`), bound by
the forge seeds beside the demo steward and each other Organization's administrator, and never
deleted by the `people` Job. No TOTP (owner 2026-11-03 04:52Z). Its password is the owner's own dev
admin password (04:48Z), copied as Keycloak stores it: `capture` reads the hashed credential on dev
into `presenter.enc.yaml` (sops, the age recipients of `.sops.yaml`), `load` writes it into each
realm by partial import. The realm import gives the person no credential, so keycloak-config-cli
leaves the copied one alone, and until `load` ran nobody can sign in as the presenter.

```bash
KUBECONFIG=.secrets/kubeconfig-dev.yaml \
    scripts/presenter-secret.sh capture --realm <dev slug> --from <owner's dev username>
SOPS_AGE_KEY_FILE=.secrets/age-<instance>.key KUBECONFIG=.secrets/kubeconfig-<instance>.yaml \
    scripts/presenter-secret.sh load --instance <instance> --realm <global.instanceSlug>
```
