use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use ekiben_bloblang::{dot_path_to_slice, Value};
use ekiben_core::{Batch, ComponentConfig, CoreError, Part, ProcContext, Processor, Registry};
use serde_json::Value as JsonValue;

use crate::branch::BranchProcessor;

pub fn register(registry: &Registry) {
    registry.register_processor("workflow", |conf, ctx| {
        Ok(Arc::new(WorkflowProcessor::from_config(conf, ctx)?))
    });
}

/// Implements the `workflow` DAG processor (`internal/impl/pure/processor_workflow.go` & `processor_workflow_branch_map.go`).
pub struct WorkflowProcessor {
    meta_path: Vec<String>,
    dag: Vec<Vec<String>>,
    branches: HashMap<String, Arc<BranchProcessor>>,
}

impl WorkflowProcessor {
    pub fn from_config(conf: &ComponentConfig, ctx: &ProcContext) -> Result<Self, CoreError> {
        let obj = conf
            .config
            .as_object()
            .ok_or_else(|| CoreError::config("workflow config must be an object"))?;

        let meta_str = obj
            .get("meta_path")
            .and_then(JsonValue::as_str)
            .unwrap_or("meta.workflow");
        let meta_path = if meta_str.is_empty() {
            Vec::new()
        } else {
            dot_path_to_slice(meta_str).to_vec()
        };

        let mut branches: HashMap<String, Arc<BranchProcessor>> = HashMap::new();
        if let Some(b_map) = obj.get("branches").and_then(JsonValue::as_object) {
            for (name, b_val) in b_map {
                if !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
                {
                    return Err(CoreError::config(format!(
                        "workflow branch name '{name}' contains invalid characters"
                    )));
                }
                let b_conf = ComponentConfig {
                    label: name.clone(),
                    component_type: "branch".to_string(),
                    config: b_val.clone(),
                    processors: Vec::new(),
                };
                let b_ctx = ctx.into_child("branch", name.clone(), format!("branches.{name}"));
                let bp = BranchProcessor::from_config(&b_conf, &b_ctx)?;
                branches.insert(name.clone(), Arc::new(bp));
            }
        }

        let mut order: Vec<Vec<String>> = Vec::new();
        if let Some(o_arr) = obj.get("order").and_then(JsonValue::as_array) {
            for (tier_idx, tier_val) in o_arr.iter().enumerate() {
                let tier_arr = tier_val.as_array().ok_or_else(|| {
                    CoreError::config(format!("workflow order tier {tier_idx} must be an array"))
                })?;
                if tier_arr.is_empty() {
                    return Err(CoreError::config(format!(
                        "explicit order tier '{tier_idx}' was empty"
                    )));
                }
                let mut tier = Vec::with_capacity(tier_arr.len());
                for item in tier_arr {
                    let s = item.as_str().ok_or_else(|| {
                        CoreError::config("workflow order branch name must be a string")
                    })?;
                    tier.push(s.to_string());
                }
                order.push(tier);
            }
        }

        let dag = if !order.is_empty() {
            verify_static_dag(&order, &branches)?;
            order
        } else {
            resolve_dynamic_dag(&branches)?
        };

        Ok(Self {
            meta_path,
            dag,
            branches,
        })
    }

    pub fn flow(&self) -> &[Vec<String>] {
        &self.dag
    }

    fn skip_from_meta(&self, root: &Value) -> HashSet<String> {
        let mut skip = HashSet::new();
        if self.meta_path.is_empty() {
            return skip;
        }
        let meta_obj = root.get_path(&self.meta_path);
        if let Value::Array(apply_arr) = meta_obj.get_path(&["apply".to_string()]) {
            if !apply_arr.is_empty() {
                for k in self.branches.keys() {
                    skip.insert(k.clone());
                }
                for item in apply_arr {
                    if let Value::String(s) = item {
                        skip.remove(&s);
                    }
                }
            }
        }
        for field in ["succeeded", "skipped"] {
            if let Value::Array(arr) = meta_obj.get_path(&[field.to_string()]) {
                for item in arr {
                    if let Value::String(s) = item {
                        if self.branches.contains_key(&s) {
                            skip.insert(s);
                        }
                    }
                }
            }
        }
        skip
    }
}

fn verify_static_dag(
    order: &[Vec<String>],
    branches: &HashMap<String, Arc<BranchProcessor>>,
) -> Result<(), CoreError> {
    let mut remaining: HashSet<String> = branches.keys().cloned().collect();
    let mut seen: HashSet<String> = HashSet::new();
    for tier in order {
        for name in tier {
            if !seen.insert(name.clone()) {
                return Err(CoreError::config(format!(
                    "branch specified in order listed multiple times: {name}"
                )));
            }
            if !branches.contains_key(name) {
                return Err(CoreError::config(format!(
                    "branch '{name}' listed in order was not found"
                )));
            }
            remaining.remove(name);
        }
    }
    if !remaining.is_empty() {
        let mut missing: Vec<String> = remaining.into_iter().collect();
        missing.sort();
        return Err(CoreError::config(format!(
            "the following branches were missing from order: {missing:?}"
        )));
    }
    Ok(())
}

fn dep_has_prefix(wanted: &[String], provided: &[String]) -> bool {
    wanted.len() >= provided.len() && wanted[..provided.len()] == *provided
}

/// Resolves a layered topological DAG from each branch's `targets_used()` and `targets_provided()`
/// (`resolveDynamicBranchDAG` in `processor_workflow_branch_map.go`).
fn resolve_dynamic_dag(
    branches: &HashMap<String, Arc<BranchProcessor>>,
) -> Result<Vec<Vec<String>>, CoreError> {
    if branches.is_empty() {
        return Ok(Vec::new());
    }
    let mut deps_map: HashMap<String, HashSet<String>> = HashMap::new();
    for (id, b) in branches {
        let wanted = b.targets_used();
        let mut deps = HashSet::new();
        for (other_id, other_b) in branches {
            if other_id == id {
                continue;
            }
            for tp in other_b.targets_provided() {
                if wanted.iter().any(|tn| dep_has_prefix(tn, &tp)) {
                    deps.insert(other_id.clone());
                    break;
                }
            }
        }
        deps_map.insert(id.clone(), deps);
    }

    let mut remaining: HashSet<String> = branches.keys().cloned().collect();
    let mut resolved: HashSet<String> = HashSet::new();
    let mut layers: Vec<Vec<String>> = Vec::new();

    loop {
        let mut ready: Vec<String> = remaining
            .iter()
            .filter(|id| deps_map[id.as_str()].iter().all(|d| resolved.contains(d)))
            .cloned()
            .collect();
        if ready.is_empty() {
            break;
        }
        ready.sort();
        for id in &ready {
            remaining.remove(id);
            resolved.insert(id.clone());
        }
        layers.push(ready);
    }

    if !remaining.is_empty() {
        let mut cycle_nodes: Vec<String> = remaining.into_iter().collect();
        cycle_nodes.sort();
        return Err(CoreError::config(format!(
            "failed to automatically resolve DAG, circular dependencies detected for branches: {cycle_nodes:?}"
        )));
    }

    Ok(layers)
}

#[derive(Clone, Default)]
struct ResultTracker {
    succeeded: HashSet<String>,
    skipped: HashSet<String>,
    failed: BTreeMap<String, String>,
}

impl ResultTracker {
    fn from_dag(dag: &[Vec<String>]) -> Self {
        let mut succeeded = HashSet::new();
        for layer in dag {
            for k in layer {
                succeeded.insert(k.clone());
            }
        }
        Self {
            succeeded,
            skipped: HashSet::new(),
            failed: BTreeMap::new(),
        }
    }

    fn mark_skipped(&mut self, k: &str) {
        self.succeeded.remove(k);
        self.skipped.insert(k.to_string());
    }

    fn mark_failed(&mut self, k: &str, why: String) {
        self.succeeded.remove(k);
        self.skipped.remove(k);
        self.failed.insert(k.to_string(), why);
    }

    fn to_value(&self) -> Value {
        let mut map = BTreeMap::new();
        if !self.succeeded.is_empty() {
            let mut s: Vec<String> = self.succeeded.iter().cloned().collect();
            s.sort();
            map.insert(
                "succeeded".to_string(),
                Value::Array(s.into_iter().map(Value::String).collect()),
            );
        }
        if !self.skipped.is_empty() {
            let mut s: Vec<String> = self.skipped.iter().cloned().collect();
            s.sort();
            map.insert(
                "skipped".to_string(),
                Value::Array(s.into_iter().map(Value::String).collect()),
            );
        }
        if !self.failed.is_empty() {
            let mut f = BTreeMap::new();
            for (k, v) in &self.failed {
                f.insert(k.clone(), Value::String(v.clone()));
            }
            map.insert("failed".to_string(), Value::Object(f.into()));
        }
        Value::Object(map.into())
    }
}

#[async_trait]
impl Processor for WorkflowProcessor {
    fn processor_type(&self) -> &'static str {
        "workflow"
    }

    async fn process_batch(
        &self,
        ctx: &ProcContext,
        mut batch: Batch,
    ) -> Result<Vec<Batch>, CoreError> {
        let skip_on_meta: Vec<HashSet<String>> = batch
            .iter()
            .map(|p| {
                p.as_structured_ref()
                    .map(|v| self.skip_from_meta(&v))
                    .unwrap_or_default()
            })
            .collect();

        let mut records: Vec<ResultTracker> = (0..batch.len())
            .map(|_| ResultTracker::from_dag(&self.dag))
            .collect();

        for layer in &self.dag {
            for p in &mut batch {
                let _ = p.as_bytes();
            }
            let ref_batch: Batch = batch.iter().map(Part::shallow_copy).collect();

            let mut futs = Vec::with_capacity(layer.len());
            for branch_id in layer {
                let bp = self.branches[branch_id].clone();
                let b_id = branch_id.clone();
                let mut branch_parts: Vec<Option<Part>> = Vec::with_capacity(batch.len());
                for (part_idx, part) in batch.iter().enumerate() {
                    if skip_on_meta[part_idx].contains(&b_id) {
                        branch_parts.push(None);
                    } else {
                        let mut cp = part.shallow_copy();
                        cp.clear_error();
                        branch_parts.push(Some(cp));
                    }
                }
                let rb = ref_batch.clone();
                let b_ctx = ctx.into_child("branch", b_id.clone(), format!("branches.{b_id}"));
                futs.push(async move {
                    let res = bp.create_result(&b_ctx, branch_parts, &rb).await;
                    (b_id, bp, res)
                });
            }

            let tier_results = futures::future::join_all(futs).await;

            for (b_id, bp, res) in tier_results {
                match res {
                    Ok((aligned, map_errs)) => {
                        for (j, slot) in aligned.iter().enumerate() {
                            if slot.is_none() {
                                records[j].mark_skipped(&b_id);
                            }
                        }
                        for e in map_errs {
                            records[e.index].mark_failed(&b_id, e.message);
                        }
                        match bp.overlay_result(&mut batch, &aligned) {
                            Ok(overlay_errs) => {
                                for e in overlay_errs {
                                    records[e.index].mark_failed(&b_id, e.message);
                                }
                            }
                            Err(err) => {
                                for rec in &mut records {
                                    rec.mark_failed(&b_id, err.clone());
                                }
                            }
                        }
                    }
                    Err((general_err, map_errs)) => {
                        for rec in &mut records {
                            rec.mark_failed(&b_id, general_err.clone());
                        }
                        for e in map_errs {
                            records[e.index].mark_failed(&b_id, e.message);
                        }
                    }
                }
            }
        }

        if !self.meta_path.is_empty() {
            for (i, part) in batch.iter_mut().enumerate() {
                match part.as_structured_mut() {
                    Ok(doc) => {
                        let previous = doc.get_path(&self.meta_path);
                        let mut current = records[i].to_value();
                        if !previous.is_null() {
                            if let Value::Object(ref mut m) = current {
                                m.insert("previous".to_string(), previous);
                            }
                        }
                        let _ = doc.set_path(&self.meta_path, current);
                    }
                    Err(e) => {
                        ctx.on_error(part, e);
                    }
                }
            }
        } else {
            for (i, part) in batch.iter_mut().enumerate() {
                if !records[i].failed.is_empty() {
                    let failed_keys: Vec<String> = records[i].failed.keys().cloned().collect();
                    ctx.on_error(part, format!("workflow branches failed: {failed_keys:?}"));
                }
            }
        }

        Ok(vec![batch])
    }

    async fn close(&self) -> Result<(), CoreError> {
        for b in self.branches.values() {
            b.close().await?;
        }
        Ok(())
    }
}
