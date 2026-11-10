# minted-cleanup

Removes the 244,837 entities jc-prod's pipelines minted under
`urn:ngsi-ld:<Type>:portal.smartcity.joinedcontext.com:` on 2026-11-10 11:49:54-12:29:26.879Z, when
the org domain was unset (T-6676 section R, fixed in deployment e4dd3cde). The copies go through each
space's Endpoint as a temporary ServiceAccount `cleanup-t6676` per project holding one temporary
Policy per space: deleteBatch only, one selector per minted type, id pattern
`^urn:ngsi-ld:<Type>:portal\.smartcity\.joinedcontext\.com:.+$`, valid for a few hours. No raw SQL
delete, no import window. `minted_cleanup.py --help` and the module docstring describe the checks;
`python3 -m unittest discover -s bins/jc/deploy/minted-cleanup` runs the selftests against a stub
gateway (ci.yml runs them in `changes`). The real-engine proof is in
/workspace/evidence/jc-cleanup-fix/REPORT.md.

Why the pattern has both anchors and the `.+` tail: jc-core's `EntitySelector::validate` refuses an
idPattern without `^` and `$` (the Portal's dryRun answers `valid: false`), Antares compiles the
pattern as `^(?:p)$` (antares-api policy.rs `Rule::selects_id`), and the gateway matches it
unanchored (context-gateway pdp/id_match.rs). With both anchors and the tail, both layers select
exactly the ids under the prefix and refuse the bare prefix, a longer domain
(`...joinedcontext.com.evil:`) and every `hel.fi` id.

## 1. Reference count

Read-only, 2026-11-10 14:01Z and 14:13Z, identical apart from the read time
(`evidence/jc-cleanup-fix/counts-1401Z.tsv`, `counts-1413Z.tsv`):

| Tenant | Project | Copies | Endpoint |
|---|---|---:|---|
| bratislava | bratislava | 4,589 | bratislava `rbamgl4sqke6y5gws3mdgj3qe2zhw7ev` |
| barcelona | barcelona | 11,074 | barcelona `pbyqbuyoeikryocvqb5uev7nxbx5mr4a` |
| helsinki | helsinki | 101,293 | helsinki-all `si6epqkx364lprho5uaigutk274r5grb` |
| praha-mesto | praha | 127,881 | praha-mesto `jw7tffshzip7igmzcszgffkkkuetnc6v` |
| total | | 244,837 | |

The 12:30Z file (`evidence/jc-prod/minted-prefix-counts-1230Z.tsv`, 226,193, newest 12:28:47Z) was
read while praha-mesto still minted: its 18,644 later copies are the difference. History under the
prefix: 7,174 temporal_entities, 34,771 attr_instances, no attr_versions, history_cutover unset
(attr_rows unused). Rows outside the prefix pointing at a copy: 0. Slugs from the frozen export
prod-20261109T1647Z: read each back on jc-prod (step 2.3).

## 2. Runbook (deployer, one project at a time)

`W` is a 0700 work directory on the deployer's host. `M=bins/jc/deploy/minted-cleanup`. Every SQL
file runs read-only against `jc_antares` as
`psql -q -At -F "$(printf '\t')" -v ON_ERROR_STOP=1 -f $M/sql/<file>.sql`, written below as
`sql <file> > out`. `G=https://<jc-prod Portal host>`, `T=$G/id/realms/smartcity/protocol/openid-connect/token`.
`MEM` is a command printing the node memory % last (for instance
`KUBECONFIG=... kubectl top node --no-headers | awk '{print $5}'`).

1. **Inputs.** `sql list > W/list.tsv`; its lines per tenant equal the table above (244,837).
   `sql sample > W/sample-before.tsv` (the real ids whose md5 starts with 000, a fixed sample of
   about one in 4096; the 12:45Z dump holds the same rows: nothing outside the prefix was deleted
   since). Write `W/endpoints.tsv`: tenant, slug, the space the Endpoint names, project (tabs, the
   four rows of the table).
2. **Account and Policies, per project.**
   1. `python3 $M/minted_cleanup.py policies --list W/list.tsv --endpoints W/endpoints.tsv --state W/st --until <now+6h>Z --owner <the deployer steward's user id>`
      writes `W/st/serviceaccount-<project>.json` (role `public`, the anonymous reader's, in the
      project's affected space until `--until`: jc-core refuses an account without a role) and
      `W/st/policy-<space>.json`.
   2. Propose each as a Change, `?dryRun=All` first (`valid: true` expected), then for real
      (`POST $G/api/v1/projects/<p>/serviceaccounts`, then `.../policies`); a second person approves.
   3. Read back: the ServiceAccount (client `<project>-cleanup-t6676`), each Policy with operations
      `[deleteBatch]`, assignee `serviceAccount cleanup-t6676`, `validity.to`, and one
      `^urn:ngsi-ld:<Type>:portal\.smartcity\.joinedcontext\.com:.+$` per type of that space only;
      each Endpoint's `spec.contextSpaceRef` equals the tenant and it has no `policyRef`.
   4. Issue the secret once in a Portal session with `approve` on ServiceAccount
      (`POST .../serviceaccounts/cleanup-t6676/secret`) into `W/secret-<project>` (0600). The tool
      fetches its own client-credentials token from it, again past half of `expires_in`; the
      secret never goes into argv, a log or W/st.
3. **Dry run.** `python3 $M/minted_cleanup.py run --list W/list.tsv --endpoints W/endpoints.tsv --state W/st`
   prints listed, layers and batches per tenant; nothing is sent.
4. **Pointers.** `sql pointers` prints nothing. A row outside the prefix naming a copy makes the
   gateway's HeldDelete refuse that copy (Conflict) and the run stops on it; such a row is real
   data pointing at a copy and needs a decision first.
5. **Canary, bratislava (the smallest):**
   `python3 $M/minted_cleanup.py run --list W/list.tsv --endpoints W/endpoints.tsv --state W/st --only bratislava --apply --gateway $G --token-url $T --client-secret-file W/secret-bratislava --mem-cmd "$MEM" --max-batches 1`.
   It ends with `paused after 1 batch(es) ...: 500 deleted` and exit 0.
6. **Recount after the canary.**
   - `sql sample > W/sample-now.tsv; LC_ALL=C comm -23 W/sample-before.tsv W/sample-now.tsv`
     prints nothing (no real id gone).
   - `LC_ALL=C comm -23 <(sort W/st/done.txt) <(cut -f2 W/list.tsv | sort)` prints nothing: every
     id sent was a listed copy. The gateway logs no ids, so this, the Policies read back in 2.3 and
     the 403 below are the proof that nothing outside the prefix was deleted.
   - A delete of one real id as the account answers 403: `curl -H @W/hdr ... -X POST --data '["<a real id of the sample>"]' $G/api/endpoint/<slug>/ngsi-ld/v1/entityOperations/delete`,
     the `Authorization: Bearer` line written into `W/hdr` (0600) from a token fetched with the
     secret, never on the command line.
   - `sql history > W/history.tsv; LC_ALL=C join <(sort W/st/done.txt) <(cut -f2 W/history.tsv | sort)`
     prints nothing: no history left for a deleted id (step 8 covers the whole run).
7. **Full runs, per tenant:** the line of step 5 without `--max-batches`, for bratislava, then
   barcelona, helsinki, praha-mesto (each with its own project's secret file; one run serves one
   project). Run `sql pointers` before each. A stop prints why on its last line; fix the cause and
   rerun the same line: it resumes from W/st/done.txt and W/st/inflight.txt. At 95 % node memory
   it stops before the next batch.
8. **Verify.** `sql recount > W/recount.tsv; sql history > W/history.tsv;`
   `python3 $M/minted_cleanup.py verify --recount W/recount.tsv --history W/history.tsv --state W/st`:
   0 copies and 0 rows in temporal_entities, attr_instances, attr_versions and attr_rows under the
   prefix pass. jc-prod's recorder runs on the NATS bus: it deletes history after the entity and
   only logs a failure, so when history is left, wait 5 minutes and run step 8 again before
   calling it left. `history-left-after-delete.tsv` lists ids whose history stayed (for
   @supervisor); `history-for-sweep.tsv` lists expired copies waiting for the broker's sweep.
   Repeat the sample check of step 6.
9. **Remove the grant.** Propose and approve the deletion of each `Policy cleanup-t6676`, then of
   each `ServiceAccount cleanup-t6676`. Read back: each GET answers 404,
   `GET $G/api/v1/projects/<p>/policies?q=cleanup-t6676` lists none, no `<project>-cleanup-t6676`
   client is left in the realm, and the token request with the old secret answers 401.
   `validity.to` (Policy) and `notAfter` (role) end the grant by themselves if this step is late.
10. **Evidence:** a before/after table per tenant (copies, temporal_entities, attr_instances from
    `sql history`, sample present), `W/st/log.txt`, the verify lines and the delete list; no entity
    values, token or secret.

## 3. History in the window

Deleting a copy deletes its history (by exact id; the counterpart's history is untouched, shown on
the real engine). On jc-prod the counterparts got almost no observations in the window (the
coverage rows of counts-1413Z.tsv: 147 instances against the copies' 34,771), so the observations
pipelines wrote 11:49-12:29Z sit only on the copies. `evidence/jc-cleanup-fix/readonly-loss.sql`
splits the copies' instances into those the counterpart already holds (same attribute and time)
and those unique to the copy. The owner decides before step 7: accept the unique ones as lost, or
copy them onto the `hel.fi` counterparts through the temporal API (`POST .../temporal/entities/
{counterpart}/attrs`) before deleting.

## 4. Rollback of a wrong delete

`selective_restore.sh <dump> <ids.tsv> <stage conninfo> <target conninfo> [--apply]` puts back the
entities rows and their history for the listed (tenant, id) only, from a `pg_dump -Fc` of
`jc_antares` taken before the run (the 12:45Z dump, age-encrypted, held by the deployer). It
restores the dump whole into an empty stage database on a scratch server (PostGIS 17-3.5, plus
timescaledb when `pg_restore --list` names it), reads the rows of the listed ids, and inserts them
into the target in one transaction as a superuser with `ON CONFLICT DO NOTHING` (generated columns
and the entity_values/entity_attrs/entity_types index rows are rebuilt by the database). Without
`--apply` it rolls back after printing the counts. Restoring the whole dump onto jc-prod would roll
back every write after 12:45Z; never do that. Passwords come from PGPASSFILE (0600), never a
conninfo. Tested on a synthetic dump of a rehearsal instance (REPORT.md); the real dump is only
opened by the deployer, on a scratch host.
