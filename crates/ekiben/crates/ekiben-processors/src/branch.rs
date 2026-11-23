use std::sync::Arc;

use async_trait::async_trait;
use ekiben_bloblang::{AssignTargetType, Executor, TargetType, Value};
use ekiben_core::{
    execute_all, Batch, ComponentConfig, CoreError, Part, PartMetaAdapter, PartsView, ProcContext,
    Processor, Registry,
};

pub fn register(registry: &Registry) {
    registry.register_processor("branch", |conf, ctx| {
        Ok(Arc::new(BranchProcessor::from_config(conf, ctx)?))
    });
}

#[derive(Clone, Debug)]
pub struct BranchMapError {
    pub index: usize,
    pub message: String,
}

/// Implements the `branch` processor (`internal/impl/pure/processor_branch.go`).
pub struct BranchProcessor {
    pub request_map: Option<Executor>,
    pub result_map: Option<Executor>,
    pub children: Vec<Arc<dyn Processor>>,
}

impl BranchProcessor {
    pub fn from_config(conf: &ComponentConfig, ctx: &ProcContext) -> Result<Self, CoreError> {
        let obj = conf
            .config
            .as_object()
            .ok_or_else(|| CoreError::config("branch config must be an object"))?;

        let request_map = match obj.get("request_map").and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => {
                Some(ctx.blobl_env.compile_mapping(s).map_err(|e| {
                    CoreError::config(format!("failed to parse request mapping: {e}"))
                })?)
            }
            _ => None,
        };

        let result_map = match obj.get("result_map").and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => {
                Some(ctx.blobl_env.compile_mapping(s).map_err(|e| {
                    CoreError::config(format!("failed to parse result mapping: {e}"))
                })?)
            }
            _ => None,
        };

        let proc_arr = obj
            .get("processors")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                CoreError::config("the branch processor requires at least one child processor")
            })?;
        if proc_arr.is_empty() {
            return Err(CoreError::config(
                "the branch processor requires at least one child processor",
            ));
        }

        let mut child_confs = Vec::with_capacity(proc_arr.len());
        for p in proc_arr {
            child_confs.push(ComponentConfig::from_json_value(p, "noop", None)?);
        }
        let children = ctx.registry.build_processor_list(&child_confs, ctx)?;

        Ok(Self {
            request_map,
            result_map,
            children,
        })
    }

    /// Returns paths read by `request_map`, prefixed by `"path"` or `"metadata"`, for `workflow` DAG resolution
    /// (`Branch.targetsUsed` in `internal/impl/pure/processor_branch.go`).
    pub fn targets_used(&self) -> Vec<Vec<String>> {
        let Some(ref req_map) = self.request_map else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for p in req_map.query_targets() {
            let prefix = match p.target_type {
                TargetType::Value => "path",
                TargetType::Metadata => "metadata",
                _ => continue,
            };
            let mut full = Vec::with_capacity(1 + p.path.len());
            full.push(prefix.to_string());
            full.extend(p.path);
            out.push(full);
        }
        out
    }

    /// Returns paths written by `result_map`, prefixed by `"path"` or `"metadata"`, for `workflow` DAG resolution
    /// (`Branch.targetsProvided` in `internal/impl/pure/processor_branch.go`).
    pub fn targets_provided(&self) -> Vec<Vec<String>> {
        let Some(ref res_map) = self.result_map else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for p in res_map.assignment_targets() {
            let prefix = match p.target_type {
                AssignTargetType::Value => "path",
                AssignTargetType::Metadata => "metadata",
                _ => continue,
            };
            let mut full = Vec::with_capacity(1 + p.path.len());
            full.push(prefix.to_string());
            full.extend(p.path);
            out.push(full);
        }
        out
    }

    /// Executes `request_map` and child processors (`Branch.createResult` in `processor_branch.go`).
    pub async fn create_result(
        &self,
        ctx: &ProcContext,
        parts: Vec<Option<Part>>,
        reference_batch: &Batch,
    ) -> Result<(Vec<Option<Part>>, Vec<BranchMapError>), (String, Vec<BranchMapError>)> {
        let original_len = parts.len();
        let mut skipped = Vec::new();
        let mut failed = Vec::new();
        let mut map_errs = Vec::new();
        let mut active_parts = Vec::with_capacity(original_len);

        let ref_view = PartsView(reference_batch);

        for (i, part_opt) in parts.into_iter().enumerate() {
            let Some(mut part) = part_opt else {
                skipped.push(i);
                continue;
            };
            if let Some(ref req_map) = self.request_map {
                part.set_bytes(bytes::Bytes::new());
                let res = {
                    let mut meta_adapter = PartMetaAdapter(&mut part);
                    req_map.map_part(i, &ref_view, &mut meta_adapter, Some(Value::Nothing))
                };
                match res {
                    Ok(None) => {
                        // `root = deleted()` -> skip branch for this message!
                        skipped.push(i);
                    }
                    Ok(Some(val)) => {
                        part.apply_mapped_value(val);
                        active_parts.push(part);
                    }
                    Err(e) => {
                        failed.push(i);
                        map_errs.push(BranchMapError {
                            index: i,
                            message: format!("request mapping failed: {e}"),
                        });
                    }
                }
            } else {
                active_parts.push(part);
            }
        }

        let mut proc_results = Vec::new();
        if !active_parts.is_empty() {
            match execute_all(ctx, &self.children, vec![active_parts]).await {
                Ok(batches) => {
                    if batches.is_empty() {
                        return Err((
                            "child processors resulted in zero messages".to_string(),
                            map_errs,
                        ));
                    }
                    proc_results = batches;
                }
                Err(e) => {
                    return Err((format!("child processors failed: {e}"), map_errs));
                }
            }
        }

        let mut aligned = align_branch_result(original_len, &skipped, &failed, proc_results)
            .map_err(|e| (e, map_errs.clone()))?;

        // Check if any child processor flagged a part with an error (`if fail := p.ErrorGet(); fail != nil`)
        for (i, slot) in aligned.iter_mut().enumerate() {
            if let Some(ref p) = slot {
                if let Some(err) = p.error_get() {
                    map_errs.push(BranchMapError {
                        index: i,
                        message: format!("processors failed: {}", err.message),
                    });
                    *slot = None;
                }
            }
        }

        Ok((aligned, map_errs))
    }

    /// Overlays `results` onto `payload` using `result_map` (`Branch.overlayResult` in `processor_branch.go`).
    /// Both callers pass `results` aligned to `payload` by `create_result` (one slot per part), so
    /// Bento's "message count diverged" check has nothing to catch here.
    pub fn overlay_result(
        &self,
        payload: &mut Batch,
        results: &[Option<Part>],
    ) -> Vec<BranchMapError> {
        let mut failed = Vec::new();
        let Some(ref res_map) = self.result_map else {
            return failed;
        };

        // Build a reference batch of result parts (with empty part placeholders for skipped slots)
        let results_batch: Batch = results
            .iter()
            .map(|opt| opt.clone().unwrap_or_default())
            .collect();
        let results_view = PartsView(&results_batch);

        for (i, (part, res_opt)) in payload.iter_mut().zip(results).enumerate() {
            if res_opt.is_none() {
                continue;
            }
            let initial_val = part.as_structured_ref().unwrap_or(Value::Nothing);
            let mut target_part = part.shallow_copy();
            let res = {
                let mut meta_adapter = PartMetaAdapter(&mut target_part);
                res_map.map_part(i, &results_view, &mut meta_adapter, Some(initial_val))
            };
            match res {
                Ok(None) => {
                    // Bento's `overlayResult` ignores `nil` (deleted) on result_map and keeps payload[i]
                }
                Ok(Some(val)) => {
                    target_part.apply_mapped_value(val);
                    *part = target_part;
                }
                Err(e) => {
                    failed.push(BranchMapError {
                        index: i,
                        message: format!("result mapping failed: {e}"),
                    });
                }
            }
        }

        failed
    }
}

fn align_branch_result(
    length: usize,
    skipped: &[usize],
    failed: &[usize],
    result: Vec<Batch>,
) -> Result<Vec<Option<Part>>, String> {
    let mut res_parts: Vec<Part> = Vec::new();
    for b in result {
        res_parts.extend(b);
    }

    let mut skipped_or_failed = Vec::with_capacity(skipped.len() + failed.len());
    skipped_or_failed.extend_from_slice(skipped);
    skipped_or_failed.extend_from_slice(failed);
    skipped_or_failed.sort_unstable();

    let total = res_parts.len() + skipped_or_failed.len();
    if total != length {
        return Err(format!(
            "message count from branch processors does not match request, started with {length} messages, finished with {total}"
        ));
    }

    if skipped_or_failed.is_empty() {
        return Ok(res_parts.into_iter().map(Some).collect());
    }

    let mut result_parts: Vec<Option<Part>> = vec![None; length];
    let mut s_index = 0usize;
    for (i, p) in res_parts.into_iter().enumerate() {
        while s_index < skipped_or_failed.len() && skipped_or_failed[s_index] == (i + s_index) {
            s_index += 1;
        }
        result_parts[i + s_index] = Some(p);
    }

    Ok(result_parts)
}

#[async_trait]
impl Processor for BranchProcessor {
    fn processor_type(&self) -> &'static str {
        "branch"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        mut batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        // Prepare branch parts with errors cleared (`p.ErrorSet(nil)`)
        let branch_parts: Vec<Option<Part>> = batch
            .iter()
            .map(|p| {
                let mut cp = p.shallow_copy();
                cp.clear_error();
                Some(cp)
            })
            .collect();

        let (aligned_results, map_errs) = match self.create_result(ctx, branch_parts, &batch).await
        {
            Ok(pair) => pair,
            Err((general_err, map_errs)) => {
                for p in &mut batch {
                    ctx.on_error(p, general_err.clone());
                }
                for e in map_errs {
                    if let Some(p) = batch.get_mut(e.index) {
                        ctx.on_error(p, e.message);
                    }
                }
                return Ok(vec![batch]);
            }
        };

        for e in map_errs {
            if let Some(p) = batch.get_mut(e.index) {
                ctx.on_error(p, e.message);
            }
        }

        for e in self.overlay_result(&mut batch, &aligned_results) {
            if let Some(p) = batch.get_mut(e.index) {
                ctx.on_error(p, e.message);
            }
        }

        Ok(vec![batch])
    }

    async fn close(&self) -> Result<(), CoreError> {
        for c in &self.children {
            c.close().await?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ekiben_core::{OrgContext, OrgId, OrgQuotas, ResourceManager, StreamMetrics};

    fn branch(config: serde_json::Value) -> BranchProcessor {
        let registry = Arc::new(Registry::new());
        crate::register_all(&registry);
        let ctx = ProcContext {
            stream_id: "s".into(),
            org: OrgContext::new(OrgId::default(), OrgQuotas::default()),
            resources: ResourceManager::new(),
            registry,
            blobl_env: ekiben_bloblang::Environment::new(),
            metrics: StreamMetrics::new("s", "o"),
            label: String::new(),
            component_type: "branch".into(),
            path: "pipeline.processors.0".into(),
            file_root: None,
            shutdown_rx: tokio::sync::watch::channel(false).1,
        };
        let conf = ComponentConfig {
            label: String::new(),
            component_type: "branch".into(),
            config,
            processors: Vec::new(),
        };
        BranchProcessor::from_config(&conf, &ctx).unwrap()
    }

    #[test]
    fn targets_name_paths_and_metadata_and_skip_variables() {
        let bare = branch(serde_json::json!({"processors": [{"noop": {}}]}));
        assert!(bare.targets_used().is_empty());
        assert!(bare.targets_provided().is_empty());
        let mapped = branch(serde_json::json!({
            "request_map": "let x = this.a\nroot.b = $x\nroot.c = @m",
            "result_map": "let v = 1\nroot.out = this\nmeta k = this.x",
            "processors": [{"noop": {}}]
        }));
        let used = mapped.targets_used();
        assert!(
            used.contains(&vec!["path".to_string(), "a".to_string()]),
            "{used:?}"
        );
        assert!(
            used.contains(&vec!["metadata".to_string(), "m".to_string()]),
            "{used:?}"
        );
        assert!(
            used.iter().all(|t| t[0] == "path" || t[0] == "metadata"),
            "{used:?}"
        );
        assert_eq!(
            mapped.targets_provided(),
            vec![
                vec!["path".to_string(), "out".to_string()],
                vec!["metadata".to_string(), "k".to_string()],
            ]
        );
    }
}
