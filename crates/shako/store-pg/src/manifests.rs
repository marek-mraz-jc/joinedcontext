//! Manifests a release before T-7523 left in `package_version.manifest_bytes`, or in the log it
//! wrote, are copied to the blob store before shako serves. The column keeps its bytes for one
//! release, so that release still serves them after a rollback; nothing here reads it otherwise.

use crate::{PgStore, tables};
use shako_core::{
    BlobRecord, InstanceId, OrgId, ShakoResult, sha256_hex, validate_repo_or_owner_name,
};
use std::future::Future;

type Held = (uuid::Uuid, String, String, String, Vec<u8>);

impl PgStore {
    /// Copies each manifest of the tenant whose bytes are in the column and whose blob row is
    /// missing: `write(owner, digest, bytes)` puts the file, then the row and the quota follow
    /// in one transaction. One manifest is held at a time and no lock during `write`, so it may
    /// run again and beside itself. Returns how many it copied.
    pub async fn copy_manifests<W, F>(
        &self,
        instance_id: InstanceId,
        org_id: OrgId,
        write: W,
    ) -> ShakoResult<usize>
    where
        W: Fn(String, String, Vec<u8>) -> F,
        F: Future<Output = ShakoResult<()>>,
    {
        let tenant = (instance_id, org_id);
        let (mut after, mut copied) = (uuid::Uuid::nil(), 0);
        loop {
            let mut tx = self.read_tx(tenant).await?;
            // octet_length reads the stored size, not the value.
            let next: Option<Held> = sqlx::query_as(
                "SELECT t.id, t.owner, t.package_name, t.digest, t.manifest_bytes
                 FROM shako.package_version t
                 WHERE t.instance_id = $1 AND t.org_id = $2 AND t.id > $3
                   AND t.package_type = 'container' AND octet_length(t.manifest_bytes) > 0
                   AND NOT EXISTS (
                       SELECT 1 FROM shako.blob b
                       WHERE b.instance_id = t.instance_id AND b.org_id = t.org_id
                         AND b.owner = t.owner AND b.repo_name = t.package_name
                         AND b.digest = t.digest)
                 ORDER BY t.id LIMIT 1",
            )
            .bind(instance_id.as_uuid())
            .bind(org_id.as_uuid())
            .bind(after)
            .fetch_optional(&mut *tx)
            .await
            .map_err(tables::failed)?;
            jc_audit::commit(tx).await.map_err(tables::failed)?;
            let Some((id, owner, image, digest, bytes)) = next else {
                return Ok(copied);
            };
            after = id;
            let computed = format!("sha256:{}", sha256_hex(&bytes));
            if computed != digest || validate_repo_or_owner_name(&owner, "owner").is_err() {
                tracing::error!(
                    %id,
                    "shako: a stored manifest does not match its digest or owner; left in its row"
                );
                continue;
            }
            let size_bytes = bytes.len() as u64;
            write(owner.clone(), computed.clone(), bytes).await?;
            // It exists already, so it counts even past the limit; what is pushed next refuses.
            self.put_blob_counted(
                BlobRecord {
                    id: uuid::Uuid::now_v7(),
                    instance_id,
                    org_id,
                    owner,
                    repo_name: image,
                    digest: computed,
                    size_bytes,
                    created_at: jc_time::TimestampTz::now(),
                },
                None,
            )
            .await?;
            copied += 1;
        }
    }
}
