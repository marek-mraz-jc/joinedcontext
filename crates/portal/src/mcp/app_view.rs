//! The MCP Apps views (SEP-1865): a host that speaks MCP Apps (claude.ai, ChatGPT, VS Code)
//! renders `jc_change_plan`'s diff and `jc_pipeline_try_mapping`'s sample table in the plan view
//! (T-5960) and `jc_render`'s chart in the chart view (T-6866), in its own sandboxed frame
//! instead of printing the JSON.
//!
//! Each view is one static page: it reads the tool result the host posts to it and nothing else.
//! It holds no token, its CSP lets it connect nowhere, and the only links it draws go to the
//! Portal origin written into it here.

use serde_json::{json, Value};

/// The plan view's address.
pub const URI: &str = "ui://joinedcontext/plan-view.html";
/// The chart view's address.
pub const CHART_URI: &str = jc_chart::VIEW_URI;
/// The MCP Apps HTML profile; a host renders only a resource of this type.
pub const MIME: &str = "text/html;profile=mcp-app";

struct View {
    uri: &'static str,
    name: &'static str,
    title: &'static str,
    description: &'static str,
    page: &'static str,
    /// The tools whose answer the view renders.
    tools: &'static [&'static str],
}

const VIEWS: &[View] = &[
    View {
        uri: URI,
        name: "plan-view",
        title: "Plan and sample view",
        description: "Renders a plan's diff or a mapping's sample records; links to the Portal",
        page: include_str!("plan_view.html"),
        tools: &["jc_change_plan", "jc_pipeline_try_mapping"],
    },
    View {
        uri: CHART_URI,
        name: "chart-view",
        title: "Chart view",
        description: "Draws a jc_render chart live from the answer's points; connects nowhere",
        page: jc_chart::VIEW_PAGE,
        tools: &["jc_render"],
    },
];

/// The view that renders `tool`'s answer, if any.
pub fn view_of(tool: &str) -> Option<&'static str> {
    VIEWS
        .iter()
        .find(|view| view.tools.contains(&tool))
        .map(|view| view.uri)
}

/// The `_meta` a tool carries in `tools/list` when a view renders its answer.
pub fn tool_meta(tool: &str) -> Option<Value> {
    view_of(tool).map(|uri| json!({ "resourceUri": uri, "visibility": ["model", "app"] }))
}

/// A resource's `_meta`: no outside origin to connect to or load from.
fn meta() -> Value {
    json!({
        "ui": {
            "csp": { "connectDomains": [], "resourceDomains": [] },
            "prefersBorder": true
        }
    })
}

/// The views' lines in `resources/list`.
pub fn listed() -> Vec<Value> {
    VIEWS
        .iter()
        .map(|view| {
            json!({
                "uri": view.uri,
                "name": view.name,
                "title": view.title,
                "description": view.description,
                "mimeType": MIME,
                "_meta": meta()
            })
        })
        .collect()
}

/// The `resources/read` contents of the view at `uri`, with the Portal origin its links go to;
/// `None` for any other URI.
pub fn contents(uri: &str, public_base_url: &url::Url) -> Option<Value> {
    let view = VIEWS.iter().find(|view| view.uri == uri)?;
    let origin = public_base_url.origin().ascii_serialization();
    // A JSON string literal inside a <script>: `<` escaped, so no value closes the element.
    let literal = serde_json::to_string(&origin)
        .unwrap_or_else(|_| "\"\"".to_owned())
        .replace('<', "\\u003c");
    Some(json!([{
        "uri": view.uri,
        "mimeType": MIME,
        "text": view.page.replace("__PORTAL_ORIGIN__", &literal),
        "_meta": meta()
    }]))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The chart view draws from the posted numbers alone (T-6866): no script engine, no markup
    /// sink, no request and no URL but the one Portal link.
    #[test]
    fn chart_view_page_has_no_eval_innerhtml_or_url_sinks() {
        let page = jc_chart::VIEW_PAGE;
        for sink in [
            "eval(",
            "new Function",
            "innerHTML",
            "outerHTML",
            "insertAdjacentHTML",
            "document.write",
            "setAttribute(\"href\"",
            "xlink",
            ".src =",
            "<img",
            "fetch(",
            "XMLHttpRequest",
            "WebSocket",
            "EventSource",
            "import(",
            "localStorage",
            "<script src",
            "foreignObject",
        ] {
            assert!(!page.contains(sink), "the chart view holds {sink}");
        }
        assert_eq!(page.matches(".href =").count(), 1, "one Portal link");
        assert!(page.contains("a.href = url;"));
        assert!(page.contains("var url = PORTAL + \"/projects/\""));
        // Messages only from the parent frame, and only JSON-RPC 2.0.
        assert!(page.contains("if (event.source !== window.parent) return;"));
        assert!(page.contains("if (!m || m.jsonrpc !== \"2.0\") return;"));
        let base = url::Url::parse("https://portal.example.org/a/b").expect("url");
        let served = contents(CHART_URI, &base).expect("the chart view");
        let text = served[0]["text"].as_str().expect("text");
        assert!(text.contains(r#"var PORTAL = "https://portal.example.org";"#));
    }
}
