//! The corners of the website crawl the end-to-end test does not reach: start addresses that
//! are refused, repeated or on the site's twin host, a crawl cut by `maxPages` or `maxDepth`,
//! a sitemap check of a page it has no date for, answers that are refused, failed, external or
//! "not modified" for a page nobody read, the links a revisited page stored, and the same for
//! documents. Needs the test database (see `common`).

#[path = "common/db.rs"]
mod db;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use assistant::crawl::{
    check_sitemap, crawl_site, CrawlPolicy, CrawlReport, Crawler, Sink, GONE_AFTER_RUNS,
};
use assistant::project_scope;
use jc_core::kinds::assistant::{KnowledgeSourceSpec, PdfPolicy, SourceType, Visibility};
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use sqlx::PgPool;
use wiremock::matchers::{header_exists, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const HOST: &str = "www.city.test";

/// `www.city.test` is the mock server; no other name resolves.
struct Fixture(SocketAddr);

impl Resolve for Fixture {
    fn resolve(&self, name: Name) -> Resolving {
        let found = (name.as_str() == HOST).then_some(self.0);
        Box::pin(std::future::ready(match found {
            Some(addr) => Ok(Box::new(vec![addr].into_iter()) as Addrs),
            None => Err(format!("the fixture serves no host named {}", name.as_str()).into()),
        }))
    }
}

#[derive(Default)]
struct Recorded {
    pages: Vec<String>,
    documents: Vec<String>,
}

impl Sink for Recorded {
    fn page(
        &mut self,
        _page_id: i64,
        url: &str,
        _language: Option<&str>,
        _html: &str,
    ) -> impl std::future::Future<Output = ()> + Send {
        self.pages.push(url.to_owned());
        async {}
    }

    fn document(
        &mut self,
        _document_id: i64,
        url: &str,
        _mime: Option<&str>,
        _bytes: &[u8],
    ) -> impl std::future::Future<Output = ()> + Send {
        self.documents.push(url.to_owned());
        async {}
    }
}

fn crawler(server: &MockServer) -> Crawler {
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), server.address().port());
    Crawler::new(
        Arc::new(Fixture(addr)),
        CrawlPolicy {
            allow_plain_http: true,
            ..CrawlPolicy::default()
        },
    )
    .expect("the crawler builds")
}

fn spec(start_urls: &[String]) -> KnowledgeSourceSpec {
    KnowledgeSourceSpec {
        source: SourceType::Website,
        categories: Vec::new(),
        start_urls: start_urls.to_vec(),
        ckan_instance_ref: None,
        context_spaces: Vec::new(),
        organizations: Vec::new(),
        upload: None,
        sitemap: true,
        wordpress_types: vec![],
        include: Vec::new(),
        exclude: Vec::new(),
        max_depth: 3,
        max_pages: 100,
        pdf: PdfPolicy::default(),
        off_domain_documents: false,
        schedule: None,
        languages: Vec::new(),
        visibility: Visibility::Public,
    }
}

fn base(server: &MockServer) -> String {
    format!("http://{HOST}:{}", server.address().port())
}

async fn page(server: &MockServer, at: &str, body: &str) {
    Mock::given(method("GET"))
        .and(path(at))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html; charset=utf-8")
                .set_body_string(format!("<html lang=\"en\"><body>{body}</body></html>")),
        )
        .mount(server)
        .await;
}

async fn sitemap(server: &MockServer, urls: &[&str]) {
    let locs: String = urls
        .iter()
        .map(|u| format!("<url><loc>{}{u}</loc></url>", base(server)))
        .collect();
    Mock::given(method("GET"))
        .and(path("/sitemap.xml"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/xml")
                .set_body_string(format!(
                    "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
                     <urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">{locs}</urlset>"
                )),
        )
        .mount(server)
        .await;
}

async fn run(
    pool: &PgPool,
    crawler: &Crawler,
    source: &str,
    spec: &KnowledgeSourceSpec,
) -> (CrawlReport, Recorded) {
    let mut sink = Recorded::default();
    let report = crawl_site(pool, crawler, "org", "city", source, spec, &mut sink)
        .await
        .expect("the crawl runs");
    (report, sink)
}

/// A start address with a bad URL, one on the twin host, one with a password, and the same for
/// a document: the bad one is skipped, the twin is read under the site's own host, and a
/// password is refused for the page and for the document alike.
#[tokio::test]
async fn start_addresses_are_checked_one_by_one() {
    let (admin, pool, name) = db::database("crawl_gaps_start").await;
    let server = MockServer::start().await;
    page(&server, "/", "<p>Home</p>").await;
    page(&server, "/twin", "<p>Twin</p>").await;
    let (p, port) = (base(&server), server.address().port());
    let start = spec(&[
        format!("{p}/"),
        "not a url".into(),
        format!("http://city.test:{port}/twin"),
        format!("http://user:secret@{HOST}:{port}/secret"),
        format!("http://user:secret@{HOST}:{port}/secret.pdf"),
    ]);
    let (report, sink) = run(&pool, &crawler(&server), "start", &start).await;
    assert_eq!(report.pages_fetched, 2, "{report:?}");
    assert_eq!(report.refused, 2, "a password is no address: {report:?}");
    assert!(
        sink.pages.contains(&format!("{p}/twin")),
        "the twin is read under the site's host: {:?}",
        sink.pages
    );
    assert!(sink.documents.is_empty());
    db::drop_database(admin, pool, &name).await;
}

/// `maxPages` stops a walk with pages still queued, and a sitemap page deeper than `maxDepth`
/// is never read.
#[tokio::test]
async fn a_walk_stops_at_its_page_and_depth_caps() {
    let (admin, pool, name) = db::database("crawl_gaps_caps").await;
    let server = MockServer::start().await;
    page(&server, "/", "<a href=\"/a\">a</a><a href=\"/b\">b</a>").await;
    page(&server, "/a", "<p>a</p>").await;
    page(&server, "/b", "<p>b</p>").await;
    sitemap(&server, &["/about"]).await;
    page(&server, "/about", "<p>about</p>").await;
    let start = [format!("{}/", base(&server))];

    let mut capped = spec(&start);
    capped.max_pages = 1;
    let (report, _) = run(&pool, &crawler(&server), "capped", &capped).await;
    assert_eq!(report.pages_fetched, 1, "{report:?}");

    let mut shallow = spec(&start);
    shallow.max_depth = 0;
    let (report, sink) = run(&pool, &crawler(&server), "shallow", &shallow).await;
    assert_eq!(report.pages_fetched, 1, "only the start page: {report:?}");
    assert_eq!(sink.pages.len(), 1);
    db::drop_database(admin, pool, &name).await;
}

/// A sitemap check reads a page it has read already only when the sitemap dates a change; a
/// page with no date waits for the full crawl.
#[tokio::test]
async fn a_sitemap_check_leaves_an_undated_page_it_has_read() {
    let (admin, pool, name) = db::database("crawl_gaps_check").await;
    let server = MockServer::start().await;
    page(&server, "/", "<p>Home</p>").await;
    page(&server, "/about", "<p>About</p>").await;
    sitemap(&server, &["/about"]).await;
    let start = spec(&[format!("{}/", base(&server))]);
    let crawler = crawler(&server);
    let (first, _) = run(&pool, &crawler, "check", &start).await;
    assert_eq!(first.pages_fetched, 2, "{first:?}");
    let mut sink = Recorded::default();
    let checked = check_sitemap(&pool, &crawler, "org", "city", "check", &start, &mut sink)
        .await
        .expect("the check runs");
    assert_eq!(
        (checked.pages_fetched, checked.pages_unchanged),
        (0, 0),
        "{checked:?}"
    );
    db::drop_database(admin, pool, &name).await;
}

/// Answers a page can give that are no page: no location to follow, a redirect to a password
/// address, a redirect off the site for a sitemap entry, a "not modified" to a plain read; and a
/// page its patterns exclude is read for its links but not handed on.
#[tokio::test]
async fn a_page_that_answers_no_page_is_counted_and_skipped() {
    let (admin, pool, name) = db::database("crawl_gaps_answers").await;
    let server = MockServer::start().await;
    let (p, port) = (base(&server), server.address().port());
    page(
        &server,
        "/",
        "<a href=\"/dead\">d</a><a href=\"/redir\">r</a><a href=\"/nm\">n</a><a href=\"/skip\">s</a>",
    )
    .await;
    page(&server, "/skip", "<p>Skipped</p>").await;
    Mock::given(method("GET"))
        .and(path("/dead"))
        .respond_with(ResponseTemplate::new(302))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/redir"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("location", format!("http://user:secret@{HOST}:{port}/z")),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/nm"))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/ext"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("location", "https://elsewhere.test/x"),
        )
        .mount(&server)
        .await;
    sitemap(&server, &["/ext"]).await;
    let mut start = spec(&[format!("{p}/")]);
    start.exclude = vec!["/skip".into()];
    let (report, sink) = run(&pool, &crawler(&server), "answers", &start).await;
    assert_eq!(report.failed, 1, "no location: {report:?}");
    assert_eq!(report.refused, 1, "a password address: {report:?}");
    assert_eq!(
        report.pages_unchanged, 1,
        "a 304 to a plain read: {report:?}"
    );
    assert_eq!(
        report.pages_fetched, 2,
        "the start and the excluded page: {report:?}"
    );
    assert_eq!(
        sink.pages,
        [format!("{p}/")],
        "the excluded page is not handed on"
    );
    db::drop_database(admin, pool, &name).await;
}

/// A page the server calls "not modified" keeps the links it stored: a page link is queued
/// again, a document link is revisited, an external one waits, and a stored address that is no
/// URL is passed over.
#[tokio::test]
async fn a_revisited_page_follows_the_links_it_stored() {
    let (admin, pool, name) = db::database("crawl_gaps_revisit").await;
    let server = MockServer::start().await;
    let p = base(&server);
    for at in ["/", "/next"] {
        Mock::given(method("GET"))
            .and(path(at))
            .and(header_exists("if-none-match"))
            .respond_with(ResponseTemplate::new(304))
            .with_priority(1)
            .mount(&server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html")
                .insert_header("etag", "\"v1\"")
                .set_body_string(
                    "<html><body><a href=\"/next\">n</a><a href=\"/r.pdf\">r</a>\
                     <a href=\"https://example.org/\">e</a></body></html>",
                ),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/next"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html")
                .insert_header("etag", "\"v1\"")
                .set_body_string("<html><body><p>Next</p></body></html>"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/r.pdf"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/pdf")
                .set_body_bytes(b"%PDF-1.4 report".to_vec()),
        )
        .mount(&server)
        .await;
    let mut start = spec(&[format!("{p}/")]);
    start.sitemap = false;
    let crawler = crawler(&server);
    let (first, _) = run(&pool, &crawler, "revisit", &start).await;
    assert_eq!(first.pages_fetched, 2, "{first:?}");

    let mut tx = project_scope(&pool, "city").await.expect("scope");
    sqlx::query(
        "INSERT INTO links (from_page, project, to_url, kind) \
         SELECT id, project, 'not a url', 'page' FROM pages WHERE url = $1",
    )
    .bind(format!("{p}/"))
    .execute(&mut *tx)
    .await
    .expect("a link no URL parses from");
    tx.commit().await.expect("commit");

    let (second, sink) = run(&pool, &crawler, "revisit", &start).await;
    assert_eq!(second.pages_unchanged, 2, "{second:?}");
    assert_eq!(second.pages_fetched, 0, "{second:?}");
    assert!(sink.pages.is_empty());
    db::drop_database(admin, pool, &name).await;
}

/// Documents: one linked by two pages is read once; one at an address with a password, one that
/// redirects to such an address and one answering "not modified" to a plain read are each
/// counted; one that fails `GONE_AFTER_RUNS` runs in a row leaves the store.
#[tokio::test]
async fn a_document_is_read_once_and_a_failing_one_leaves_the_store() {
    let (admin, pool, name) = db::database("crawl_gaps_documents").await;
    let server = MockServer::start().await;
    let (p, port) = (base(&server), server.address().port());
    page(
        &server,
        "/",
        "<a href=\"/a\">a</a><a href=\"/b\">b</a><a href=\"/bad.pdf\">x</a>\
         <a href=\"/redir.pdf\">r</a><a href=\"/nm.pdf\">n</a>",
    )
    .await;
    page(&server, "/a", "<a href=\"/shared.pdf\">s</a>").await;
    page(&server, "/b", "<a href=\"/shared.pdf\">s</a>").await;
    Mock::given(method("GET"))
        .and(path("/shared.pdf"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/pdf")
                .set_body_bytes(b"%PDF-1.4 shared".to_vec()),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/bad.pdf"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/redir.pdf"))
        .respond_with(ResponseTemplate::new(302).insert_header(
            "location",
            format!("http://user:secret@{HOST}:{port}/q.pdf"),
        ))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/nm.pdf"))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;
    let mut start = spec(&[format!("{p}/")]);
    start.sitemap = false;
    let crawler = crawler(&server);
    let mut removed = 0;
    for run_number in 1..=GONE_AFTER_RUNS {
        let (report, sink) = run(&pool, &crawler, "docs", &start).await;
        removed += report.removed;
        if run_number == 1 {
            assert_eq!(
                sink.documents,
                [format!("{p}/shared.pdf")],
                "two links, one read"
            );
            assert_eq!(report.documents_fetched, 1, "{report:?}");
            assert_eq!(report.documents_unchanged, 1, "the plain 304: {report:?}");
            assert_eq!(report.refused, 1, "the password redirect: {report:?}");
        }
    }
    assert_eq!(
        removed, 1,
        "the failing document is dropped on its last run"
    );
    db::drop_database(admin, pool, &name).await;
}

/// A WordPress type whose address answers no JSON list ends that type, not the crawl.
#[tokio::test]
async fn a_wordpress_answer_that_is_no_list_ends_its_type() {
    let (admin, pool, name) = db::database("crawl_gaps_wordpress").await;
    let server = MockServer::start().await;
    page(&server, "/", "<p>Home</p>").await;
    Mock::given(method("GET"))
        .and(path("/wp-json/wp/v2/pages"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<html>not json</html>"))
        .mount(&server)
        .await;
    let mut start = spec(&[format!("{}/", base(&server))]);
    start.wordpress_types = vec!["pages".into()];
    let (report, sink) = run(&pool, &crawler(&server), "wordpress", &start).await;
    assert_eq!(report.pages_fetched, 1, "{report:?}");
    assert_eq!(sink.pages.len(), 1);
    db::drop_database(admin, pool, &name).await;
}

/// A robots.txt whose rules cannot be compiled says nothing a crawler may rely on: the site is
/// read as closed to it.
#[tokio::test]
async fn a_robots_file_that_cannot_be_read_closes_the_site() {
    let (admin, pool, name) = db::database("crawl_gaps_robots").await;
    let server = MockServer::start().await;
    page(&server, "/", "<p>Home</p>").await;
    Mock::given(method("GET"))
        .and(path("/robots.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string(format!(
            "User-agent: *\nDisallow: /{}$\n",
            "a".repeat(100_000)
        )))
        .mount(&server)
        .await;
    let start = spec(&[format!("{}/", base(&server))]);
    let (report, sink) = run(&pool, &crawler(&server), "robots", &start).await;
    assert_eq!(report.pages_fetched, 0, "{report:?}");
    assert!(report.disallowed >= 1, "{report:?}");
    assert!(sink.pages.is_empty());
    db::drop_database(admin, pool, &name).await;
}
