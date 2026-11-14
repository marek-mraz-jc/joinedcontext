# kaisatsu

Identity server (replaces Keycloak and FIWARE VCVerifier): OIDC, OID4VCI, OID4VP, SD-JWT VC, did:web, LDAP and SAML brokering, multi-organization. Held: T-6633, research T-6637.

## Second factor at sign-in

One check, `kaisatsu_oidc::second_factor_required`, decides for the login pages, the password,
refresh, device-code and token-exchange grants, silent SSO and `prompt=none` at `/authorize`,
device approval, brokered (upstream IdP) sign-ins and the account console. A person must pass a second factor when any
of these holds (strictest wins, nothing lowers another):

- the deployment floor `[kaisatsu] second_factor = "required"` in `jc.toml` (default `optional`);
- the realm is `master`;
- the person holds the effective realm role `mfa-required`;
- an Organization the person belongs to, or one of the person's groups (the Portal writes it on
  the groups carrying an Organization's `jc_org`), carries the attribute `jc.secondFactor` with
  any value other than `optional` (an unknown value asks the factor);
- the client asks for `acr_values=mfa` (login pages only).

A person without an authenticator is sent to enrolment at the next sign-in. New
authenticators use HmacSHA1, 6 digits, 30 s (RFC 6238), which every authenticator app computes;
stored credentials keep their own algorithm.

Turning a rule on ends the live sessions it newly covers: the next refresh of a session signed
in without a second factor (refresh and offline tokens alike) is refused with `invalid_grant`,
and every session and refresh family of that person is revoked, so the app signs the person in
again and the login asks for the second factor. An SSO session signed in without one mints no
code (silent SSO signs in again, `prompt=none` answers `login_required`) and approves no device;
a subject token without one exchanges for nothing. A brokered sign-in counts as the first factor:
the person goes on to the local one-time code or its enrolment. The refresh rule holds for every
rule above, with the floor `optional` too: a holder of `mfa-required` or a member of an
Organization asking it, whose session passed no second factor (the role granted after sign-in, a
brokered session from before brokered sign-ins asked), signs in again. Service accounts (client_credentials,
jwt-bearer) never ask. The first sign-in after the switch enrols whoever holds the password.
