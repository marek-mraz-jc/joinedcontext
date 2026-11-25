#![forbid(unsafe_code)]
//! `shako-git`: in-process Git smart HTTP engine on `gix` (upload-pack v0/v2, receive-pack
//! with quarantine and thin-pack completion, send-pack client for push mirrors, bundle v2
//! writer and restore, archive streaming, in-memory 3-way tree merge, pack compaction, and fsck).

pub mod archive;
pub mod bundle;
pub mod compact;
pub mod merge;
pub mod pack;
pub mod pktline;
pub mod receive_pack;
pub mod repo;
pub mod send_pack;
pub mod upload_pack;

pub use archive::{ArchiveFormat, create_archive};
pub use bundle::{create_full_backup_bundle, create_single_ref_bundle, restore_from_bundle};
pub use compact::{
    AUTO_COMPACT_PACK_THRESHOLD, CompactOutcome, DEFAULT_PRUNE_GRACE, FsckOutcome,
    compact_repository, compaction_due, fsck_repository, has_active_quarantine,
};
pub use merge::{
    DiffFileEntry, MergeOptions, MergeStyle, MergeabilityOutcome, check_mergeability,
    diff_refs_files, execute_pr_merge, update_pr_branch,
};
pub use pack::{Admit, job_bytes, write_pack};
pub use pktline::{PktLine, parse_all_pkt_lines, read_pkt_line, write_flush, write_text_line};
pub use receive_pack::{
    MAX_COMMAND_BYTES, PushGuard, RECEIVE_PACK_CAPABILITIES, ReceivePackOutcome, RefUpdateCommand,
    advertise_receive_pack, handle_receive_pack, scan_blob_for_secrets,
};
pub use repo::{
    CommitAuthor, CommitInfo, FileChangeOp, FileOpKind, RepoLocks, SAFE_BARE_REPO_CONFIG,
    TreeEntryInfo, commit_file_changes, compute_repo_size_bytes, copy_bare_repo, create_branch,
    delete_branch, init_bare_repo, list_branches, list_commits, list_tags, list_tree_entries,
    open_isolated, read_commit_info, read_file_at_ref, resolve_commit_oid, set_ref_direct,
};
pub use send_pack::{MirrorRemote, SendPack, write_send_pack};
pub use upload_pack::{
    AdvertisedRef, ObjectFilter, V0_UPLOAD_CAPABILITIES, advertise_upload_pack,
    collect_advertised_refs, handle_upload_pack, read_negotiation,
};

/// Maps a failed gix call to `ShakoError::Git` with `what` naming the step: one error arm for
/// every call site, so the arm is the same code wherever a repository read fails.
pub(crate) fn git_err<E: std::fmt::Display>(
    what: &'static str,
) -> impl FnOnce(E) -> shako_core::ShakoError {
    move |e| shako_core::ShakoError::Git(format!("{what}: {e}"))
}

/// Maps a failed file system call to `ShakoError::Storage` with `what` naming the step.
pub(crate) fn storage_err<E: std::fmt::Display>(
    what: &'static str,
) -> impl FnOnce(E) -> shako_core::ShakoError {
    move |e| shako_core::ShakoError::Storage(format!("{what}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use shako_core::{
        Action, BranchProtectionRule, InstanceId, LocalAuthorizer, OrgId, Principal, RefPolicy,
        RepoId, RequestCtx, RoleBinding, Subject,
    };
    use std::time::{Duration, SystemTime};

    /// Pushes `main` of `source` into a new empty repository and answers the report-status text.
    fn push_main(tmp: &std::path::Path, source: &std::path::Path, tip: &str) -> String {
        push_refs(tmp, source, &[(tip, "refs/heads/main")]).0
    }

    /// Pushes the objects of `source`'s `main` with one ref creation per `(tip, refname)` in one
    /// non-atomic receive-pack; answers the report-status text and the new repository's path.
    fn push_refs(
        tmp: &std::path::Path,
        source: &std::path::Path,
        updates: &[(&str, &str)],
    ) -> (String, std::path::PathBuf) {
        let inst = InstanceId::from_u128(1);
        let dest = init_bare_repo(tmp, inst, RepoId::from_u128(201), "main", false, "Dest")
            .expect("init dest");
        (receive_refs(&dest, source, updates), dest)
    }

    /// One receive-pack of `source`'s `main` into `dest`; answers the report-status text.
    fn receive_refs(
        dest: &std::path::Path,
        source: &std::path::Path,
        updates: &[(&str, &str)],
    ) -> String {
        let inst = InstanceId::from_u128(1);
        let org = OrgId::from_u128(1);
        let mut bundle = Vec::new();
        create_single_ref_bundle(source, "main", &mut bundle, &mut crate::testutil::admit_all)
            .expect("bundle");
        let pack = bundle
            .windows(2)
            .position(|w| w == b"\n\n")
            .expect("header end")
            + 2;
        let mut body = Vec::new();
        for (i, (tip, refname)) in updates.iter().enumerate() {
            let caps = if i == 0 { "\0 report-status" } else { "" };
            write_text_line(
                &mut body,
                &format!("0000000000000000000000000000000000000000 {tip} {refname}{caps}"),
            )
            .unwrap();
        }
        write_flush(&mut body).unwrap();
        body.extend_from_slice(&bundle[pack..]);
        let authz = LocalAuthorizer::new(vec![RoleBinding {
            org_id: org,
            principal: Principal::User("jc-portal".into()),
            actions: vec![Action::Write],
            project_id: None,
            repo: None,
        }]);
        let ctx = RequestCtx::new(
            inst,
            org,
            Subject::authenticated("jc-portal", "jc-portal", vec![org]),
        );
        let guard = PushGuard {
            ctx: &ctx,
            authorizer: &authz,
            owner: "helsinki",
            repo_name: "dest",
            policy: &RefPolicy::default(),
        };
        let body = spooled(&body);
        let out = handle_receive_pack(
            dest,
            &guard,
            10 * 1024 * 1024,
            &body,
            &mut crate::testutil::admit_all,
        )
        .expect("receive-pack");
        String::from_utf8_lossy(&out.response_bytes).into_owned()
    }

    fn spooled(bytes: &[u8]) -> std::fs::File {
        let mut file = tempfile::tempfile().expect("spool");
        std::io::Write::write_all(&mut file, bytes).expect("spool write");
        file
    }

    /// Contract S4: a pack is named by its content, so a refused repeat of an accepted push
    /// carries the stored pack's name. Refusing it keeps what the accepted push stored.
    #[test]
    fn the_refusing_writer_refuses_a_flush_too() {
        use std::io::Write as _;
        let err = testutil::Refusing.flush().expect_err("disk full");
        assert_eq!(err.to_string(), "disk full");
    }

    #[test]
    fn a_refused_repeat_of_a_push_keeps_the_accepted_objects() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let source = init_bare_repo(
            tmp.path(),
            InstanceId::from_u128(1),
            RepoId::from_u128(200),
            "main",
            true,
            "Source",
        )
        .expect("init source");
        let tip = resolve_commit_oid(&open_isolated(&source).expect("open"), "main")
            .expect("tip")
            .to_string();
        let updates = [(tip.as_str(), "refs/heads/main")];
        let (accepted, dest) = push_refs(tmp.path(), &source, &updates);
        assert!(accepted.contains("ok refs/heads/main"), "{accepted}");
        let repeated = receive_refs(&dest, &source, &updates);
        assert!(repeated.contains("ng refs/heads/main"), "{repeated}");
        let (_, files) = list_tree_entries(&dest, "main", true).expect("main still reads");
        assert!(files.iter().any(|f| f.path == "README.md"));
    }

    /// SEC-06, P-19 (T-6713): a credential committed and removed again inside one push is still
    /// refused, since it would stay in history; the refusal names the file, the line and the rule,
    /// never the value. The scan has no off switch.
    #[test]
    fn a_secret_anywhere_in_a_pushed_history_is_refused() {
        let author = CommitAuthor {
            name: "Alice".into(),
            email: "alice@hel.fi".into(),
        };
        // Built at run time so no scanner meets a literal credential in this file.
        let literal = format!("pw{}", "4feed".repeat(3));
        let token = format!("ghp_{}", "a1B2c3D4e5".repeat(4));
        let cases = [
            (
                "spaces/air/source.yaml",
                format!(
                    "apiVersion: joinedcontext.com/v1\nkind: DataSource\nspec:\n  http:\n    password: {literal}\n"
                ),
                "literal-secret-field in 'spaces/air/source.yaml' line 5 (field 'password'",
            ),
            (
                "app/src/config.ts",
                format!("export const key = \"{token}\";\n"),
                "github-token in 'app/src/config.ts' line 1",
            ),
        ];
        for (path, content, rule) in cases {
            let tmp = tempfile::tempdir().expect("tempdir");
            let source = init_bare_repo(
                tmp.path(),
                InstanceId::from_u128(1),
                RepoId::from_u128(200),
                "main",
                true,
                "Source",
            )
            .expect("init source");
            let commit = |message: &str, operation, content: Option<&str>, sha: Option<String>| {
                commit_file_changes(
                    &source,
                    "main",
                    message,
                    &author,
                    &author,
                    &[FileChangeOp {
                        operation,
                        path: path.into(),
                        content: content.map(|c| c.as_bytes().to_vec()),
                        sha,
                    }],
                )
                .expect("commit")
            };
            commit("add the feed", FileOpKind::Create, Some(&content), None);
            let (_, entries) = list_tree_entries(&source, "main", true).expect("tree");
            let blob = entries
                .into_iter()
                .find(|e| e.path == path)
                .expect("the file")
                .sha;
            let tip = commit("take it out again", FileOpKind::Delete, None, Some(blob));
            let said = push_main(tmp.path(), &source, &tip);
            assert!(said.contains("ng refs/heads/main"), "{said}");
            assert!(said.contains(rule), "{said}");
            assert!(!said.contains(&literal) && !said.contains(&token), "{said}");
        }
    }

    /// P-6-03: the scan skips only subtrees a parent holds unchanged, so a credential added beside
    /// a clean file deep in a directory the parent already has, then removed, is still refused.
    #[test]
    fn a_secret_beside_an_inherited_file_in_a_held_directory_is_refused() {
        let author = CommitAuthor {
            name: "Alice".into(),
            email: "alice@hel.fi".into(),
        };
        let token = format!("ghp_{}", "a1B2c3D4e5".repeat(4));
        let tmp = tempfile::tempdir().expect("tempdir");
        let source = init_bare_repo(
            tmp.path(),
            InstanceId::from_u128(1),
            RepoId::from_u128(200),
            "main",
            true,
            "Source",
        )
        .expect("init source");
        let change = |message: &str, operation, path: &str, content: Option<String>, sha| {
            commit_file_changes(
                &source,
                "main",
                message,
                &author,
                &author,
                &[FileChangeOp {
                    operation,
                    path: path.into(),
                    content: content.map(String::into_bytes),
                    sha,
                }],
            )
            .expect("commit")
        };
        change(
            "clean",
            FileOpKind::Create,
            "app/src/a.ts",
            Some("ok\n".into()),
            None,
        );
        change(
            "other",
            FileOpKind::Create,
            "docs/b.md",
            Some("ok\n".into()),
            None,
        );
        let path = "app/src/config.ts";
        change(
            "leak",
            FileOpKind::Create,
            path,
            Some(format!("const k = \"{token}\";\n")),
            None,
        );
        let (_, entries) = list_tree_entries(&source, "main", true).expect("tree");
        let blob = entries
            .into_iter()
            .find(|e| e.path == path)
            .expect("file")
            .sha;
        let tip = change("take it out", FileOpKind::Delete, path, None, Some(blob));
        let said = push_main(tmp.path(), &source, &tip);
        assert!(said.contains("ng refs/heads/main"), "{said}");
        assert!(
            said.contains("github-token in 'app/src/config.ts' line 1"),
            "{said}"
        );
    }

    /// Contract S4: a push whose one ref carries a credential is refused whole, so the pack holding
    /// the credential is not kept beside the refs the push would have created.
    #[test]
    fn a_secret_in_one_ref_of_a_push_refuses_every_ref_and_keeps_no_pack() {
        let author = CommitAuthor {
            name: "Alice".into(),
            email: "alice@hel.fi".into(),
        };
        let token = format!("ghp_{}", "a1B2c3D4e5".repeat(4));
        let tmp = tempfile::tempdir().expect("tempdir");
        let source = init_bare_repo(
            tmp.path(),
            InstanceId::from_u128(1),
            RepoId::from_u128(200),
            "main",
            true,
            "Source",
        )
        .expect("init source");
        let commit = |message: &str, path: &str, content: String| {
            commit_file_changes(
                &source,
                "main",
                message,
                &author,
                &author,
                &[FileChangeOp {
                    operation: FileOpKind::Create,
                    path: path.into(),
                    content: Some(content.into_bytes()),
                    sha: None,
                }],
            )
            .expect("commit")
        };
        let clean = commit("readme", "notes.md", "plain text\n".into());
        let leaked = commit(
            "config",
            "src/config.ts",
            format!("const k = \"{token}\";\n"),
        );
        let (said, dest) = push_refs(
            tmp.path(),
            &source,
            &[(&clean, "refs/heads/ok"), (&leaked, "refs/heads/main")],
        );
        assert!(said.contains("ng refs/heads/main"), "{said}");
        assert!(said.contains("ng refs/heads/ok"), "{said}");
        let packs = std::fs::read_dir(dest.join("objects/pack"))
            .map(|d| d.flatten().count())
            .unwrap_or(0);
        assert_eq!(packs, 0, "no pack of a refused push stays");
    }

    #[test]
    fn a_secret_reference_in_a_manifest_passes_the_scan() {
        for open in [
            "    password: ${FEED_PW}",
            "    password:",
            "    password: null",
            "  token:\n    secretRef: feed",
            "    apiTokenRef: opendata",
            "    description: \"the password lives in the secret form\"",
            // A Bloblang mapping inside a block scalar (deployment seeds
            // bbsk-pipeline-ukazovatele.yaml:83, slovensko-pipeline-participacia-statistika.yaml:37).
            "spec:\n  mapping: |\n    root = this.map_each(w -> {\n      \"token\": $territory.get(w.area),\n    })\n",
            "spec:\n  mapping: |-\n    root = {\n      \"token\": e.proposalState.value.or(\"\")\n    }\n",
            // A sops-encrypted value is ciphertext, no literal.
            "password: ENC[AES256_GCM,data:Zm9v,iv:YmFy,tag:YmF6,type:str]",
            // Not YAML at all: the field rule judges parsed manifests only.
            "password: [unclosed",
            // Helm values naming a Kubernetes Secret, and a workflow's secrets expression.
            "apiToken:\n  secret: ckan-api-token\n",
            "kind: A\napiVersion: v1\nwith: { password: \"${{ secrets.GITHUB_TOKEN }}\" }",
        ] {
            assert_eq!(
                scan_blob_for_secrets("a/b.yaml", open.as_bytes()),
                None,
                "{open}"
            );
        }
        let literal = format!(
            "apiVersion: v1\nkind: A\nclientSecret: \"{}\"",
            "s3cr3t".repeat(2)
        );
        assert!(scan_blob_for_secrets("a/b.yml", literal.as_bytes()).is_some());
        // The same literal in the second document, nested under a list item, names its line.
        let nested = format!(
            "kind: A\n---\napiVersion: v1\nkind: A\nspec:\n  users:\n    - name: x\n      password: {}\n",
            "s3cr3t".repeat(2)
        );
        let said = scan_blob_for_secrets("a/b.yaml", nested.as_bytes()).expect("refused");
        assert!(
            said.contains("line 8") && said.contains("'password'"),
            "{said}"
        );
        assert_eq!(scan_blob_for_secrets("a/b.md", literal.as_bytes()), None);
    }

    #[test]
    fn git_engine_commit_merge_bundle_archive_receive_pack_and_compaction() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let inst = InstanceId::from_u128(1);
        let org = OrgId::from_u128(1);
        let repo_id = RepoId::from_u128(100);

        // 1. Initialize bare repo with auto_init
        let repo_path = init_bare_repo(tmp.path(), inst, repo_id, "main", true, "Test Repo")
            .expect("init bare");

        // Verify T2:even if someone injects core.hooksPath or core.fsmonitor into config,
        // isolated open + our operations never execute any external command
        let author = CommitAuthor {
            name: "Alice".into(),
            email: "alice@hel.fi".into(),
        };
        let sha1 = commit_file_changes(
            &repo_path,
            "main",
            "Add space.yaml",
            &author,
            &author,
            &[FileChangeOp {
                operation: FileOpKind::Create,
                path: "spaces/air/space.yaml".into(),
                content: Some(b"kind: ContextSpace\n".to_vec()),
                sha: None,
            }],
        )
        .expect("commit 1");
        assert_eq!(sha1.len(), 40);

        // 2. Create feature branch, commit a second file, test mergeability and squash merge
        create_branch(&repo_path, "feature/b1", "main").expect("create branch");
        let sha2 = commit_file_changes(
            &repo_path,
            "feature/b1",
            "Add endpoint.yaml",
            &author,
            &author,
            &[FileChangeOp {
                operation: FileOpKind::Create,
                path: "spaces/air/endpoint.yaml".into(),
                content: Some(b"kind: Endpoint\n".to_vec()),
                sha: None,
            }],
        )
        .expect("commit 2");

        let mergeability =
            check_mergeability(&repo_path, "main", "feature/b1").expect("mergeability");
        assert!(mergeability.mergeable);
        assert!(!mergeability.is_behind_base);
        assert!(mergeability.conflicting_paths.is_empty());

        let diff_files = diff_refs_files(&repo_path, "main", "feature/b1").expect("diff");
        assert_eq!(diff_files.len(), 1);
        assert_eq!(diff_files[0].filename, "spaces/air/endpoint.yaml");
        assert_eq!(diff_files[0].status, "added");

        // Moved head check on merge -> 409 Conflict (Threat T10)
        let moved_err = execute_pr_merge(
            &repo_path,
            "main",
            "feature/b1",
            &MergeOptions {
                expected_head_sha: Some("0000000000000000000000000000000000000000"),
                style: MergeStyle::Squash,
                message: "Squash PR #1",
                author: &author,
                committer: &author,
            },
        )
        .expect_err("moved head must fail");
        assert_eq!(moved_err.status_code(), 409);

        let merged_sha = execute_pr_merge(
            &repo_path,
            "main",
            "feature/b1",
            &MergeOptions {
                expected_head_sha: Some(&sha2),
                style: MergeStyle::Squash,
                message: "Squash PR #1",
                author: &author,
                committer: &author,
            },
        )
        .expect("squash merge");
        assert_eq!(merged_sha.len(), 40);

        // 3. Test upload-pack v0 and v2 advertisements and v2 ls-refs + fetch
        let adv_v0 = advertise_upload_pack(&repo_path, None).expect("adv v0");
        let adv_v0_str = String::from_utf8_lossy(&adv_v0);
        assert!(adv_v0_str.contains("# service=git-upload-pack"));
        assert!(adv_v0_str.contains("refs/heads/main"));

        let adv_v2 = advertise_upload_pack(&repo_path, Some("version=2")).expect("adv v2");
        assert!(String::from_utf8_lossy(&adv_v2).contains("version 2"));

        let mut v2_fetch_req = Vec::new();
        write_text_line(&mut v2_fetch_req, "command=fetch").unwrap();
        pktline::write_delim(&mut v2_fetch_req).unwrap();
        write_text_line(&mut v2_fetch_req, &format!("want {merged_sha}")).unwrap();
        write_text_line(&mut v2_fetch_req, "done").unwrap();
        write_flush(&mut v2_fetch_req).unwrap();
        let mut v2_fetch_resp = Vec::new();
        handle_upload_pack(
            &repo_path,
            Some("version=2"),
            &v2_fetch_req,
            &mut v2_fetch_resp,
            &mut crate::testutil::admit_all,
        )
        .expect("v2 fetch");
        assert!(String::from_utf8_lossy(&v2_fetch_resp).contains("packfile"));

        // 4. Test bundle export and push_bundle into an empty repository via receive-pack
        let mut bundle = Vec::new();
        create_single_ref_bundle(
            &repo_path,
            "main",
            &mut bundle,
            &mut crate::testutil::admit_all,
        )
        .expect("bundle");
        assert!(bundle.starts_with(b"# v2 git bundle\n"));

        let dest_id = RepoId::from_u128(101);
        let dest_path =
            init_bare_repo(tmp.path(), inst, dest_id, "main", false, "Dest").expect("init dest");
        let pack_offset = bundle
            .windows(2)
            .position(|w| w == b"\n\n")
            .expect("header end")
            + 2;
        let pack_slice = &bundle[pack_offset..];

        let mut rp_body = Vec::new();
        write_text_line(
            &mut rp_body,
            &format!(
                "0000000000000000000000000000000000000000 {merged_sha} refs/heads/main\0 report-status"
            ),
        )
        .unwrap();
        write_flush(&mut rp_body).unwrap();
        rp_body.extend_from_slice(pack_slice);

        let authz = LocalAuthorizer::new(vec![RoleBinding {
            org_id: org,
            principal: Principal::User("jc-portal".into()),
            actions: vec![Action::Write],
            project_id: None,
            repo: None,
        }]);
        let ctx = RequestCtx::new(
            inst,
            org,
            Subject::authenticated("jc-portal", "jc-portal", vec![org]),
        );
        let rp_out = handle_receive_pack(
            &dest_path,
            &PushGuard {
                ctx: &ctx,
                authorizer: &authz,
                owner: "helsinki",
                repo_name: "dest",
                policy: &RefPolicy::default(),
            },
            10 * 1024 * 1024,
            &spooled(&rp_body),
            &mut crate::testutil::admit_all,
        )
        .expect("receive-pack");
        assert_eq!(rp_out.updated_refs.len(), 1);
        assert!(String::from_utf8_lossy(&rp_out.response_bytes).contains("unpack ok"));
        assert!(String::from_utf8_lossy(&rp_out.response_bytes).contains("ok refs/heads/main"));

        // Verify quarantine is clean and quota refusal leaves no quarantine behind
        assert!(!has_active_quarantine(&dest_path));
        let over_quota = handle_receive_pack(
            &dest_path,
            &PushGuard {
                ctx: &ctx,
                authorizer: &authz,
                owner: "helsinki",
                repo_name: "dest",
                policy: &RefPolicy {
                    branch_rules: vec![BranchProtectionRule::default()],
                    tag_rules: vec![],
                },
            },
            10,
            &spooled(&rp_body),
            &mut testutil::admit_all,
        ) // 10 bytes max_push_bytes -> refused with unpack error quota
        .expect("quota refusal");
        assert!(String::from_utf8_lossy(&over_quota.response_bytes).contains("unpack error quota"));
        assert!(!has_active_quarantine(&dest_path));

        // 5. Archives (.zip and .tar.gz), Compaction, and Fsck
        let archived = |format| {
            let mut out = std::io::Cursor::new(Vec::new());
            create_archive(
                &dest_path,
                "dest",
                "main",
                format,
                &mut out,
                &mut crate::testutil::admit_all,
            )
            .map(|()| out.into_inner())
        };
        let zip_bytes = archived(ArchiveFormat::Zip).expect("zip");
        assert!(zip_bytes.starts_with(b"PK\x03\x04"));
        let targz_bytes = archived(ArchiveFormat::TarGz).expect("targz");
        assert!(!targz_bytes.is_empty());

        // A push that runs holds compaction back; what a crash left a day ago does not.
        let incoming = repo_path.join("objects/incoming-left");
        std::fs::create_dir_all(&incoming).expect("quarantine");
        let compact = || {
            compact_repository(
                &repo_path,
                Duration::from_secs(3600),
                &mut crate::testutil::admit_all,
            )
        };
        assert!(compact().expect("compact").skipped_due_to_quarantine);
        std::fs::File::open(&incoming)
            .and_then(|dir| dir.set_modified(SystemTime::now() - Duration::from_secs(25 * 3600)))
            .expect("age");
        assert!(!compact().expect("compact").skipped_due_to_quarantine);
        assert!(!incoming.exists());
        let compact_out = compact_repository(
            &repo_path,
            Duration::from_secs(3600),
            &mut crate::testutil::admit_all,
        )
        .expect("compact");
        assert_eq!(compact_out.packs_after, 1);

        let fsck_out = fsck_repository(&repo_path).expect("fsck");
        assert_eq!(fsck_out.verified_packs, 1);
        assert!(fsck_out.verified_refs >= 2);

        // 6. A want for a blob no ref reaches is refused, even though the object is on disk
        let orphan = b"dropped secret\n";
        let orphan_oid =
            gix_object::compute_hash(gix_hash::Kind::Sha1, gix_object::Kind::Blob, orphan)
                .expect("hash");
        repo::write_objects_as_pack(
            &repo_path,
            &[(orphan_oid, gix_object::Kind::Blob, orphan.to_vec())],
        )
        .expect("write orphan");
        let mut orphan_req = Vec::new();
        write_text_line(&mut orphan_req, "command=fetch").unwrap();
        pktline::write_delim(&mut orphan_req).unwrap();
        write_text_line(&mut orphan_req, &format!("want {orphan_oid}")).unwrap();
        write_text_line(&mut orphan_req, "done").unwrap();
        write_flush(&mut orphan_req).unwrap();
        let refused = handle_upload_pack(
            &repo_path,
            Some("version=2"),
            &orphan_req,
            std::io::sink(),
            &mut testutil::admit_all,
        )
        .expect_err("unreachable blob must not be served");
        assert_eq!(refused.status_code(), 400);
    }
}

/// Fixtures the in-file test modules share: small repositories, commits, tags and pkt-line
/// bodies.
#[cfg(test)]
pub(crate) mod testutil {
    use crate::pktline::{write_delim, write_flush, write_text_line};
    use crate::receive_pack::{PushGuard, ReceivePackOutcome, handle_receive_pack};
    use crate::repo::{
        CommitAuthor, FileChangeOp, FileOpKind, commit_file_changes, init_bare_repo, open_isolated,
        read_file_at_ref, set_ref_direct,
    };
    use gix_hash::ObjectId;
    use gix_object::tree::EntryKind;
    use shako_core::{
        Action, InstanceId, LocalAuthorizer, OrgId, Principal, RefPolicy, RepoId, RequestCtx,
        RoleBinding, ShakoResult, Subject,
    };
    use std::path::{Path, PathBuf};

    /// A pkt-line body marker for a flush packet (`0000`).
    pub(crate) const FLUSH: &str = "<flush>";
    /// A pkt-line body marker for a delimiter packet (`0001`).
    pub(crate) const DELIM: &str = "<delim>";

    /// Admits every job: one function, so a test whose job is refused before admission leaves
    /// no admission closure of its own unrun.
    pub(crate) fn admit_all(_: u64) -> ShakoResult<()> {
        Ok(())
    }

    pub(crate) fn author() -> CommitAuthor {
        CommitAuthor {
            name: "Alice".into(),
            email: "alice@hel.fi".into(),
        }
    }

    /// A new bare repository with `main` as its default branch, `{dir}/1/{id}.git`.
    pub(crate) fn repo(dir: &Path, id: u128, auto_init: bool) -> PathBuf {
        init_bare_repo(
            dir,
            InstanceId::from_u128(1),
            RepoId::from_u128(id),
            "main",
            auto_init,
            "Fixture",
        )
        .expect("init")
    }

    pub(crate) fn oid(hex: &str) -> ObjectId {
        ObjectId::from_hex(hex.as_bytes()).expect("an object id")
    }

    /// Writes `content` at `path` on `branch` (created or overwritten) and answers the commit.
    pub(crate) fn put(repo: &Path, branch: &str, path: &str, content: &[u8]) -> ObjectId {
        change(repo, branch, FileOpKind::Upload, path, Some(content), None)
    }

    /// Deletes `path` from `branch` and answers the commit.
    pub(crate) fn remove(repo: &Path, branch: &str, path: &str) -> ObjectId {
        let (sha, _) = read_file_at_ref(repo, branch, path, &mut admit_all)
            .expect("read")
            .expect("the file exists");
        change(repo, branch, FileOpKind::Delete, path, None, Some(sha))
    }

    fn change(
        repo: &Path,
        branch: &str,
        operation: FileOpKind,
        path: &str,
        content: Option<&[u8]>,
        sha: Option<String>,
    ) -> ObjectId {
        let sha = commit_file_changes(
            repo,
            branch,
            path,
            &author(),
            &author(),
            &[FileChangeOp {
                operation,
                path: path.into(),
                content: content.map(<[u8]>::to_vec),
                sha,
            }],
        )
        .expect("commit");
        oid(&sha)
    }

    /// Writes an annotated tag object naming `target` and points `refs/tags/{name}` at it.
    pub(crate) fn annotated_tag(
        repo: &Path,
        name: &str,
        target: ObjectId,
        kind: gix_object::Kind,
    ) -> ObjectId {
        let handle = open_isolated(repo).expect("open");
        let tag = gix_object::Tag {
            target,
            target_kind: kind,
            name: name.into(),
            tagger: Some(gix::actor::Signature {
                name: "t".into(),
                email: "t@example.org".into(),
                time: gix::date::Time::new(1_700_000_000, 0),
            }),
            message: "release\n".into(),
            signature: None,
        };
        let id = handle.write_object(&tag).expect("write tag").detach();
        set_ref_direct(repo, &format!("refs/tags/{name}"), &id.to_string()).expect("tag ref");
        id
    }

    /// A pkt-line body: each text as one data line, [`FLUSH`] and [`DELIM`] as their packets.
    pub(crate) fn pkt(lines: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for line in lines {
            match *line {
                FLUSH => write_flush(&mut out),
                DELIM => write_delim(&mut out),
                text => write_text_line(&mut out, text),
            }
            .expect("pkt-line");
        }
        out
    }

    pub(crate) fn text(bytes: &[u8]) -> String {
        String::from_utf8_lossy(bytes).into_owned()
    }

    /// The object count of the first pack in `bytes`, or `None` when it holds none.
    pub(crate) fn pack_count(bytes: &[u8]) -> Option<u32> {
        let at = bytes.windows(4).position(|w| w == b"PACK")?;
        let count = bytes.get(at + 8..at + 12)?;
        Some(u32::from_be_bytes(count.try_into().ok()?))
    }

    /// The raw pack of every object `tip` of `source` reaches.
    pub(crate) fn pack_of(source: &Path, tip: &str) -> Vec<u8> {
        let mut bundle = Vec::new();
        crate::create_single_ref_bundle(source, tip, &mut bundle, &mut admit_all).expect("bundle");
        let at = bundle
            .windows(2)
            .position(|w| w == b"\n\n")
            .expect("header end");
        bundle.split_off(at + 2)
    }

    pub(crate) fn spooled(bytes: &[u8]) -> std::fs::File {
        let mut file = tempfile::tempfile().expect("spool");
        std::io::Write::write_all(&mut file, bytes).expect("spool write");
        file
    }

    /// One receive-pack of `body` into `dest` by a writer of the organization, under `policy`.
    pub(crate) fn receive(
        dest: &Path,
        policy: &RefPolicy,
        body: &[u8],
    ) -> ShakoResult<ReceivePackOutcome> {
        let org = OrgId::from_u128(1);
        let authz = LocalAuthorizer::new(vec![RoleBinding {
            org_id: org,
            principal: Principal::User("jc-portal".into()),
            actions: vec![Action::Write],
            project_id: None,
            repo: None,
        }]);
        let ctx = RequestCtx::new(
            InstanceId::from_u128(1),
            org,
            Subject::authenticated("jc-portal", "jc-portal", vec![org]),
        );
        let guard = PushGuard {
            ctx: &ctx,
            authorizer: &authz,
            owner: "helsinki",
            repo_name: "dest",
            policy,
        };
        handle_receive_pack(
            dest,
            &guard,
            10 * 1024 * 1024,
            &spooled(body),
            &mut admit_all,
        )
    }

    /// An object id no repository holds.
    pub(crate) const GONE: &str = "1111111111111111111111111111111111111111";

    /// Writes `bytes` as a loose object of `kind`, unchecked: a malformed tree or commit stays one.
    pub(crate) fn raw(repo: &Path, kind: gix_object::Kind, bytes: &[u8]) -> ObjectId {
        let handle = open_isolated(repo).expect("open");
        gix_object::Write::write_buf(&handle, kind, bytes).expect("write object")
    }

    /// Writes a tree of `(mode, name, id)` entries; no id is checked.
    pub(crate) fn tree(repo: &Path, entries: &[(EntryKind, &str, ObjectId)]) -> ObjectId {
        let mut tree = gix_object::Tree {
            entries: entries
                .iter()
                .map(|&(kind, name, oid)| gix_object::tree::Entry {
                    mode: kind.into(),
                    filename: name.into(),
                    oid,
                })
                .collect(),
        };
        tree.entries.sort();
        let handle = open_isolated(repo).expect("open");
        handle.write_object(&tree).expect("write tree").detach()
    }

    /// Writes a commit of `tree` on `parents`; no id is checked.
    pub(crate) fn commit(repo: &Path, tree: ObjectId, parents: &[ObjectId]) -> ObjectId {
        let sig = gix::actor::Signature {
            name: "t".into(),
            email: "t@example.org".into(),
            time: gix::date::Time::new(1_700_000_000, 0),
        };
        let commit = gix_object::Commit {
            tree,
            parents: parents.iter().copied().collect(),
            author: sig.clone(),
            committer: sig,
            encoding: None,
            message: "crafted\n".into(),
            extra_headers: Vec::new(),
        };
        let handle = open_isolated(repo).expect("open");
        handle.write_object(&commit).expect("write commit").detach()
    }

    /// Writes the loose ref file `refname` holding `content` (an id or `ref: <target>`), so no
    /// check of the target runs.
    pub(crate) fn point(repo: &Path, refname: &str, content: &str) {
        let file = repo.join(refname);
        std::fs::create_dir_all(file.parent().expect("ref dir")).expect("mkdir");
        std::fs::write(file, format!("{content}\n")).expect("ref");
    }

    /// Holds `refname`'s lock file as a concurrent writer would.
    pub(crate) fn lock(repo: &Path, refname: &str) {
        let file = repo.join(format!("{refname}.lock"));
        std::fs::create_dir_all(file.parent().expect("ref dir")).expect("mkdir");
        std::fs::write(file, "").expect("lock");
    }

    /// Replaces the loose object `id` with bytes that do not inflate.
    pub(crate) fn corrupt(repo: &Path, id: ObjectId) {
        let hex = id.to_string();
        let file = repo.join("objects").join(&hex[..2]).join(&hex[2..]);
        std::fs::remove_file(&file).expect("loose object");
        std::fs::write(&file, b"not zlib").expect("garbage");
    }

    /// The raw pack of exactly `ids` from `repo`, so a crafted graph travels as it is.
    pub(crate) fn pack_of_ids(repo: &Path, ids: &[ObjectId]) -> Vec<u8> {
        let handle = open_isolated(repo).expect("open");
        let mut out = Vec::new();
        crate::write_pack(&handle, ids, &mut out, &mut admit_all).expect("pack");
        out
    }

    /// A writer that refuses every byte, for the error arm of each pkt-line and pack write.
    pub(crate) struct Refusing;

    impl std::io::Write for Refusing {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("disk full"))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::other("disk full"))
        }
    }
}
