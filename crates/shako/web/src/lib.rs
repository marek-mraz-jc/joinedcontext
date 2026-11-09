#![forbid(unsafe_code)]
//! `shako-web`: Server-rendered, read-only HTML pages (askama 0.12, no JavaScript),
//! Markdown rendering through `pulldown-cmark` with raw HTML events escaped (Threat T14),
//! raw file downloads (`text/plain` / `Content-Disposition: attachment` for SVG/HTML),
//! and Gitea page-path redirects of R§1.1 (`00-task.md` section 1, Contract S18 subset).

use askama::Template;
use axum::Router;
use axum::extract::{Extension, Path, Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::get;
use jc_router::Mount;
use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd, html};
use serde::Deserialize;
use shako_api::{ShakoState, authenticate_request, resolve_and_authorize_repo};
use shako_core::{Action, ShakoError};
use shako_git::{
    diff_refs_files, list_branches, list_commits, list_tags, list_tree_entries, read_commit_info,
    read_file_at_ref,
};

/// Renders Markdown to safe HTML using `pulldown-cmark`, converting all raw HTML events
/// (`Event::Html` and `Event::InlineHtml`) into `Event::Text` so `<script>`, `<svg onload=...>`,
/// and `<iframe>` are HTML-escaped, and neutralizing unsafe URL schemes (`javascript:`, `data:`) (Threat T14).
/// A `# heading` renders as `<h2>`: the Markdown sits under the page's own `<h1>` (one h1 per page).
#[must_use]
pub fn render_safe_markdown(markdown: &str) -> String {
    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);

    let parser = Parser::new_ext(markdown, opts).map(|event| match event {
        Event::Html(raw) | Event::InlineHtml(raw) => Event::Text(raw),
        Event::Start(Tag::Heading {
            level: HeadingLevel::H1,
            id,
            classes,
            attrs,
        }) => Event::Start(Tag::Heading {
            level: HeadingLevel::H2,
            id,
            classes,
            attrs,
        }),
        Event::End(TagEnd::Heading(HeadingLevel::H1)) => {
            Event::End(TagEnd::Heading(HeadingLevel::H2))
        }
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) => {
            let safe_url = if is_safe_href(&dest_url) {
                dest_url
            } else {
                "#".into()
            };
            Event::Start(Tag::Link {
                link_type,
                dest_url: safe_url,
                title,
                id,
            })
        }
        Event::Start(Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) => {
            let safe_url = if is_safe_href(&dest_url) {
                dest_url
            } else {
                "#".into()
            };
            Event::Start(Tag::Image {
                link_type,
                dest_url: safe_url,
                title,
                id,
            })
        }
        other => other,
    });

    let mut html_out = String::new();
    html::push_html(&mut html_out, parser);
    html_out
}

fn is_safe_href(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.is_empty()
        || trimmed.starts_with('#')
        || (trimmed.starts_with('/') && !trimmed.starts_with("//"))
    {
        return true;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("javascript:")
        || lower.starts_with("data:")
        || lower.starts_with("vbscript:")
    {
        return false;
    }
    lower.starts_with("https://") || lower.starts_with("http://") || lower.starts_with("mailto:")
}

#[derive(Template)]
#[template(
    source = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{{ title }} — shako</title>
</head>
<body>
<header><h1>{{ title }}</h1></header>
<main>
<ul>
{% for r in repos %}
  <li><a href="{{ prefix }}/{{ r.owner }}/{{ r.name }}">{{ r.owner }}/{{ r.name }}</a> — {{ r.description }}</li>
{% endfor %}
</ul>
</main>
</body>
</html>"#,
    ext = "html"
)]
struct RepoListTemplate {
    title: String,
    prefix: String,
    repos: Vec<shako_core::RepoRecord>,
}

#[derive(Template)]
#[template(
    source = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{{ owner }}/{{ repo }} — shako</title>
</head>
<body>
<header>
  <h1>{{ owner }}/{{ repo }}</h1>
  <p>{{ description }}</p>
  <nav>
    <a href="{{ prefix }}/{{ owner }}/{{ repo }}/branches">Branches</a> |
    <a href="{{ prefix }}/{{ owner }}/{{ repo }}/tags">Tags</a> |
    <a href="{{ prefix }}/{{ owner }}/{{ repo }}/commits/{{ default_branch }}">Commits</a> |
    <a href="{{ prefix }}/{{ owner }}/{{ repo }}/pulls">Pull Requests</a>
  </nav>
</header>
<main>
  <section>
    <h2>Files ({{ default_branch }})</h2>
    <ul>
    {% for entry in entries %}
      <li><a href="{{ prefix }}/{{ owner }}/{{ repo }}/blob/{{ default_branch }}/{{ entry.path }}">{{ entry.path }}</a> ({{ entry.entry_type }})</li>
    {% endfor %}
    </ul>
  </section>
  {% if !readme_html.is_empty() %}
  <section class="readme">
    <h2>README</h2>
    <div>{{ readme_html|safe }}</div>
  </section>
  {% endif %}
</main>
</body>
</html>"#,
    ext = "html"
)]
struct RepoHomeTemplate {
    prefix: String,
    owner: String,
    repo: String,
    description: String,
    default_branch: String,
    entries: Vec<shako_git::TreeEntryInfo>,
    readme_html: String,
}

#[derive(Template)]
#[template(
    source = r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{{ path }} at {{ git_ref }} — {{ owner }}/{{ repo }}</title>
</head>
<body>
<header>
  <h1><a href="{{ prefix }}/{{ owner }}/{{ repo }}">{{ owner }}/{{ repo }}</a> / {{ path }}</h1>
  <p>Ref: <code>{{ git_ref }}</code> | SHA: <code>{{ blob_sha }}</code> | <a href="{{ prefix }}/{{ owner }}/{{ repo }}/raw/{{ git_ref }}/{{ path }}">Raw</a></p>
</header>
<main>
  <pre><code>{{ content }}</code></pre>
</main>
</body>
</html>"#,
    ext = "html"
)]
struct FileViewTemplate {
    prefix: String,
    owner: String,
    repo: String,
    git_ref: String,
    path: String,
    blob_sha: String,
    content: String,
}

#[derive(Template)]
#[template(
    source = r#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8"><title>{{ heading }} — {{ owner }}/{{ repo }}</title></head>
<body>
<header><h1>{{ owner }}/{{ repo }} — {{ heading }}</h1></header>
<main>
<ul>
{% for (name, sha) in items %}
  <li><strong>{{ name }}</strong> <code>{{ sha }}</code></li>
{% endfor %}
</ul>
</main>
</body>
</html>"#,
    ext = "html"
)]
struct RefListTemplate {
    owner: String,
    repo: String,
    heading: String,
    items: Vec<(String, String)>,
}

#[derive(Template)]
#[template(
    source = r#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8"><title>Commits — {{ owner }}/{{ repo }}</title></head>
<body>
<header><h1>{{ owner }}/{{ repo }} — Commits ({{ git_ref }})</h1></header>
<main>
<ul>
{% for c in commits %}
  <li><a href="{{ prefix }}/{{ owner }}/{{ repo }}/commit/{{ c.sha }}"><code>{{ c.sha }}</code></a> — {{ c.message }} ({{ c.author_name }})</li>
{% endfor %}
</ul>
</main>
</body>
</html>"#,
    ext = "html"
)]
struct CommitsTemplate {
    prefix: String,
    owner: String,
    repo: String,
    git_ref: String,
    commits: Vec<shako_git::CommitInfo>,
}

#[derive(Template)]
#[template(
    source = r#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8"><title>Commit {{ commit.sha }} — {{ owner }}/{{ repo }}</title></head>
<body>
<header>
  <h1>Commit <code>{{ commit.sha }}</code></h1>
  <p>Author: {{ commit.author_name }} &lt;{{ commit.author_email }}&gt; on {{ commit.author_date }}</p>
  <pre>{{ commit.message }}</pre>
</header>
<main>
  <h2>Changed Files</h2>
  <ul>
  {% for f in files %}
    <li><code>{{ f.status }}</code> {{ f.filename }}</li>
  {% endfor %}
  </ul>
</main>
</body>
</html>"#,
    ext = "html"
)]
struct CommitDiffTemplate {
    owner: String,
    repo: String,
    commit: shako_git::CommitInfo,
    files: Vec<shako_git::DiffFileEntry>,
}

#[derive(Template)]
#[template(
    source = r#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8"><title>Pull Requests — {{ owner }}/{{ repo }}</title></head>
<body>
<header><h1>{{ owner }}/{{ repo }} — Pull Requests</h1></header>
<main>
<ul>
{% for pr in prs %}
  <li><a href="{{ prefix }}/{{ owner }}/{{ repo }}/pulls/{{ pr.number }}">#{{ pr.number }} {{ pr.title }}</a> [{{ pr.state }}] ({{ pr.head_ref }} &rarr; {{ pr.base_ref }})</li>
{% endfor %}
</ul>
</main>
</body>
</html>"#,
    ext = "html"
)]
struct PullListTemplate {
    prefix: String,
    owner: String,
    repo: String,
    prs: Vec<shako_core::PullRequestRecord>,
}

#[derive(Template)]
#[template(
    source = r#"<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8"><title>PR #{{ pr.number }} {{ pr.title }} — {{ owner }}/{{ repo }}</title></head>
<body>
<header>
  <h1>#{{ pr.number }} {{ pr.title }} [{{ pr.state }}]</h1>
  <p>{{ pr.head_ref }} (<code>{{ pr.head_sha }}</code>) &rarr; {{ pr.base_ref }} (<code>{{ pr.base_sha }}</code>)</p>
</header>
<main>
  <section class="pr-body">{{ body_html|safe }}</section>
  <section>
    <h2>Changed Files ({{ files.len() }})</h2>
    <ul>
    {% for f in files %}
      <li><code>{{ f.status }}</code> {{ f.filename }}</li>
    {% endfor %}
    </ul>
  </section>
  <section>
    <h2>Reviews ({{ reviews.len() }})</h2>
    <ul>
    {% for r in reviews %}
      <li><strong>{{ r.author_login }}</strong> [{{ r.verdict.as_str() }}]: {{ r.body }}</li>
    {% endfor %}
    </ul>
  </section>
</main>
</body>
</html>"#,
    ext = "html"
)]
struct PullDetailTemplate {
    owner: String,
    repo: String,
    pr: shako_core::PullRequestRecord,
    body_html: String,
    files: Vec<shako_git::DiffFileEntry>,
    reviews: Vec<shako_core::ReviewRecord>,
}

fn not_found_html() -> Response {
    (StatusCode::NOT_FOUND, Html("<h1>404 Not Found</h1>")).into_response()
}

fn render_template<T: Template>(tpl: &T) -> Response {
    match tpl.render() {
        Ok(html) => (StatusCode::OK, Html(html)).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn web_repo_list(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
) -> Response {
    let Ok(ctx) = authenticate_request(&state, &headers, None, "shako") else {
        return not_found_html();
    };
    let mount = mount_ext.map_or_else(Mount::default, |Extension(m)| m);
    let repos = state
        .store()
        .list_repos(ctx.instance_id, ctx.org_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| {
            state
                .authorizer
                .check(
                    &ctx,
                    Action::Read,
                    &shako_core::ObjectTarget::Repository {
                        instance_id: r.instance_id,
                        org_id: r.org_id,
                        project_id: (!r.project_id.is_empty()).then(|| r.project_id.clone()),
                        owner: r.owner.clone(),
                        repo: r.name.clone(),
                        git_ref: None,
                    },
                )
                .allowed
        })
        .collect();
    render_template(&RepoListTemplate {
        title: "Repositories".into(),
        prefix: mount.path_prefix,
        repos,
    })
}

async fn web_repo_home(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Path((owner, repo_name)): Path<(String, String)>,
) -> Response {
    let Ok(ctx) = authenticate_request(&state, &headers, Some(&owner), "shako") else {
        return not_found_html();
    };
    let Ok((repo, disk_path)) =
        resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Read).await
    else {
        return not_found_html();
    };
    let mount = mount_ext.map_or_else(Mount::default, |Extension(m)| m);
    let def_b = repo.default_branch.clone();
    let disk_c = disk_path.clone();

    let (entries, readme_md) = tokio::task::spawn_blocking(move || {
        let entries = list_tree_entries(&disk_c, &def_b, false)
            .map(|(_, e)| e)
            .unwrap_or_default();
        let readme = read_file_at_ref(&disk_c, &def_b, "README.md")
            .ok()
            .flatten()
            .map(|(_, b)| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default();
        (entries, readme)
    })
    .await
    .unwrap_or_default();

    let readme_html = if readme_md.is_empty() {
        String::new()
    } else {
        render_safe_markdown(&readme_md)
    };

    render_template(&RepoHomeTemplate {
        prefix: mount.path_prefix,
        owner: repo.owner,
        repo: repo.name,
        description: repo.description,
        default_branch: repo.default_branch,
        entries,
        readme_html,
    })
}

async fn web_file_view(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Path((owner, repo_name, git_ref, path)): Path<(String, String, String, String)>,
) -> Response {
    let Ok(ctx) = authenticate_request(&state, &headers, Some(&owner), "shako") else {
        return not_found_html();
    };
    let Ok((repo, disk_path)) =
        resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Read).await
    else {
        return not_found_html();
    };
    let mount = mount_ext.map_or_else(Mount::default, |Extension(m)| m);
    let gr = git_ref.clone();
    let fp = path.clone();

    let Ok(Ok(Some((blob_sha, bytes)))) =
        tokio::task::spawn_blocking(move || read_file_at_ref(&disk_path, &gr, &fp)).await
    else {
        return not_found_html();
    };

    render_template(&FileViewTemplate {
        prefix: mount.path_prefix,
        owner: repo.owner,
        repo: repo.name,
        git_ref,
        path,
        blob_sha,
        content: String::from_utf8_lossy(&bytes).into_owned(),
    })
}

/// Raw file download: always serves `text/plain; charset=utf-8` or `application/octet-stream`,
/// `X-Content-Type-Options: nosniff`, and `Content-Disposition: attachment` for `.svg`, `.html`,
/// `.htm`, `.xhtml`, `.xml` (Contract S18, Threat T14).
async fn web_raw_file(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name, git_ref, path)): Path<(String, String, String, String)>,
) -> Response {
    let Ok(ctx) = authenticate_request(&state, &headers, Some(&owner), "shako") else {
        return not_found_html();
    };
    let Ok((_repo, disk_path)) =
        resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Read).await
    else {
        return not_found_html();
    };
    let fp = path.clone();
    let Ok(Ok(Some((_sha, bytes)))) =
        tokio::task::spawn_blocking(move || read_file_at_ref(&disk_path, &git_ref, &fp)).await
    else {
        return not_found_html();
    };

    let filename: String = path
        .rsplit('/')
        .next()
        .unwrap_or("file")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        .collect();
    let lower = filename.to_ascii_lowercase();
    let is_dangerous_ext = lower.ends_with(".svg")
        || lower.ends_with(".html")
        || lower.ends_with(".htm")
        || lower.ends_with(".xhtml")
        || lower.ends_with(".xml");
    let content_type = if is_dangerous_ext || std::str::from_utf8(&bytes).is_err() {
        "application/octet-stream"
    } else {
        "text/plain; charset=utf-8"
    };
    let disposition = format!("attachment; filename=\"{filename}\"");
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, content_type),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::CONTENT_DISPOSITION, disposition.as_str()),
        ],
        bytes,
    )
        .into_response()
}

async fn web_branches(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name)): Path<(String, String)>,
) -> Response {
    let Ok(ctx) = authenticate_request(&state, &headers, Some(&owner), "shako") else {
        return not_found_html();
    };
    let Ok((repo, disk_path)) =
        resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Read).await
    else {
        return not_found_html();
    };
    let items = tokio::task::spawn_blocking(move || list_branches(&disk_path))
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default();
    render_template(&RefListTemplate {
        owner: repo.owner,
        repo: repo.name,
        heading: "Branches".into(),
        items,
    })
}

async fn web_tags(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name)): Path<(String, String)>,
) -> Response {
    let Ok(ctx) = authenticate_request(&state, &headers, Some(&owner), "shako") else {
        return not_found_html();
    };
    let Ok((repo, disk_path)) =
        resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Read).await
    else {
        return not_found_html();
    };
    let items = tokio::task::spawn_blocking(move || list_tags(&disk_path))
        .await
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default()
        .into_iter()
        .map(|(n, _, sha, _)| (n, sha))
        .collect();
    render_template(&RefListTemplate {
        owner: repo.owner,
        repo: repo.name,
        heading: "Tags".into(),
        items,
    })
}

async fn web_commits(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Path((owner, repo_name, git_ref)): Path<(String, String, String)>,
) -> Response {
    let Ok(ctx) = authenticate_request(&state, &headers, Some(&owner), "shako") else {
        return not_found_html();
    };
    let Ok((repo, disk_path)) =
        resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Read).await
    else {
        return not_found_html();
    };
    let mount = mount_ext.map_or_else(Mount::default, |Extension(m)| m);
    let gr = git_ref.clone();
    let commits =
        tokio::task::spawn_blocking(move || list_commits(&disk_path, &gr, None, 50, 10_000))
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();
    render_template(&CommitsTemplate {
        prefix: mount.path_prefix,
        owner: repo.owner,
        repo: repo.name,
        git_ref,
        commits,
    })
}

async fn web_commit_diff(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name, sha)): Path<(String, String, String)>,
) -> Response {
    let Ok(ctx) = authenticate_request(&state, &headers, Some(&owner), "shako") else {
        return not_found_html();
    };
    let Ok((repo, disk_path)) =
        resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Read).await
    else {
        return not_found_html();
    };
    let Ok(Ok((commit, files))) = tokio::task::spawn_blocking(move || {
        let c = read_commit_info(&disk_path, &sha)?;
        let files = if let Some(parent) = c.parent_shas.first() {
            diff_refs_files(&disk_path, parent, &c.sha).unwrap_or_default()
        } else {
            Vec::new()
        };
        Ok::<_, ShakoError>((c, files))
    })
    .await
    else {
        return not_found_html();
    };
    render_template(&CommitDiffTemplate {
        owner: repo.owner,
        repo: repo.name,
        commit,
        files,
    })
}

async fn web_pulls_list(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    mount_ext: Option<Extension<Mount>>,
    Path((owner, repo_name)): Path<(String, String)>,
) -> Response {
    let Ok(ctx) = authenticate_request(&state, &headers, Some(&owner), "shako") else {
        return not_found_html();
    };
    let Ok((repo, _)) =
        resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Read).await
    else {
        return not_found_html();
    };
    let mount = mount_ext.map_or_else(Mount::default, |Extension(m)| m);
    let prs = state
        .store()
        .list_pull_requests(repo.instance_id, repo.org_id, Some(repo.id), None)
        .await
        .unwrap_or_default();
    render_template(&PullListTemplate {
        prefix: mount.path_prefix,
        owner: repo.owner,
        repo: repo.name,
        prs,
    })
}

async fn web_pull_detail(
    State(state): State<ShakoState>,
    headers: HeaderMap,
    Path((owner, repo_name, number)): Path<(String, String, u64)>,
) -> Response {
    let Ok(ctx) = authenticate_request(&state, &headers, Some(&owner), "shako") else {
        return not_found_html();
    };
    let Ok((repo, disk_path)) =
        resolve_and_authorize_repo(&state, &ctx, &owner, &repo_name, Action::Read).await
    else {
        return not_found_html();
    };
    let Some(pr) = state
        .store()
        .get_pull_request(repo.instance_id, repo.org_id, repo.id, number)
        .await
        .ok()
        .flatten()
    else {
        return not_found_html();
    };
    let reviews = state
        .store()
        .list_reviews(repo.instance_id, repo.org_id, Some(repo.id), Some(number))
        .await
        .unwrap_or_default();
    let base_sha = if !pr.merge_base.is_empty() {
        pr.merge_base.clone()
    } else {
        pr.base_sha.clone()
    };
    let head_sha = pr.head_sha.clone();
    let files =
        tokio::task::spawn_blocking(move || diff_refs_files(&disk_path, &base_sha, &head_sha))
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();
    let body_html = render_safe_markdown(&pr.body);

    render_template(&PullDetailTemplate {
        owner: repo.owner,
        repo: repo.name,
        pr,
        body_html,
        files,
        reviews,
    })
}

// =========================================================================
// Gitea Page-Path Redirects of R§1.1 (`00-task.md` section 1)
// =========================================================================

async fn redirect_gitea_src_branch(
    mount_ext: Option<Extension<Mount>>,
    Path((owner, repo, git_ref, path)): Path<(String, String, String, String)>,
) -> Response {
    let prefix = mount_ext.map_or_else(String::new, |Extension(m)| m.path_prefix);
    Redirect::temporary(&format!("{prefix}/{owner}/{repo}/blob/{git_ref}/{path}")).into_response()
}

async fn redirect_gitea_package_page(
    mount_ext: Option<Extension<Mount>>,
    Path((owner, pkg, ver)): Path<(String, String, String)>,
) -> Response {
    let prefix = mount_ext.map_or_else(String::new, |Extension(m)| m.path_prefix);
    Redirect::temporary(&format!("{prefix}/{owner}/packages/generic/{pkg}/{ver}")).into_response()
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OAuthRedirectQuery {
    #[serde(default)]
    pub redirect_to: Option<String>,
}

/// `GET /user/oauth2/keycloak?redirect_to=...` — validates `redirect_to` is a local path
/// starting with `/` and not `//` or `\` (open-redirect guard) and redirects (`gitea.rs:866-883`).
async fn redirect_gitea_oauth_keycloak(
    mount_ext: Option<Extension<Mount>>,
    Query(q): Query<OAuthRedirectQuery>,
) -> Response {
    let prefix = mount_ext.map_or_else(String::new, |Extension(m)| m.path_prefix);
    let target = q
        .redirect_to
        .filter(|p| p.starts_with('/') && !p.starts_with("//") && !p.contains('\\'))
        .unwrap_or_else(|| {
            if prefix.is_empty() {
                "/".into()
            } else {
                prefix
            }
        });
    Redirect::temporary(&target).into_response()
}

/// `/actions/runs/{n}` answers 404 until S16 (`00-task.md` section 1).
async fn web_actions_not_served() -> Response {
    not_found_html()
}

/// Builds the `shako-web` router (read-only HTML views + Gitea page-path redirects).
pub fn web_router() -> Router<ShakoState> {
    Router::new()
        .route("/", get(web_repo_list))
        .route("/user/oauth2/keycloak", get(redirect_gitea_oauth_keycloak))
        .route(
            "/{owner}/-/packages/generic/{pkg}/{ver}",
            get(redirect_gitea_package_page),
        )
        .route("/{owner}/{repo}", get(web_repo_home))
        .route(
            "/{owner}/{repo}/src/branch/{git_ref}/{*path}",
            get(redirect_gitea_src_branch),
        )
        .route("/{owner}/{repo}/blob/{git_ref}/{*path}", get(web_file_view))
        .route("/{owner}/{repo}/raw/{git_ref}/{*path}", get(web_raw_file))
        .route("/{owner}/{repo}/branches", get(web_branches))
        .route("/{owner}/{repo}/tags", get(web_tags))
        .route("/{owner}/{repo}/commits/{git_ref}", get(web_commits))
        .route("/{owner}/{repo}/commit/{sha}", get(web_commit_diff))
        .route("/{owner}/{repo}/pulls", get(web_pulls_list))
        .route("/{owner}/{repo}/pulls/{number}", get(web_pull_detail))
        .route("/{owner}/{repo}/actions", get(web_actions_not_served))
        .route(
            "/{owner}/{repo}/actions/runs/{number}",
            get(web_actions_not_served),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_escapes_raw_html_and_blocks_javascript_links() {
        // Blank lines end the HTML block, so the links below parse as links and reach the sanitizer.
        let md = "# Hello\n<script>alert(1)</script>\n<svg onload=alert(2)>\n\n[Click](javascript:alert(3))\n\n[Safe](https://city.example)";
        let rendered = render_safe_markdown(md);
        assert!(!rendered.contains("<script>"));
        assert!(!rendered.contains("<svg"));
        assert!(!rendered.contains("javascript:"), "{rendered}");
        assert!(rendered.contains("&lt;script&gt;"));
        assert!(rendered.contains("href=\"https://city.example\""));
    }

    #[test]
    fn a_markdown_h1_renders_below_the_page_h1() {
        let rendered = render_safe_markdown("# configuration\n\n## Layout\n");
        assert!(!rendered.contains("<h1"), "{rendered}");
        assert!(rendered.contains("<h2>configuration</h2>"), "{rendered}");
        assert!(rendered.contains("<h2>Layout</h2>"), "{rendered}");
    }
}
