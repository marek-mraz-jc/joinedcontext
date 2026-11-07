//! The plan view as an MCP Apps resource (T-5960, SEP-1865): a host that speaks MCP Apps
//! (claude.ai, ChatGPT, VS Code) renders `jc_change_plan`'s diff and `jc_pipeline_try_mapping`'s
//! sample table in its own sandboxed frame instead of printing the JSON.
//!
//! The view is one static page: it reads the tool result the host posts to it and nothing else.
//! It holds no token, its CSP lets it connect nowhere, and the only links it draws go to the
//! Portal origin written into it here.

use serde_json::{json, Value};

/// The view's address, named by each tool that renders in it.
pub const URI: &str = "ui://joinedcontext/plan-view.html";
/// The MCP Apps HTML profile; a host renders only a resource of this type.
pub const MIME: &str = "text/html;profile=mcp-app";
/// The tools whose answer the view renders.
pub const TOOLS: &[&str] = &["jc_change_plan", "jc_pipeline_try_mapping"];

const PAGE: &str = include_str!("plan_view.html");

/// The `_meta` a tool carries in `tools/list` when the view renders its answer.
pub fn tool_meta(tool: &str) -> Option<Value> {
    TOOLS
        .contains(&tool)
        .then(|| json!({ "resourceUri": URI, "visibility": ["model", "app"] }))
}

/// The resource's `_meta`: no outside origin to connect to or load from.
fn meta() -> Value {
    json!({
        "ui": {
            "csp": { "connectDomains": [], "resourceDomains": [] },
            "prefersBorder": true
        }
    })
}

/// The view's line in `resources/list`.
pub fn listed() -> Value {
    json!({
        "uri": URI,
        "name": "plan-view",
        "title": "Plan and sample view",
        "description": "Renders a plan's diff or a mapping's sample records; links to the Portal",
        "mimeType": MIME,
        "_meta": meta()
    })
}

/// The view's `resources/read` contents, with the Portal origin its links go to.
pub fn contents(public_base_url: &url::Url) -> Value {
    let origin = public_base_url.origin().ascii_serialization();
    // A JSON string literal inside a <script>: `<` escaped, so no value closes the element.
    let literal = serde_json::to_string(&origin)
        .unwrap_or_else(|_| "\"\"".to_owned())
        .replace('<', "\\u003c");
    json!([{
        "uri": URI,
        "mimeType": MIME,
        "text": PAGE.replace("__PORTAL_ORIGIN__", &literal),
        "_meta": meta()
    }])
}
