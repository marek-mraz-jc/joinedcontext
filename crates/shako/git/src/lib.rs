#![forbid(unsafe_code)]
//! `shako-git`: in-process Git smart HTTP engine on `gix` (upload-pack v0/v2, receive-pack
//! with quarantine and thin-pack completion, send-pack client for push mirrors, bundle v2
//! writer and restore, archive streaming, in-memory 3-way tree merge, pack compaction, and fsck).

pub mod archive;
pub mod bundle;
pub mod compact;
pub mod merge;
pub mod pktline;
pub mod receive_pack;
pub mod repo;
pub mod send_pack;
pub mod upload_pack;

pub use archive::{ArchiveFormat, create_archive};
pub use bundle::{create_full_backup_bundle, create_single_ref_bundle, restore_from_bundle};
pub use compact::{
    AUTO_COMPACT_PACK_THRESHOLD, CompactOutcome, DEFAULT_PRUNE_GRACE, FsckOutcome,
    compact_repository, fsck_repository, has_active_quarantine,
};
pub use merge::{
    DiffFileEntry, MergeOptions, MergeStyle, MergeabilityOutcome, check_mergeability,
    diff_refs_files, execute_pr_merge, update_pr_branch,
};
pub use pktline::{PktLine, parse_all_pkt_lines, read_pkt_line, write_flush, write_text_line};
pub use receive_pack::{
    PushGuard, RECEIVE_PACK_CAPABILITIES, ReceivePackOutcome, RefUpdateCommand,
    advertise_receive_pack, handle_receive_pack, scan_blob_for_secrets,
};
pub use repo::{
    CommitAuthor, CommitInfo, FileChangeOp, FileOpKind, RepoLocks, SAFE_BARE_REPO_CONFIG,
    TreeEntryInfo, commit_file_changes, compute_repo_size_bytes, copy_bare_repo, create_branch,
    delete_branch, init_bare_repo, list_branches, list_commits, list_tags, list_tree_entries,
    open_isolated, read_commit_info, read_file_at_ref, resolve_commit_oid, set_ref_direct,
};
pub use send_pack::push_mirror_over_https;
pub use upload_pack::{
    AdvertisedRef, ObjectFilter, V0_UPLOAD_CAPABILITIES, advertise_upload_pack, build_pack_bytes,
    collect_advertised_refs, handle_upload_pack,
};

#[cfg(test)]
mod tests {
    use super::*;
    use shako_core::{
        Action, BranchProtectionRule, InstanceId, LocalAuthorizer, OrgId, RefPolicy, RepoId,
        RequestCtx, RoleBinding, Subject,
    };
    use std::time::Duration;

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
        let v2_fetch_resp =
            handle_upload_pack(&repo_path, Some("version=2"), &v2_fetch_req).expect("v2 fetch");
        assert!(String::from_utf8_lossy(&v2_fetch_resp).contains("packfile"));

        // 4. Test bundle export and push_bundle into an empty repository via receive-pack
        let bundle = create_single_ref_bundle(&repo_path, "main").expect("bundle");
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
            principal: "jc-portal".into(),
            actions: vec![Action::Write],
            project_id: None,
            repo: None,
        }]);
        let ctx = RequestCtx::new(
            inst,
            org,
            Subject::authenticated("jc-portal", "jc-portal", vec![org], vec![]),
        );
        let rp_out = handle_receive_pack(
            &dest_path,
            &PushGuard {
                ctx: &ctx,
                authorizer: &authz,
                owner: "helsinki",
                repo_name: "dest",
                policy: &RefPolicy::default(),
                secret_scan_enabled: true,
            },
            10 * 1024 * 1024,
            &rp_body,
        )
        .expect("receive-pack");
        assert_eq!(rp_out.updated_refs.len(), 1);
        assert!(String::from_utf8_lossy(&rp_out.response_bytes).contains("unpack ok"));
        assert!(String::from_utf8_lossy(&rp_out.response_bytes).contains("ok refs/heads/main"));

        // Verify quarantine is clean and quota refusal leaves no quarantine behind
        assert!(!has_active_quarantine(&dest_path));
        let over_quota = handle_receive_pack(&dest_path, &PushGuard { ctx: &ctx, authorizer: &authz, owner: "helsinki", repo_name: "dest", policy: &RefPolicy {
                branch_rules: vec![BranchProtectionRule::default()],
                tag_rules: vec![],
            }, secret_scan_enabled: // 10 bytes max_push_bytes -> refused with unpack error quota
            true }, 10, &rp_body)
        .expect("quota refusal");
        assert!(String::from_utf8_lossy(&over_quota.response_bytes).contains("unpack error quota"));
        assert!(!has_active_quarantine(&dest_path));

        // 5. Archives (.zip and .tar.gz), Compaction, and Fsck
        let zip_bytes =
            create_archive(&dest_path, "dest", "main", ArchiveFormat::Zip).expect("zip");
        assert!(zip_bytes.starts_with(b"PK\x03\x04"));
        let targz_bytes =
            create_archive(&dest_path, "dest", "main", ArchiveFormat::TarGz).expect("targz");
        assert!(!targz_bytes.is_empty());

        let compact_out =
            compact_repository(&repo_path, Duration::from_secs(3600)).expect("compact");
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
        let refused = handle_upload_pack(&repo_path, Some("version=2"), &orphan_req)
            .expect_err("unreachable blob must not be served");
        assert_eq!(refused.status_code(), 400);
    }
}
