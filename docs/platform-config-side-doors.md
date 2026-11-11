# Side doors that write live state or main

Findings P-14, P-22, P-23 and P-24 of the platform configuration-as-code critic (2026-11-11), task T-6715.
The rule (CHG-12, CFM-01): live state and `main` move through a Change. This page lists each path that wrote
either one without a Change, what now holds it, and why the paths that remain stay.

## 1. Closed

| Path | Before | Now | Proof |
|---|---|---|---|
| `jcctl apply --repo-dir … --gateway-url` (seed replay) | wrote seed entities from any directory, no `validate`, no branch check | refused unless the directory validates and is a clean checkout whose `HEAD` is `origin/main` (`jcctl::commands::reviewed::check`); without a gateway `apply` writes nothing live and keeps reading any tree | `jcctl` `reviewed_tests` |
| `jcctl publish ckan` | wrote a live catalogue from any directory | the same gate, before the first CKAN call | `reviewed_tests::apply_and_publish_refuse_a_working_copy_nobody_merged` |
| Realm apply on start (`kaisatsu-import` `apply_realm_files`) | every apply whose files changed (an upgrade, a new public URL) hashed the setup password again and moved the person's own to history | a credential type is seeded only while the user holds none: first creation, or a user who never had one | `kaisatsu-import` `a_restart_keeps_a_password_changed_after_the_user_was_created` |
| Portal merge in shako | merge commit authored and committed by `admin` | `Shako.Merge` takes `Author`; the Portal sends the approving person, shako keeps its own caller as committer, so the forge history names both | `shako-server` `portal_contract::merge_requests_reviews_merge_and_webhooks` |
| Branch rule removal | the Portal's `admin` token could delete or loosen any rule through `/api/v1/…/branch_protections` or `Shako.Unprotect` | a rule that guards writers (push off, a push allow-list or a merge allow-list) is never deleted through the API and never loosened; tightening and rules that guard nobody stay open | `portal_contract::the_portal_token_cannot_remove_or_loosen_a_branch_rule` |

## 2. Listed, with the reason they stay

- **Reconciler commits on `main` (P-22).** Two steps of `reconciler/daemon.rs` still write to the default branch:
  step 7 `publish_roles` (CODEOWNERS and the role bindings, compiled from `users/`) and step 7a
  `drop_retired_representations` (removes `csv`-era representation names from Endpoint and App manifests). Both
  are deterministic functions of content that already reached `main` through a Change, and both write only when
  the file differs, so a second run commits nothing. Moving them to a Change needs a propose-and-merge path for
  the daemon (the `merge_if_only` pattern the build lane uses), which the daemon does not hold today. Parked as
  `later/platform-config-reconciler-commits-as-changes.md`.
- **Shako identity (P-24, partly).** The Portal still reaches shako with one static token bound to `admin`. The
  owner's decision is an audience-bound token exchange per person. That needs kaisatsu to issue a shako-audience
  token for the person, shako to trust kaisatsu's JWKS, and per-person bindings in shako, which today exist only
  in the Portal (PF-50). Until then: the person is the merge author (section 1), the Portal is the committer, and
  the token cannot remove or loosen protection. Repository creation keeps `admin`. Two merges into an App's own
  repository (`agents/repository.rs` publish, `api/app_dashboard.rs` dashboard edit) and the platform's own
  merges (`sync/proposal.rs` auto-merge, `api/projects.rs` lax open) send no `Author`: the first two hold the
  approver only as a login string, which the merge message already names; the last two have no person.
- **Database-only state (P-23, CFM-01 "two homes").** These live only in the database by design and are not
  drift: people and their credentials (kaisatsu users, sessions, the account console), group membership the
  Portal grants (`api/people.rs` calls to the realm admin API), sync pauses (`sync_source_state.paused`), access
  pauses, drafts and workspace state before a Change is opened. The realm files `jc` applies on start still own
  the jc clients and the first administrator's existence; a person edits a jc-owned client only through a jc
  upgrade.

## 3. Decisions

Decided (worker-t6715, owner may override):

- The live-write gate compares with the local `origin/main` ref and does not fetch: a stale ref still names a
  reviewed commit, and the gate never touches the network. No `--allow-dirty` override: nobody needs one today
  (no script or Job runs either command from a working copy), and one would reopen the door.
- A guarded branch rule has no API path to removal at all. An operator who must drop one deletes the repository
  or edits the shako database; there is no operator role in shako yet to grant it to.
- Credentials: kept per type (password, otp, webauthn). A password the person reset through the account console
  wins over the setup file for the life of the user.

## Related

- `/workspace/supervisor/reports/platform-config-critic-20261111.md` (P-14, P-22, P-23, P-24)
- `crates/shako/PORTAL-ODATA.md` row 28 (`Shako.Merge` `Author`)
- T-6706 (only the Portal merges into main; `portal/*` and `workspace/*` reserved)
