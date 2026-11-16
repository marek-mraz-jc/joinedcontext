//! `jc kaisatsu split-realm` (T-7128, evidence/T-7128/design.md §3): copies each person of the
//! installation realm into the realm of every Organization they belong to, with their password
//! hash and one-time-code seed as stored, then retires the source account once the copy can sign
//! in. Store to store in this process: no credential is written to a file, a log or the output,
//! which prints counts per realm and nothing that names a person.
//!
//! Who goes where: a member of the Organization's `{org}_bound` group (its Organization-scoped
//! RoleBindings) or of `/{org}/members` (its `Group` manifest), as the Portal's reconciler keeps
//! them from Git. Only groups carrying `managed-by: joinedcontext` count: a group somebody made by
//! hand in the console places nobody. A project-only binding (`{project}-readers`, a vendor) keeps
//! the person in the installation realm.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use clap::Parser;
use kaisatsu_core::Store;
use kaisatsu_core::{GroupRepresentation, KaisatsuError, UserRepresentation};

/// The attribute `--retire` sets on a source account it disabled, holding the Organization realms
/// it was copied to; `--restore` re-enables exactly those accounts.
pub const RETIRED_ATTRIBUTE: &str = "jc.splitRealm.retiredTo";

/// The reconciler's mark on the groups it writes (`reconciler::groups::MANAGED_BY`).
const MANAGED_BY: &str = "managed-by";
const MANAGED_VALUE: &str = "joinedcontext";

/// Groups and members read per group; an installation has a few hundred people.
/// ponytail: one page of 100 000, page through `first` once a realm grows past it.
const MAX_MEMBERS: usize = 100_000;

#[derive(Parser, Debug)]
#[command(
    name = "jc kaisatsu split-realm",
    about = "Copy each person of the installation realm into their Organization's realm"
)]
struct Args {
    /// The jc.toml the running jc reads (its database and key-encryption key).
    #[arg(long, env = "JCBIN_CONFIG")]
    config: std::path::PathBuf,
    /// The realm people are copied from; the installation realm when unset.
    #[arg(long)]
    from: Option<String>,
    /// Copy the people (default: a dry run that changes nothing).
    #[arg(long, conflicts_with_all = ["retire", "restore"])]
    apply: bool,
    /// Disable the copied accounts in the source realm and end their sessions.
    #[arg(long, conflicts_with = "restore")]
    retire: bool,
    /// Enable again every source account `--retire` disabled.
    #[arg(long)]
    restore: bool,
}

/// What a run does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    DryRun,
    Apply,
    Retire,
    Restore,
}

/// Counts for one Organization realm. Never a name, an address or a credential.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RealmCounts {
    /// People the Organization's groups place here.
    pub placed: usize,
    /// Of those, already in the realm under the same id (left as they are).
    pub present: usize,
    /// Copied by this run.
    pub copied: usize,
    /// Of those placed, holding a one-time-code seed in the source realm.
    pub with_otp: usize,
    /// Of those placed, holding a passkey: it is bound to the old sign-in host and does not move.
    pub with_passkey: usize,
    /// Refused by the realm (a username or address another account of it holds).
    pub conflicts: usize,
}

/// Counts of a run.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub realms: BTreeMap<String, RealmCounts>,
    /// People of the source realm no Organization places (operators, people with a project-only
    /// binding, people with none): they stay.
    pub staying: usize,
    /// Source accounts disabled (`--retire`) or enabled again (`--restore`) by this run.
    pub retired: usize,
    pub restored: usize,
    /// Placed people `--retire` leaves enabled: an operator, a passkey holder, or a copy the
    /// reconciler has not put into its Organization's group yet (no `jc_org` in its token).
    pub kept: usize,
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (realm, c) in &self.realms {
            writeln!(
                f,
                "realm {realm}: {} placed, {} already there, {} copied, {} with a one-time code, \
                 {} with a passkey to enrol again, {} refused by a duplicate username or address",
                c.placed, c.present, c.copied, c.with_otp, c.with_passkey, c.conflicts
            )?;
        }
        writeln!(
            f,
            "installation realm: {} staying, {} retired, {} restored, {} placed but kept enabled",
            self.staying, self.retired, self.restored, self.kept
        )
    }
}

/// An error a person can act on; never carries a person's data.
pub type SplitError = String;

fn store_err(e: impl std::fmt::Display) -> SplitError {
    format!("kaisatsu store: {e}")
}

fn managed(group: &GroupRepresentation) -> bool {
    group
        .attributes
        .get(MANAGED_BY)
        .is_some_and(|v| v.iter().any(|v| v == MANAGED_VALUE))
}

/// The ids of the people the reconciler's groups place in `organization`, read in `realm_id`.
async fn placed(
    store: &dyn Store,
    realm_id: &str,
    organization: &str,
) -> Result<BTreeSet<String>, SplitError> {
    let mut ids = BTreeSet::new();
    for path in [
        format!("/{organization}_bound"),
        format!("/{organization}/members"),
    ] {
        let Some(group) = store
            .get_group_by_path(realm_id, None, &path)
            .await
            .map_err(store_err)?
        else {
            continue;
        };
        if !managed(&group) {
            continue;
        }
        for user in store
            .list_group_members(realm_id, None, &group.id, MAX_MEMBERS)
            .await
            .map_err(store_err)?
        {
            ids.insert(user.id);
        }
    }
    Ok(ids)
}

async fn realm_id(store: &dyn Store, name: &str) -> Result<String, SplitError> {
    store
        .get_realm(name)
        .await
        .map_err(store_err)?
        .map(|r| r.id)
        .ok_or_else(|| {
            format!(
                "realm '{name}' is not in kaisatsu's store: start jc with this configuration once \
                 so it creates the Organization realms"
            )
        })
}

/// The copy of `user` for `target`: same id, so `sub` and every binding by user id stay valid;
/// no roles, groups or Organization row of the source realm (the target's reconciler writes its own).
fn copy_for(user: &UserRepresentation, target: &str) -> UserRepresentation {
    let mut copy = user.clone();
    copy.realm_id = target.to_owned();
    copy.org_id = None;
    copy.realm_roles.clear();
    copy.client_roles.clear();
    copy.groups.clear();
    copy.attributes.remove(RETIRED_ATTRIBUTE);
    copy
}

/// Runs `mode` from realm `from` into the realm of each of `organizations` (each one with its own
/// sign-in host, so its realm is named like it).
pub async fn run(
    store: &dyn Store,
    from: &str,
    organizations: &[String],
    bootstrap_group: &str,
    mode: Mode,
) -> Result<Report, SplitError> {
    if organizations.iter().any(|o| o == from) {
        return Err(format!(
            "realm '{from}' is both the source and an Organization's realm: name the installation \
             realm with --from"
        ));
    }
    let source = realm_id(store, from).await?;
    let mut report = Report::default();
    if mode == Mode::Restore {
        report.restored = restore(store, &source).await?;
        return Ok(report);
    }
    // Person id -> the Organization realms (name, id) that place them; and, for `--retire`, the
    // people each Organization realm's own groups hold already.
    let mut placement: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    let mut converged: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for org in organizations {
        let target = realm_id(store, org).await?;
        if mode == Mode::Retire {
            converged.insert(org.clone(), placed(store, &target, org).await?);
        }
        for id in placed(store, &source, org).await? {
            placement
                .entry(id)
                .or_default()
                .push((org.clone(), target.clone()));
        }
        report.realms.insert(org.clone(), RealmCounts::default());
    }
    let operators = match store
        .get_group_by_path(&source, None, &format!("/{bootstrap_group}"))
        .await
        .map_err(store_err)?
    {
        Some(group) => store
            .list_group_members(&source, None, &group.id, MAX_MEMBERS)
            .await
            .map_err(store_err)?
            .into_iter()
            .map(|u| u.id)
            .collect(),
        None => BTreeSet::new(),
    };
    let people = store
        .list_users(&source, None, None, None, None, false, 0, MAX_MEMBERS)
        .await
        .map_err(store_err)?;
    for user in people
        .into_iter()
        .filter(|u| u.service_account_client_id.is_none())
    {
        let Some(targets) = placement.get(&user.id) else {
            report.staying += 1;
            continue;
        };
        let credentials = store
            .list_credentials(&source, &user.id)
            .await
            .map_err(store_err)?;
        let passkey = !store
            .list_webauthn_credentials(&source, &user.id)
            .await
            .map_err(store_err)?
            .is_empty();
        let otp = credentials.iter().any(|c| c.credential_type == "otp");
        let mut ready = true;
        for (org, target) in targets {
            let counts = report.realms.entry(org.clone()).or_default();
            counts.placed += 1;
            counts.with_otp += usize::from(otp);
            counts.with_passkey += usize::from(passkey);
            let there = store
                .get_user_by_id(target, None, &user.id)
                .await
                .map_err(store_err)?;
            match mode {
                Mode::Apply => {
                    if there.is_some() {
                        counts.present += 1;
                    } else if let Err(e) = store.create_user(copy_for(&user, target)).await {
                        if matches!(e, KaisatsuError::Conflict(_)) {
                            counts.conflicts += 1;
                            continue;
                        }
                        return Err(store_err(e));
                    } else {
                        counts.copied += 1;
                    }
                    // A copy with no credential yet (a run stopped between the two writes)
                    // gets the source's; one with its own is never overwritten.
                    if store
                        .list_credentials(target, &user.id)
                        .await
                        .map_err(store_err)?
                        .is_empty()
                    {
                        for credential in &credentials {
                            let mut copy = credential.clone();
                            copy.realm_id = target.clone();
                            store.upsert_credential(copy).await.map_err(store_err)?;
                        }
                    }
                }
                Mode::Retire => {
                    ready &= there.is_some()
                        && converged.get(org).is_some_and(|ids| ids.contains(&user.id));
                }
                Mode::DryRun | Mode::Restore => {
                    counts.present += usize::from(there.is_some());
                }
            }
        }
        if mode == Mode::Retire {
            if !user.enabled {
                continue;
            }
            if !ready || passkey || operators.contains(&user.id) {
                report.kept += 1;
                continue;
            }
            let mut retired = user.clone();
            retired.enabled = false;
            retired.attributes.insert(
                RETIRED_ATTRIBUTE.to_owned(),
                targets.iter().map(|(org, _)| org.clone()).collect(),
            );
            store.update_user(retired).await.map_err(store_err)?;
            store
                .delete_user_sso_sessions(&source, &user.id)
                .await
                .map_err(store_err)?;
            report.retired += 1;
        }
    }
    Ok(report)
}

async fn restore(store: &dyn Store, source: &str) -> Result<usize, SplitError> {
    let mut restored = 0;
    for mut user in store
        .list_users(source, None, None, None, None, false, 0, MAX_MEMBERS)
        .await
        .map_err(store_err)?
    {
        if user.attributes.remove(RETIRED_ATTRIBUTE).is_none() {
            continue;
        }
        user.enabled = true;
        store.update_user(user).await.map_err(store_err)?;
        restored += 1;
    }
    Ok(restored)
}

/// `jc kaisatsu split-realm <args>`: 0 done, 1 an error, 2 bad usage.
pub fn command(args: &[String]) -> u8 {
    let args = match Args::try_parse_from(
        std::iter::once("jc kaisatsu split-realm".to_owned()).chain(args.iter().cloned()),
    ) {
        Ok(args) => args,
        Err(e) => {
            let _ = e.print();
            return if e.use_stderr() { 2 } else { 0 };
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("jc kaisatsu split-realm: tokio runtime: {e}");
            return 1;
        }
    };
    match runtime.block_on(main(args)) {
        Ok(report) => {
            print!("{report}");
            0
        }
        Err(e) => {
            eprintln!("jc kaisatsu split-realm: {e}");
            1
        }
    }
}

async fn main(args: Args) -> Result<Report, SplitError> {
    let cli = crate::config::Cli::try_parse_from([
        std::ffi::OsString::from("jc"),
        "--config".into(),
        args.config.into_os_string(),
    ])
    .map_err(|e| e.to_string())?;
    let cfg = crate::config::Config::resolve(cli).map_err(|e| e.to_string())?;
    let secrets = crate::config::Secrets::load(&cfg).map_err(|e| e.to_string())?;
    let kek = match secrets.module.get(&crate::setup::Secret::KaisatsuKek) {
        Some(given) => given.secret.expose().to_owned(),
        None => {
            let path = cfg
                .state_dir
                .join(crate::setup::Secret::KaisatsuKek.file_name());
            std::fs::read_to_string(&path)
                .map_err(|e| {
                    format!(
                        "{}: {e}: start jc once so it creates the key",
                        path.display()
                    )
                })?
                .trim()
                .to_owned()
        }
    };
    let kek = kaisatsu_crypto::Kek::from_raw_bytes(kek.as_bytes()).map_err(|e| e.to_string())?;
    if cfg.kaisatsu.database.is_none() && cfg.kaisatsu.database_url_file.is_none() {
        return Err(
            "[kaisatsu] names no database: an in-memory store holds nobody to move".to_owned(),
        );
    }
    let store: Arc<dyn Store> = crate::modules::kaisatsu::open_kaisatsu_store(&cfg.kaisatsu, kek)
        .await
        .map_err(|e| e.to_string())?;
    let organizations: Vec<String> = cfg
        .organizations
        .iter()
        .filter(|o| o.hosts.idm.is_some())
        .map(|o| o.name.clone())
        .collect();
    if organizations.is_empty() {
        return Err(
            "no [[organizations]] entry has hosts.idm: no Organization has a realm of its own"
                .to_owned(),
        );
    }
    let from = args.from.unwrap_or_else(|| cfg.realm.as_str().to_owned());
    let mode = match (args.apply, args.retire, args.restore) {
        (true, _, _) => Mode::Apply,
        (_, true, _) => Mode::Retire,
        (_, _, true) => Mode::Restore,
        _ => Mode::DryRun,
    };
    run(
        store.as_ref(),
        &from,
        &organizations,
        crate::setup::BOOTSTRAP_GROUP,
        mode,
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use kaisatsu_core::{CredentialRecord, RealmConfig};
    use kaisatsu_crypto::Kek;
    use kaisatsu_store_memory::MemoryStore;
    use std::collections::HashMap;

    const HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$c2FsdHNhbHQ$aGFzaGhhc2hoYXNoaGFzaA";
    const SEED: &str = "{\"value\":\"JBSWY3DPEHPK3PXP\"}";

    fn person(n: u8) -> String {
        format!("00000000-0000-4000-8000-0000000000{n:02}")
    }

    async fn realm(store: &MemoryStore, name: &str) -> String {
        let id = format!("id-{name}");
        store
            .upsert_realm(RealmConfig {
                id: id.clone(),
                realm: name.to_owned(),
                ..RealmConfig::default()
            })
            .await
            .unwrap();
        id
    }

    async fn group(
        store: &MemoryStore,
        realm: &str,
        path: &str,
        managed: bool,
        members: &[String],
    ) {
        let name = path.rsplit('/').next().unwrap().to_owned();
        let attributes: HashMap<String, Vec<String>> = if managed {
            [(MANAGED_BY.to_owned(), vec![MANAGED_VALUE.to_owned()])].into()
        } else {
            HashMap::new()
        };
        let group = store
            .upsert_group(GroupRepresentation {
                realm_id: realm.to_owned(),
                name,
                path: path.to_owned(),
                attributes,
                ..GroupRepresentation::default()
            })
            .await
            .unwrap();
        for id in members {
            store.add_user_to_group(realm, id, &group.id).await.unwrap();
        }
    }

    async fn user(store: &MemoryStore, realm: &str, n: u8, otp: bool) {
        store
            .create_user(UserRepresentation {
                id: person(n),
                realm_id: realm.to_owned(),
                username: format!("person{n}"),
                email: Some(format!("person{n}@city.example")),
                enabled: true,
                ..UserRepresentation::default()
            })
            .await
            .unwrap();
        let mut credentials = vec![("password", HASH)];
        if otp {
            credentials.push(("otp", SEED));
        }
        for (kind, secret) in credentials {
            store
                .upsert_credential(CredentialRecord {
                    id: format!("{kind}-{n}"),
                    realm_id: realm.to_owned(),
                    user_id: person(n),
                    credential_type: kind.to_owned(),
                    user_label: None,
                    created_date: 1,
                    secret_data: secret.to_owned(),
                    credential_data: "{}".to_owned(),
                    temporary: false,
                })
                .await
                .unwrap();
        }
    }

    /// Realm `dev` with: 1 bound in hel, 2 in hel and a member of bbsk (one account in each), 3 with a
    /// project-only group, 4 an operator bound in bbsk, 5 in hel with a passkey, 6 only in a group
    /// somebody made by hand, and a service account.
    async fn fixture() -> (MemoryStore, String, String, String) {
        let store = MemoryStore::new(Kek::from_raw_bytes(&[7u8; 32]).unwrap());
        let dev = realm(&store, "dev").await;
        let hel = realm(&store, "hel").await;
        let bbsk = realm(&store, "bbsk").await;
        for n in 1..=6 {
            user(&store, &dev, n, n == 2).await;
        }
        store
            .create_user(UserRepresentation {
                id: "svc".into(),
                realm_id: dev.clone(),
                username: "service-account-pipeline".into(),
                service_account_client_id: Some("pipeline".into()),
                enabled: true,
                ..UserRepresentation::default()
            })
            .await
            .unwrap();
        store
            .insert_webauthn_credential(
                serde_json::from_value(serde_json::json!({
                    "id": "wa-5", "realmId": dev, "orgId": null, "userId": person(5),
                    "userHandle": "h", "credentialId": "c", "publicKeyCoseJson": "{}",
                    "coseAlg": -7, "signCount": 0, "uvInitialized": true,
                    "backupEligible": false, "backupState": false, "transports": [],
                    "attestationType": "none", "rpId": "login.city.example", "userLabel": null,
                    "createdAt": "2026-11-16T00:00:00Z", "lastUsedAt": null
                }))
                .unwrap(),
            )
            .await
            .unwrap();
        group(
            &store,
            &dev,
            "/hel_bound",
            true,
            &[person(1), person(2), person(5)],
        )
        .await;
        group(&store, &dev, "/bbsk", true, &[]).await;
        group(&store, &dev, "/bbsk/members", true, &[person(2), person(4)]).await;
        group(&store, &dev, "/hel-roads-readers", true, &[person(3)]).await;
        group(&store, &dev, "/bbsk_bound", false, &[person(6)]).await;
        group(&store, &dev, "/platform-admins", false, &[person(4)]).await;
        (store, dev, hel, bbsk)
    }

    fn orgs() -> Vec<String> {
        vec!["hel".into(), "bbsk".into()]
    }

    async fn run_mode(store: &MemoryStore, mode: Mode) -> Report {
        run(store, "dev", &orgs(), "platform-admins", mode)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn the_dry_run_counts_who_goes_where_and_changes_nothing() {
        let (store, _, hel, bbsk) = fixture().await;
        let report = run_mode(&store, Mode::DryRun).await;
        let hel_counts = &report.realms["hel"];
        assert_eq!(
            (
                hel_counts.placed,
                hel_counts.with_otp,
                hel_counts.with_passkey
            ),
            (3, 1, 1)
        );
        let bbsk_counts = &report.realms["bbsk"];
        assert_eq!((bbsk_counts.placed, bbsk_counts.with_otp), (2, 1));
        // 3 (project-only), 6 (a hand-made group); the service account is no person.
        assert_eq!(report.staying, 2);
        for realm in [&hel, &bbsk] {
            assert!(
                store
                    .list_users(realm, None, None, None, None, false, 0, 100)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[tokio::test]
    async fn apply_copies_hash_and_seed_once_and_prints_no_person() {
        let (store, dev, hel, bbsk) = fixture().await;
        let first = run_mode(&store, Mode::Apply).await;
        assert_eq!(first.realms["hel"].copied, 3);
        assert_eq!(first.realms["bbsk"].copied, 2);
        let second = run_mode(&store, Mode::Apply).await;
        assert_eq!(second.realms["hel"].copied, 0);
        assert_eq!(second.realms["hel"].present, 3);
        // Person 2 has an account in each realm, same id, same hash and seed bytes.
        for realm in [&hel, &bbsk] {
            let copy = store
                .get_user_by_id(realm, None, &person(2))
                .await
                .unwrap()
                .unwrap();
            assert!(copy.enabled && copy.org_id.is_none() && copy.groups.is_empty());
            let creds = store.list_credentials(realm, &person(2)).await.unwrap();
            let secret = |kind: &str| {
                creds
                    .iter()
                    .find(|c| c.credential_type == kind)
                    .map(|c| c.secret_data.clone())
            };
            assert_eq!(secret("password").as_deref(), Some(HASH));
            assert_eq!(secret("otp").as_deref(), Some(SEED));
            assert_eq!(creds.len(), 2);
        }
        assert!(
            store
                .get_user_by_id(&hel, None, &person(3))
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .get_user_by_id(&bbsk, None, &person(6))
                .await
                .unwrap()
                .is_none()
        );
        // The source stays enabled until --retire.
        assert!(
            store
                .get_user_by_id(&dev, None, &person(1))
                .await
                .unwrap()
                .unwrap()
                .enabled
        );
        let printed = format!("{first}{second}");
        assert!(
            !printed.contains('@') && !printed.contains("person") && !printed.contains("argon2")
        );
        assert!(!printed.contains("JBSW") && !printed.contains(&person(1)));
    }

    #[tokio::test]
    async fn a_copy_that_changed_its_password_is_left_as_it_is() {
        let (store, _, hel, _) = fixture().await;
        run_mode(&store, Mode::Apply).await;
        let mut changed = store
            .list_credentials(&hel, &person(1))
            .await
            .unwrap()
            .remove(0);
        changed.secret_data = "$argon2id$changed".into();
        store.upsert_credential(changed).await.unwrap();
        run_mode(&store, Mode::Apply).await;
        let creds = store.list_credentials(&hel, &person(1)).await.unwrap();
        assert!(creds.iter().any(|c| c.secret_data == "$argon2id$changed"));
        assert!(
            !creds
                .iter()
                .any(|c| c.credential_type == "password" && c.secret_data == HASH)
        );
    }

    #[tokio::test]
    async fn retire_waits_for_the_reconciler_and_keeps_operators_and_passkeys() {
        let (store, dev, hel, bbsk) = fixture().await;
        run_mode(&store, Mode::Apply).await;
        // No Organization realm holds its groups yet: nobody's copy carries jc_org, nobody retires.
        let early = run_mode(&store, Mode::Retire).await;
        assert_eq!((early.retired, early.kept), (0, 4));
        group(
            &store,
            &hel,
            "/hel_bound",
            true,
            &[person(1), person(2), person(5)],
        )
        .await;
        let half = run_mode(&store, Mode::Retire).await;
        // 1 retires; 2 waits for bbsk's groups; 4 is an operator; 5 holds a passkey.
        assert_eq!((half.retired, half.kept), (1, 3));
        group(&store, &bbsk, "/bbsk", true, &[]).await;
        group(
            &store,
            &bbsk,
            "/bbsk/members",
            true,
            &[person(2), person(4)],
        )
        .await;
        let rest = run_mode(&store, Mode::Retire).await;
        assert_eq!((rest.retired, rest.kept), (1, 2));
        let source = |n| {
            let store = &store;
            let dev = dev.clone();
            async move {
                store
                    .get_user_by_id(&dev, None, &person(n))
                    .await
                    .unwrap()
                    .unwrap()
            }
        };
        assert!(!source(1).await.enabled && !source(2).await.enabled);
        assert!(source(4).await.enabled && source(5).await.enabled && source(3).await.enabled);
        let restored = run_mode(&store, Mode::Restore).await;
        assert_eq!(restored.restored, 2);
        assert!(source(1).await.enabled && source(2).await.enabled);
        assert!(!source(1).await.attributes.contains_key(RETIRED_ATTRIBUTE));
        assert_eq!(run_mode(&store, Mode::Restore).await.restored, 0);
    }

    #[tokio::test]
    async fn refuses_a_missing_realm_and_a_source_named_as_a_target() {
        let (store, _, _, _) = fixture().await;
        let missing = run(
            &store,
            "dev",
            &["praha".into()],
            "platform-admins",
            Mode::DryRun,
        )
        .await
        .unwrap_err();
        assert!(missing.contains("realm 'praha'"), "{missing}");
        let same = run(&store, "hel", &orgs(), "platform-admins", Mode::Apply)
            .await
            .unwrap_err();
        assert!(same.contains("both the source"), "{same}");
    }
}
