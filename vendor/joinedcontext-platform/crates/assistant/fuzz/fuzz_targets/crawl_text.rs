//! What a crawled site serves: robots.txt, sitemap XML, HTML pages, a WordPress listing and a PDF
//! (T-7768). Every parser ends in a value, never a panic.
#![no_main]

use assistant::crawl::{links, meta, patterns, robots, sitemap, wordpress};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let text = String::from_utf8_lossy(data);
    let _ = robots::extract_sitemaps(&text);
    if let Ok(robot) = texting_robots::Robot::new(robots::USER_AGENT_TOKEN, data) {
        let rules = robots::Robots::Rules(robot);
        let _ = rules.is_allowed("https://www.example.org/a/b?c=d");
        let _ = rules.is_allowed(&text);
    }
    let _ = sitemap::parse_sitemap_xml(&text);
    let _ = links::extract_language(&text);
    let base = url::Url::parse("https://www.example.org/a/b.html").expect("a fixed URL");
    let _ = links::extract_links(&base, &text, "www.example.org");
    let _ = meta::page(&text);
    let _ = meta::instant(&text);
    let _ = meta::pdf(data);
    let _ = wordpress::parse_links(data);
    let (pattern, path) = text.split_once('\n').unwrap_or((&text, "/"));
    let _ = patterns::glob_match(pattern, path);
    let _ = patterns::is_included(path, &[pattern.to_owned()], &[]);
});
