//! Language resolution edges and the localized number, plural and date formats (C1-C3).

use dataroom_web::i18n::*;

#[test]
fn dates_read_like_ckan_render_datetime_in_every_shipped_language() {
    let months = [
        ("sk", "januára", "decembra"),
        ("cs", "ledna", "prosince"),
        ("hu", "január", "december"),
        ("de", "Januar", "Dezember"),
        ("en", "January", "December"),
    ];
    for (lang, jan, dec) in months {
        let d1 = format_date_localized(2026, 1, 7, lang);
        let d12 = format_date_localized(2026, 12, 7, lang);
        assert!(
            d1.contains(jan) && d12.contains(dec),
            "{lang}: {d1} / {d12}"
        );
    }
    assert_eq!(format_date_localized(2026, 11, 7, "sk"), "7. novembra 2026");
    assert_eq!(
        format_date_localized(2026, 11, 7, "cs_CZ"),
        "7. listopadu 2026"
    );
    assert_eq!(
        format_date_localized(2026, 11, 7, "hu"),
        "2026. november 7."
    );
    assert_eq!(format_date_localized(2026, 3, 7, "de-AT"), "7. März 2026");
    assert_eq!(format_date_localized(2026, 11, 7, "en"), "November 7, 2026");
    assert_eq!(format_date_localized(2026, 11, 7, "fi"), "November 7, 2026");
    // Every month has its own name in every language.
    for lang in SHIPPED_LANGUAGES {
        let names: std::collections::BTreeSet<String> = (1..=12)
            .map(|m| format_date_localized(2000, m, 1, lang))
            .collect();
        assert_eq!(names.len(), 12, "{lang}");
    }
    // A month outside 1-12 (never from chrono) still gives the year, without a panic.
    for lang in SHIPPED_LANGUAGES {
        assert!(format_date_localized(2026, 13, 1, lang).contains("2026"));
    }
}

#[test]
fn counted_nouns_take_the_slavic_plural_forms() {
    let cases = [
        (
            "sk",
            [
                "0 datasetov",
                "1 dataset",
                "2 datasety",
                "4 datasety",
                "5 datasetov",
            ],
        ),
        (
            "cs",
            [
                "0 datových sad",
                "1 datová sada",
                "2 datové sady",
                "4 datové sady",
                "5 datových sad",
            ],
        ),
        (
            "de",
            [
                "0 Datensätze",
                "1 Datensatz",
                "2 Datensätze",
                "4 Datensätze",
                "5 Datensätze",
            ],
        ),
        (
            "hu",
            [
                "0 adatkészlet",
                "1 adatkészlet",
                "2 adatkészlet",
                "4 adatkészlet",
                "5 adatkészlet",
            ],
        ),
        (
            "en",
            [
                "0 datasets",
                "1 dataset",
                "2 datasets",
                "4 datasets",
                "5 datasets",
            ],
        ),
    ];
    for (lang, forms) in cases {
        for (n, want) in [0, 1, 2, 4, 5].into_iter().zip(forms) {
            assert_eq!(jc_rows(n, "datasets_n", lang), want, "{lang} {n}");
        }
    }
    assert_eq!(jc_rows(3, "rows", "sk"), "3 riadky");
    assert_eq!(jc_rows(1, "rows", "cs"), "1 řádek");
    assert_eq!(jc_rows(7, "rows", "de"), "7 Zeilen");
    assert_eq!(jc_rows(7, "rows", "hu"), "7 sor");
    assert_eq!(jc_rows(1, "rows", "fi"), "1 row");
    assert_eq!(jc_rows(12_345, "rows", "en"), "12\u{202f}345 rows");
}

#[test]
fn numbers_group_thousands_with_a_narrow_space() {
    assert_eq!(jc_number(0), "0");
    assert_eq!(jc_number(999), "999");
    assert_eq!(jc_number(1000), "1\u{202f}000");
    assert_eq!(jc_number(1_234_567), "1\u{202f}234\u{202f}567");
}

#[test]
fn words_fall_back_to_english_then_to_the_key() {
    assert_eq!(jc_t("no_such_key_x", "sk"), "no_such_key_x");
    assert_eq!(jc_t("search", "fi"), jc_t("search", "en"));
    assert_ne!(jc_t("search", "sk"), jc_t("search", "en"));
    assert_eq!(jc_t("search", "sk-SK"), jc_t("search", "sk"));
}

#[test]
fn language_names_and_codes() {
    for (code, name) in [
        ("sk", "slovenčina"),
        ("cs", "čeština"),
        ("en", "English"),
        ("hu", "magyar"),
        ("de", "Deutsch"),
        ("fi", "suomi"),
        ("xx", "English"),
    ] {
        assert_eq!(language_display_name(code), name);
    }
    assert_eq!(normalize_lang_code(" sk-SK "), "sk");
    assert_eq!(normalize_lang_code("CS_cz"), "cs");
    assert_eq!(normalize_lang_code(""), "");
    assert_eq!(
        available_languages(&["de".into(), "DE".into(), "fi".into(), "sk_SK".into()]),
        ["de", "sk"]
    );
    assert_eq!(available_languages(&[]), ["en"]);
}

#[test]
fn url_in_lang_keeps_the_page_and_query() {
    assert_eq!(url_in_lang("", "en", "/dataset/a", None), "/en/dataset/a");
    assert_eq!(
        url_in_lang("/data/", "sk", "dataset/a", Some("q=x")),
        "/data/sk/dataset/a?q=x"
    );
    assert_eq!(url_in_lang("/data", "cs", "/", Some("")), "/data/cs/");
}

fn resolved(lang: &str, path: &str, prefix: bool) -> LanguageResolution {
    LanguageResolution::Resolved {
        lang: lang.into(),
        stripped_path: path.into(),
        had_url_prefix: prefix,
    }
}

#[test]
fn language_resolution_edges() {
    let offered: Vec<String> = vec!["sk".into(), "en".into()];
    let r = |path: &str, cookie: Option<&str>, al: Option<&str>, def: &str| {
        resolve_request_language(path, cookie, al, &offered, def)
    };
    // A bare prefix and an upper-case prefix are languages; the root and routes are not.
    assert_eq!(r("/en", None, None, "sk"), resolved("en", "/", true));
    assert_eq!(
        r("/EN/dataset", None, None, "sk"),
        resolved("en", "/dataset", true)
    );
    assert_eq!(r("", None, None, "sk"), resolved("sk", "/", false));
    for route in [
        "/map/a",
        "/api/3/action/x",
        "/catalog.ttl",
        "/robots.txt",
        "/abc/x",
        "/x1/y",
    ] {
        assert_eq!(
            r(route, None, None, "sk"),
            resolved("sk", route, false),
            "{route}"
        );
    }
    assert_eq!(
        r("/fi/dataset", None, None, "sk"),
        LanguageResolution::UnknownUrlPrefix("fi".into())
    );
    // A cookie in a language dataroom does not ship falls through to Accept-Language.
    assert_eq!(
        r("/dataset", Some("jc_lang=fi; lang"), Some("en"), "sk"),
        resolved("en", "/dataset", false)
    );
    assert_eq!(
        r("/dataset", Some("lang=en"), None, "sk"),
        resolved("en", "/dataset", false)
    );
    // q=0 refuses a language; a broken q counts as 1; quality orders the rest.
    assert_eq!(
        r("/dataset", None, Some("sk;q=0, fi, en;q=0.5"), "cs"),
        resolved("en", "/dataset", false)
    );
    assert_eq!(
        r("/dataset", None, Some("en;q=0.4, sk;q=bad"), "en"),
        resolved("sk", "/dataset", false)
    );
    assert_eq!(
        r("/dataset", None, Some("fi, ,"), "en"),
        resolved("en", "/dataset", false)
    );
    // A default nobody ships falls back to the first offered language.
    assert_eq!(
        r("/dataset", None, None, "fi"),
        resolved("sk", "/dataset", false)
    );
}

#[test]
fn a_default_language_nobody_ships_falls_back_to_the_first_offered() {
    let offered: Vec<String> = vec!["cs".into(), "en".into()];
    assert_eq!(
        resolve_request_language("/dataset", None, None, &offered, "xx"),
        resolved("cs", "/dataset", false)
    );
}
