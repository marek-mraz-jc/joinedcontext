use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
#[cfg(feature = "browser")]
use wasm_bindgen::prelude::*;

pub const ENGLISH_STOP_WORDS: &[&str] = &[
    "about",
    "above",
    "after",
    "again",
    "against",
    "all",
    "almost",
    "also",
    "although",
    "always",
    "among",
    "an",
    "and",
    "another",
    "any",
    "are",
    "around",
    "as",
    "at",
    "be",
    "because",
    "been",
    "before",
    "being",
    "below",
    "between",
    "both",
    "but",
    "by",
    "can",
    "could",
    "did",
    "do",
    "does",
    "doing",
    "down",
    "during",
    "each",
    "even",
    "every",
    "few",
    "for",
    "from",
    "further",
    "had",
    "has",
    "have",
    "having",
    "he",
    "her",
    "here",
    "hers",
    "herself",
    "him",
    "himself",
    "his",
    "how",
    "if",
    "in",
    "into",
    "is",
    "it",
    "its",
    "itself",
    "just",
    "me",
    "more",
    "most",
    "much",
    "must",
    "my",
    "myself",
    "no",
    "nor",
    "not",
    "now",
    "of",
    "off",
    "on",
    "once",
    "only",
    "or",
    "other",
    "our",
    "ours",
    "ourselves",
    "out",
    "over",
    "own",
    "same",
    "should",
    "so",
    "some",
    "such",
    "than",
    "that",
    "the",
    "their",
    "theirs",
    "them",
    "themselves",
    "then",
    "there",
    "these",
    "they",
    "this",
    "those",
    "through",
    "to",
    "too",
    "under",
    "until",
    "up",
    "very",
    "was",
    "we",
    "were",
    "what",
    "when",
    "where",
    "which",
    "while",
    "who",
    "whom",
    "whose",
    "why",
    "will",
    "with",
    "would",
    "you",
    "your",
    "yours",
    "yourself",
    "yourselves",
];

pub const FINNISH_STOP_WORDS: &[&str] = &[
    "aiheesta",
    "aikaan",
    "aikana",
    "aikoina",
    "aina",
    "ainakin",
    "aikaisemmin",
    "aiemmin",
    "alla",
    "alle",
    "alta",
    "alussa",
    "asti",
    "avulla",
    "edelleen",
    "edellä",
    "edessä",
    "ehkä",
    "eikä",
    "eilen",
    "eivät",
    "eli",
    "ellei",
    "enemmän",
    "ennen",
    "erittäin",
    "eri",
    "esiin",
    "että",
    "hän",
    "hänen",
    "hänelle",
    "häneltä",
    "hänet",
    "hänessä",
    "hänestä",
    "häntä",
    "he",
    "heidän",
    "heille",
    "heiltä",
    "heidät",
    "heissä",
    "heistä",
    "heitä",
    "hieman",
    "hitaasti",
    "huolimatta",
    "hyvin",
    "ilman",
    "itse",
    "itseään",
    "itsekseen",
    "ja",
    "jälkeen",
    "jo",
    "johon",
    "joiden",
    "joihin",
    "joilla",
    "joilta",
    "joissa",
    "joista",
    "joita",
    "joka",
    "jokainen",
    "jokin",
    "joku",
    "jolla",
    "jolle",
    "jolta",
    "jolloin",
    "jonka",
    "jonne",
    "jos",
    "joskus",
    "jossa",
    "josta",
    "jota",
    "jotta",
    "kautta",
    "kanssa",
    "kaikki",
    "kaikkia",
    "kaikkien",
    "kaikille",
    "kaikilta",
    "kaikissa",
    "kaikista",
    "koko",
    "koska",
    "kuin",
    "kuitenkin",
    "kuka",
    "kukin",
    "kun",
    "kuten",
    "kyllä",
    "lähellä",
    "läpi",
    "me",
    "meidän",
    "meille",
    "meiltä",
    "meidät",
    "meissä",
    "meistä",
    "meitä",
    "melkein",
    "miksi",
    "mikä",
    "mille",
    "miltä",
    "milloin",
    "minkä",
    "minne",
    "minä",
    "minun",
    "minulle",
    "minulta",
    "minut",
    "minussa",
    "minusta",
    "minua",
    "missä",
    "mistä",
    "mitä",
    "miten",
    "moni",
    "monia",
    "monien",
    "monissa",
    "monista",
    "mukaan",
    "mutta",
    "muu",
    "muut",
    "muuta",
    "muuten",
    "muiden",
    "muille",
    "muilta",
    "muissa",
    "muista",
    "myöhemmin",
    "myös",
    "myöskään",
    "näiden",
    "näille",
    "näiltä",
    "näissä",
    "näistä",
    "näitä",
    "näin",
    "nämä",
    "ne",
    "niiden",
    "niille",
    "niiltä",
    "niin",
    "niissä",
    "niistä",
    "niitä",
    "noin",
    "nopeasti",
    "nyt",
    "ole",
    "oleva",
    "olevan",
    "olevat",
    "oli",
    "olivat",
    "olla",
    "ollut",
    "olleet",
    "on",
    "ovat",
    "paljon",
    "paremmin",
    "pian",
    "pois",
    "puolesta",
    "päinvastoin",
    "saakka",
    "sama",
    "saman",
    "samalla",
    "sanoa",
    "sanoi",
    "se",
    "sekä",
    "sen",
    "sille",
    "siltä",
    "siinä",
    "siitä",
    "siksi",
    "silloin",
    "silti",
    "sinne",
    "sinä",
    "sinun",
    "sinulle",
    "sinulta",
    "sinut",
    "sinussa",
    "sinusta",
    "sinua",
    "sisällä",
    "sitten",
    "sitä",
    "suoraan",
    "taas",
    "tai",
    "takana",
    "takia",
    "tämä",
    "tämän",
    "tälle",
    "tältä",
    "tässä",
    "tästä",
    "tätä",
    "tähän",
    "tänne",
    "tavalla",
    "te",
    "teidän",
    "teille",
    "teiltä",
    "teidät",
    "teissä",
    "teistä",
    "teitä",
    "toinen",
    "toisen",
    "toista",
    "toiset",
    "toisten",
    "toisaalta",
    "tuo",
    "tuon",
    "tuolle",
    "tuolta",
    "tuossa",
    "tuosta",
    "tuota",
    "tuohon",
    "tuonne",
    "tulee",
    "tulla",
    "tuli",
    "tulleet",
    "tullut",
    "turhaan",
    "usein",
    "usea",
    "useat",
    "useiden",
    "useita",
    "vaan",
    "vaikka",
    "vain",
    "varten",
    "vasta",
    "vastaan",
    "vielä",
    "viime",
    "voida",
    "voisi",
    "voidaan",
    "voi",
    "voivat",
    "vuoksi",
    "vuonna",
    "vähän",
    "vähemmän",
    "yhä",
    "yksi",
    "yksin",
    "yleensä",
    "yli",
];

fn is_en_stop(token: &str) -> bool {
    ENGLISH_STOP_WORDS.contains(&token)
}

fn is_fi_stop(token: &str) -> bool {
    FINNISH_STOP_WORDS.contains(&token)
}

/// Finnish when at least two of its stop words occur and they outnumber the English ones: the two
/// lists share short words ("on", "se"), and an English headline is not Finnish for holding them.
pub fn detect_language(text: &str) -> &'static str {
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    let fi = words.iter().filter(|w| is_fi_stop(w)).count();
    let en = words.iter().filter(|w| is_en_stop(w)).count();
    if fi >= 2 && fi > en {
        "fi"
    } else {
        "en"
    }
}

/// Words that only name the city whose news this is: in every article, so in no topic.
const CITY_WORDS: &[&str] = &[
    "helsinki",
    "helsingin",
    "helsingissä",
    "helsinkiin",
    "helsingistä",
    "helsingfors",
    "city",
    "kaupunki",
    "kaupungin",
    "kaupunginosa",
    "stad",
    "staden",
];

pub fn stem_english(word: &str) -> String {
    for suffix in ["ing", "ly", "ed", "es", "s"] {
        if word.ends_with(suffix) {
            let stem_byte_len = word.len() - suffix.len();
            let stem = &word[..stem_byte_len];
            if stem.chars().count() >= 3 {
                return stem.to_string();
            }
        }
    }
    word.to_string()
}

pub fn stem_finnish(word: &str) -> String {
    const FINNISH_ENDINGS: &[&str] = &[
        "ssa", "ssä", "sta", "stä", "lla", "llä", "lta", "ltä", "lle", "ksi", "n", "t", "a", "ä",
    ];
    for &ending in FINNISH_ENDINGS {
        if word.ends_with(ending) {
            let stem_byte_len = word.len() - ending.len();
            let stem = &word[..stem_byte_len];
            if stem.chars().count() >= 3 {
                return stem.to_string();
            }
        }
    }
    word.to_string()
}

pub fn tokenize(text: &str, lang: &str) -> Vec<String> {
    let lower = text.to_lowercase();
    let raw_tokens: Vec<&str> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();

    let mut tokens = Vec::new();
    for token in raw_tokens {
        if token.chars().count() < 3 {
            continue;
        }
        if token.chars().all(|c| c.is_numeric()) {
            continue;
        }
        // Either language's stop words go whatever the language: the language picks the stemmer.
        if is_en_stop(token) || is_fi_stop(token) || CITY_WORDS.contains(&token) {
            continue;
        }

        let stemmed = if lang == "fi" {
            stem_finnish(token)
        } else {
            stem_english(token)
        };

        if stemmed.chars().count() >= 3 {
            tokens.push(stemmed);
        }
    }
    tokens
}

pub fn tfidf(docs: &[Vec<String>]) -> (Vec<BTreeMap<u32, f32>>, Vec<String>) {
    let n = docs.len();
    if n == 0 {
        return (Vec::new(), Vec::new());
    }

    let mut df_map: BTreeMap<String, usize> = BTreeMap::new();
    for doc in docs {
        let unique_terms: BTreeSet<&str> = doc.iter().map(|s| s.as_str()).collect();
        for term in unique_terms {
            *df_map.entry(term.to_string()).or_default() += 1;
        }
    }

    let mut vocab: Vec<String> = if n < 10 {
        df_map.keys().cloned().collect()
    } else {
        df_map
            .iter()
            .filter(|(_, &df)| df >= 2)
            .map(|(term, _)| term.clone())
            .collect()
    };
    vocab.sort();

    let term_to_idx: HashMap<&str, u32> = vocab
        .iter()
        .enumerate()
        .map(|(i, t)| (t.as_str(), i as u32))
        .collect();

    let idf_list: Vec<f32> = vocab
        .iter()
        .map(|term| {
            let df = *df_map.get(term).unwrap_or(&1) as f32;
            ((1.0 + n as f32) / (1.0 + df)).ln() + 1.0
        })
        .collect();

    let mut vectors = Vec::with_capacity(n);
    for doc in docs {
        let mut tf_counts: BTreeMap<u32, f32> = BTreeMap::new();
        for token in doc {
            if let Some(&idx) = term_to_idx.get(token.as_str()) {
                *tf_counts.entry(idx).or_default() += 1.0;
            }
        }

        let mut vec = BTreeMap::new();
        let mut norm_sq = 0.0f32;
        for (idx, tf) in tf_counts {
            let idf = idf_list[idx as usize];
            let val = tf * idf;
            norm_sq += val * val;
            vec.insert(idx, val);
        }

        if norm_sq > 0.0 {
            let norm = norm_sq.sqrt();
            for val in vec.values_mut() {
                *val /= norm;
            }
        } else {
            vec.clear();
        }
        vectors.push(vec);
    }

    (vectors, vocab)
}

struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    fn new(seed: u64) -> Self {
        let state = if seed == 0 { 0xda942042e4dd58b5 } else { seed };
        Self { state }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn cosine_similarity(a: &BTreeMap<u32, f32>, b: &BTreeMap<u32, f32>) -> f32 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let mut dot = 0.0f32;
    let mut it_a = a.iter();
    let mut it_b = b.iter();
    let mut cur_a = it_a.next();
    let mut cur_b = it_b.next();
    while let (Some((ka, va)), Some((kb, vb))) = (cur_a, cur_b) {
        match ka.cmp(kb) {
            std::cmp::Ordering::Less => cur_a = it_a.next(),
            std::cmp::Ordering::Greater => cur_b = it_b.next(),
            std::cmp::Ordering::Equal => {
                dot += va * vb;
                cur_a = it_a.next();
                cur_b = it_b.next();
            }
        }
    }
    dot
}

#[derive(Debug, Clone)]
pub struct KMeansResult {
    pub assignments: Vec<Option<usize>>,
    pub centroids: Vec<BTreeMap<u32, f32>>,
}

pub fn kmeans(vectors: &[BTreeMap<u32, f32>], k: usize, seed: u64) -> KMeansResult {
    if vectors.is_empty() {
        return KMeansResult {
            assignments: Vec::new(),
            centroids: Vec::new(),
        };
    }

    let valid_indices: Vec<usize> = vectors
        .iter()
        .enumerate()
        .filter(|(_, v)| !v.is_empty())
        .map(|(i, _)| i)
        .collect();

    if valid_indices.is_empty() {
        return KMeansResult {
            assignments: vec![None; vectors.len()],
            centroids: Vec::new(),
        };
    }

    let target_k = k.clamp(1, valid_indices.len());
    let mut rng = XorShift64::new(seed);
    let mut centroids: Vec<BTreeMap<u32, f32>> = Vec::with_capacity(target_k);

    let first_idx = valid_indices[(rng.next_u64() as usize) % valid_indices.len()];
    centroids.push(vectors[first_idx].clone());

    while centroids.len() < target_k {
        let mut distances: Vec<f32> = Vec::with_capacity(valid_indices.len());
        let mut sum_dist_sq = 0.0f32;

        for &v_idx in &valid_indices {
            let v = &vectors[v_idx];
            let mut min_dist = 1.0f32;
            for c in &centroids {
                let dist = (1.0 - cosine_similarity(v, c)).max(0.0);
                if dist < min_dist {
                    min_dist = dist;
                }
            }
            let dist_sq = min_dist * min_dist;
            distances.push(dist_sq);
            sum_dist_sq += dist_sq;
        }

        if sum_dist_sq <= 0.0 {
            break;
        }

        let threshold = rng.next_f64() as f32 * sum_dist_sq;
        let mut running = 0.0f32;
        let mut chosen_idx = valid_indices[0];
        for (i, &d_sq) in distances.iter().enumerate() {
            running += d_sq;
            if running >= threshold {
                chosen_idx = valid_indices[i];
                break;
            }
        }
        centroids.push(vectors[chosen_idx].clone());
    }

    let actual_k = centroids.len();
    let mut assignments: Vec<Option<usize>> = vec![None; vectors.len()];

    for _ in 0..50 {
        let mut changed = false;

        for (i, v) in vectors.iter().enumerate() {
            if v.is_empty() {
                assignments[i] = None;
                continue;
            }

            let mut best_sim = -1.0f32;
            let mut best_cluster = 0;
            for (c_idx, c) in centroids.iter().enumerate() {
                let sim = cosine_similarity(v, c);
                if sim > best_sim {
                    best_sim = sim;
                    best_cluster = c_idx;
                }
            }

            let next_assignment = Some(best_cluster);
            if assignments[i] != next_assignment {
                assignments[i] = next_assignment;
                changed = true;
            }
        }

        if !changed {
            break;
        }

        for (c_idx, centroid) in centroids.iter_mut().enumerate() {
            let mut sum_vec: BTreeMap<u32, f32> = BTreeMap::new();
            let mut count = 0usize;

            for (i, v) in vectors.iter().enumerate() {
                if assignments[i] == Some(c_idx) {
                    count += 1;
                    for (&term_idx, &val) in v {
                        *sum_vec.entry(term_idx).or_default() += val;
                    }
                }
            }

            if count > 0 {
                let mut norm_sq = 0.0f32;
                for &val in sum_vec.values() {
                    norm_sq += val * val;
                }
                if norm_sq > 0.0 {
                    let norm = norm_sq.sqrt();
                    for val in sum_vec.values_mut() {
                        *val /= norm;
                    }
                    *centroid = sum_vec;
                }
            }
        }
    }

    KMeansResult {
        assignments,
        centroids: centroids.into_iter().take(actual_k).collect(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Keyword {
    pub term: String,
    pub weight: f32,
}

#[derive(Debug, Clone)]
pub struct Cluster {
    pub centroid: BTreeMap<String, f32>,
    pub articles: Vec<String>,
}

pub fn keywords(cluster: &Cluster) -> Vec<Keyword> {
    let mut items: Vec<Keyword> = cluster
        .centroid
        .iter()
        .filter(|(_, &w)| w > 0.0)
        .map(|(term, &weight)| Keyword {
            term: term.clone(),
            weight,
        })
        .collect();
    items.sort_by(|a, b| {
        b.weight
            .partial_cmp(&a.weight)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.term.cmp(&b.term))
    });
    items.truncate(6);
    items
}

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
}

fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 => 31,
        2 => {
            if is_leap(y) {
                29
            } else {
                28
            }
        }
        3 => 31,
        4 => 30,
        5 => 31,
        6 => 30,
        7 => 31,
        8 => 31,
        9 => 30,
        10 => 31,
        11 => 30,
        12 => 31,
        _ => 30,
    }
}

fn day_of_week(y: i32, m: u32, d: u32) -> u32 {
    static T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let mut y = y;
    if m < 3 {
        y -= 1;
    }
    let dow = (y + y / 4 - y / 100 + y / 400 + T[(m - 1) as usize] + d as i32) % 7;
    if dow == 0 {
        7
    } else {
        dow as u32
    }
}

fn day_of_year(y: i32, m: u32, d: u32) -> u32 {
    let mut sum = d;
    for prev_m in 1..m {
        sum += days_in_month(y, prev_m);
    }
    sum
}

fn add_days(mut y: i32, mut m: u32, mut d: i32, delta: i32) -> (i32, u32, u32) {
    d += delta;
    while d < 1 {
        if m == 1 {
            y -= 1;
            m = 12;
        } else {
            m -= 1;
        }
        d += days_in_month(y, m) as i32;
    }
    while d > days_in_month(y, m) as i32 {
        d -= days_in_month(y, m) as i32;
        if m == 12 {
            y += 1;
            m = 1;
        } else {
            m += 1;
        }
    }
    (y, m, d as u32)
}

pub fn iso_week(date_str: &str) -> Option<String> {
    if date_str.len() < 10 {
        return None;
    }
    let y = date_str[0..4].parse::<i32>().ok()?;
    let m = date_str[5..7].parse::<u32>().ok()?;
    let d = date_str[8..10].parse::<u32>().ok()?;
    if !(1..=12).contains(&m) || d < 1 || d > days_in_month(y, m) {
        return None;
    }

    let dow = day_of_week(y, m, d);
    let delta_to_thursday = 4 - (dow as i32);
    let (thurs_y, thurs_m, thurs_d) = add_days(y, m, d as i32, delta_to_thursday);
    let doy = day_of_year(thurs_y, thurs_m, thurs_d);
    let week = (doy - 1) / 7 + 1;
    Some(format!("{:04}-W{:02}", thurs_y, week))
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WeekShare {
    pub week: String,
    pub shares: Vec<f32>,
}

fn monday_of_iso_week(week_str: &str) -> Option<(i32, u32, u32)> {
    if week_str.len() < 8 || !week_str.contains("-W") {
        return None;
    }
    let y = week_str[0..4].parse::<i32>().ok()?;
    let w = week_str[6..8].parse::<u32>().ok()?;
    let dow_jan4 = day_of_week(y, 1, 4);
    let (w1_y, w1_m, w1_d) = add_days(y, 1, 4, 1 - dow_jan4 as i32);
    Some(add_days(w1_y, w1_m, w1_d as i32, (w as i32 - 1) * 7))
}

pub fn weekly_share_with_k(
    dates: &[Option<String>],
    labels: &[Option<usize>],
    k: usize,
) -> Vec<WeekShare> {
    if dates.is_empty() || k == 0 {
        return Vec::new();
    }

    let mut article_weeks: Vec<Option<String>> = Vec::with_capacity(dates.len());
    let mut all_valid_weeks: BTreeSet<String> = BTreeSet::new();

    for date_opt in dates {
        let w_opt = date_opt.as_deref().and_then(iso_week);
        if let Some(ref w) = w_opt {
            all_valid_weeks.insert(w.clone());
        }
        article_weeks.push(w_opt);
    }

    if all_valid_weeks.is_empty() {
        return Vec::new();
    }

    let min_week = all_valid_weeks.iter().next().cloned().unwrap();
    let max_week = all_valid_weeks.iter().next_back().cloned().unwrap();

    let mut sequence: Vec<String> = Vec::new();
    let mut cur_mon = match monday_of_iso_week(&min_week) {
        Some(m) => m,
        None => return Vec::new(),
    };

    loop {
        let cur_date_str = format!("{:04}-{:02}-{:02}", cur_mon.0, cur_mon.1, cur_mon.2);
        let cur_week = match iso_week(&cur_date_str) {
            Some(w) => w,
            None => break,
        };
        sequence.push(cur_week.clone());
        if cur_week == max_week {
            break;
        }
        cur_mon = add_days(cur_mon.0, cur_mon.1, cur_mon.2 as i32, 7);
    }

    let mut result = Vec::with_capacity(sequence.len());
    for w in sequence {
        let mut counts = vec![0usize; k];
        let mut total_assigned_in_week = 0usize;

        for (i, art_w) in article_weeks.iter().enumerate() {
            if art_w.as_deref() == Some(&w) {
                if let Some(topic_idx) = labels.get(i).copied().flatten() {
                    if topic_idx < k {
                        counts[topic_idx] += 1;
                        total_assigned_in_week += 1;
                    }
                }
            }
        }

        let shares: Vec<f32> = if total_assigned_in_week > 0 {
            counts
                .iter()
                .map(|&c| c as f32 / total_assigned_in_week as f32)
                .collect()
        } else {
            vec![0.0f32; k]
        };

        result.push(WeekShare { week: w, shares });
    }

    result
}

#[derive(Debug, Deserialize)]
pub struct InputArticle {
    pub id: String,
    pub title: String,
    pub summary: Option<String>,
    pub published: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AnalysisInput {
    pub articles: Vec<InputArticle>,
    pub k: Option<usize>,
    pub seed: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct TopicOutput {
    pub id: usize,
    pub keywords: Vec<Keyword>,
    pub articles: Vec<String>,
    pub share: f32,
}

#[derive(Debug, Serialize, Deserialize, PartialEq)]
pub struct AnalysisOutput {
    pub topics: Vec<TopicOutput>,
    pub weeks: Vec<WeekShare>,
    pub unassigned: Vec<String>,
}

pub fn run_analyse(input_str: &str) -> Result<String, String> {
    let input: AnalysisInput = match serde_json::from_str(input_str) {
        Ok(v) => v,
        Err(e) => return Err(format!("Invalid input JSON: {}", e)),
    };

    let n = input.articles.len();
    if n == 0 {
        let empty_output = AnalysisOutput {
            topics: Vec::new(),
            weeks: Vec::new(),
            unassigned: Vec::new(),
        };
        return serde_json::to_string(&empty_output).map_err(|e| e.to_string());
    }

    let tokenized: Vec<Vec<String>> = input
        .articles
        .iter()
        .map(|art| {
            let combined = match &art.summary {
                Some(s) if !s.trim().is_empty() => format!("{} {}", art.title, s),
                _ => art.title.clone(),
            };
            let lang = detect_language(&combined);
            tokenize(&combined, lang)
        })
        .collect();

    let (vectors, vocab) = tfidf(&tokenized);
    let req_k = input.k.unwrap_or(5).clamp(1, n);
    let seed = input.seed.unwrap_or(1);
    let km = kmeans(&vectors, req_k, seed);

    let actual_k = km.centroids.len();
    let mut cluster_members: Vec<Vec<usize>> = vec![Vec::new(); actual_k];
    let mut unassigned: Vec<String> = Vec::new();

    for (i, assign_opt) in km.assignments.iter().enumerate() {
        match assign_opt {
            Some(c_idx) if *c_idx < actual_k => {
                cluster_members[*c_idx].push(i);
            }
            _ => {
                unassigned.push(input.articles[i].id.clone());
            }
        }
    }

    let mut ranked_clusters: Vec<(usize, Vec<usize>)> = cluster_members
        .into_iter()
        .enumerate()
        .filter(|(_, members)| !members.is_empty())
        .collect();

    ranked_clusters.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(&b.0)));

    let mut renumbered_labels: Vec<Option<usize>> = vec![None; n];
    let mut topics: Vec<TopicOutput> = Vec::with_capacity(ranked_clusters.len());

    for (new_id, &(old_c_idx, ref member_indices)) in ranked_clusters.iter().enumerate() {
        for &doc_idx in member_indices {
            renumbered_labels[doc_idx] = Some(new_id);
        }

        let old_centroid = &km.centroids[old_c_idx];
        let mut centroid_terms: BTreeMap<String, f32> = BTreeMap::new();
        for (&t_idx, &val) in old_centroid {
            if let Some(term_str) = vocab.get(t_idx as usize) {
                centroid_terms.insert(term_str.clone(), val);
            }
        }

        let cluster_struct = Cluster {
            centroid: centroid_terms,
            articles: member_indices
                .iter()
                .map(|&idx| input.articles[idx].id.clone())
                .collect(),
        };

        let topic_keywords = keywords(&cluster_struct);
        let share = member_indices.len() as f32 / n as f32;

        topics.push(TopicOutput {
            id: new_id,
            keywords: topic_keywords,
            articles: cluster_struct.articles,
            share,
        });
    }

    let dates: Vec<Option<String>> = input.articles.iter().map(|a| a.published.clone()).collect();
    let weeks = weekly_share_with_k(&dates, &renumbered_labels, topics.len());

    let output = AnalysisOutput {
        topics,
        weeks,
        unassigned,
    };

    serde_json::to_string(&output).map_err(|e| e.to_string())
}

#[cfg_attr(feature = "browser", wasm_bindgen)]
pub fn analyse(input: &str) -> String {
    match run_analyse(input) {
        Ok(json) => json,
        Err(err) => serde_json::to_string(&serde_json::json!({ "error": err }))
            .unwrap_or_else(|_| "{\"error\":\"Serialization error\"}".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_input() {
        let output_str = analyse(r#"{"articles": []}"#);
        let out: AnalysisOutput = serde_json::from_str(&output_str).expect("parse empty output");
        assert!(out.topics.is_empty());
        assert!(out.weeks.is_empty());
        assert!(out.unassigned.is_empty());
    }

    #[test]
    fn test_one_article() {
        let input = r#"{
            "articles": [
                {
                    "id": "urn:article:1",
                    "title": "Helsinki opens brand new primary school in Pasila",
                    "summary": "The school serves hundreds of local students.",
                    "published": "2026-10-20T10:00:00Z"
                }
            ],
            "k": 3
        }"#;
        let out_str = analyse(input);
        let out: AnalysisOutput = serde_json::from_str(&out_str).expect("parse output");
        assert_eq!(out.topics.len(), 1);
        assert_eq!(out.topics[0].id, 0);
        assert_eq!(out.topics[0].articles, vec!["urn:article:1"]);
        assert_eq!(out.topics[0].share, 1.0);
        assert!(!out.topics[0].keywords.is_empty());
        assert_eq!(out.weeks.len(), 1);
        assert_eq!(out.weeks[0].shares, vec![1.0]);
        assert!(out.unassigned.is_empty());
    }

    #[test]
    fn test_all_articles_identical() {
        let input = r#"{
            "articles": [
                { "id": "1", "title": "Bicycle path construction starts", "published": "2026-10-01T08:00:00Z" },
                { "id": "2", "title": "Bicycle path construction starts", "published": "2026-10-01T09:00:00Z" },
                { "id": "3", "title": "Bicycle path construction starts", "published": "2026-10-08T09:00:00Z" }
            ],
            "k": 3,
            "seed": 42
        }"#;
        let out_str = analyse(input);
        let out: AnalysisOutput = serde_json::from_str(&out_str).expect("parse output");
        assert_eq!(out.topics.len(), 1);
        assert_eq!(out.topics[0].articles.len(), 3);
        assert_eq!(out.topics[0].share, 1.0);
        for w in &out.weeks {
            assert_eq!(w.shares, vec![1.0]);
        }
    }

    #[test]
    fn test_missing_summary_and_date() {
        let input = r#"{
            "articles": [
                { "id": "a1", "title": "Culture festival organized at Stoa center" }
            ]
        }"#;
        let out_str = analyse(input);
        let out: AnalysisOutput = serde_json::from_str(&out_str).expect("parse output");
        assert_eq!(out.topics.len(), 1);
        assert_eq!(out.topics[0].articles, vec!["a1"]);
        assert!(out.weeks.is_empty());
    }

    #[test]
    fn an_english_headline_keeps_no_stop_word_and_no_word_that_only_names_the_city() {
        // A real headline of the feed: "on" and "and" are stop words of either language, and the
        // guess must not take it for Finnish (it did, and "and" became a topic keyword).
        let text = "Helsinki opens new library on Saturday and the city celebrates";
        assert_eq!(detect_language(text), "en");
        let tokens = tokenize(text, detect_language(text));
        for word in ["and", "the", "on", "helsinki", "city"] {
            assert!(!tokens.iter().any(|t| t == word), "{word} in {tokens:?}");
        }
        assert!(tokens.iter().any(|t| t.starts_with("librar")), "{tokens:?}");
        let fi = "Helsingin kaupunki avaa uuden kirjaston ja se on auki lauantaina";
        assert_eq!(detect_language(fi), "fi");
        assert!(!tokenize(fi, "fi")
            .iter()
            .any(|t| t.starts_with("helsing") || t.starts_with("kaupun")));
    }

    #[test]
    fn test_finnish_and_english_stopwords_and_stems() {
        let fi_text = "tämä on testi ja koulussa opiskellaan";
        assert_eq!(detect_language(fi_text), "fi");
        let fi_tokens = tokenize(fi_text, "fi");
        assert!(!fi_tokens.contains(&"tämä".to_string()));
        assert!(!fi_tokens.contains(&"on".to_string()));
        assert!(!fi_tokens.contains(&"ja".to_string()));
        assert!(fi_tokens.contains(&"koulu".to_string()));

        let en_text = "the schools are building playing fields quickly";
        assert_eq!(detect_language(en_text), "en");
        let en_tokens = tokenize(en_text, "en");
        assert!(!en_tokens.contains(&"the".to_string()));
        assert!(!en_tokens.contains(&"are".to_string()));
        assert!(en_tokens.contains(&"school".to_string()));
        assert!(en_tokens.contains(&"build".to_string()));
        assert!(en_tokens.contains(&"play".to_string()));
        assert!(en_tokens.contains(&"quick".to_string()));
    }

    #[test]
    fn test_k_larger_than_documents() {
        let input = r#"{
            "articles": [
                { "id": "1", "title": "Harbor traffic increases in Vuosaari", "published": "2026-10-01T10:00:00Z" },
                { "id": "2", "title": "Library opens new quiet spaces", "published": "2026-10-02T10:00:00Z" }
            ],
            "k": 10,
            "seed": 1
        }"#;
        let out_str = analyse(input);
        let out: AnalysisOutput = serde_json::from_str(&out_str).expect("parse output");
        assert!(out.topics.len() <= 2);
    }

    #[test]
    fn test_iso_week_year_boundaries() {
        assert_eq!(iso_week("2020-12-31"), Some("2020-W53".to_string()));
        assert_eq!(iso_week("2021-01-03"), Some("2020-W53".to_string()));
        assert_eq!(iso_week("2024-12-30"), Some("2025-W01".to_string()));
    }

    #[test]
    fn test_bad_json_answers_error_object() {
        let out_str = analyse("not a json string");
        assert!(out_str.contains("\"error\""));
        let val: serde_json::Value = serde_json::from_str(&out_str).expect("valid json error");
        assert!(val.get("error").is_some());
    }

    #[test]
    fn test_determinism() {
        let input = r#"{
            "articles": [
                { "id": "1", "title": "Climate roadmap updated for carbon neutrality", "published": "2026-09-01T10:00:00Z" },
                { "id": "2", "title": "Tramline extension opens to Kalasatama", "published": "2026-09-08T10:00:00Z" },
                { "id": "3", "title": "Solar panels installed on city building roofs", "published": "2026-09-15T10:00:00Z" },
                { "id": "4", "title": "Public transport ticket prices frozen for winter", "published": "2026-09-22T10:00:00Z" }
            ],
            "k": 2,
            "seed": 123
        }"#;
        let out1 = analyse(input);
        let out2 = analyse(input);
        assert_eq!(out1, out2);
    }

    // T-3401: what a feed may send that the analysis still reads.
    #[test]
    fn stems_only_what_leaves_a_stem_and_skips_numbers() {
        assert_eq!(stem_english("bus"), "bus");
        assert_eq!(stem_english("buses"), "bus");
        assert_eq!(stem_finnish("kissa"), "kiss");
        assert_eq!(stem_finnish("tie"), "tie");
        assert_eq!(
            tokenize("2024 budget 12345", "en"),
            vec!["budget".to_string()]
        );
    }

    #[test]
    fn weighs_nothing_for_no_document_and_only_shared_words_from_ten_on() {
        assert_eq!(tfidf(&[]), (Vec::new(), Vec::new()));
        let mut docs: Vec<Vec<String>> = (0..10)
            .map(|_| vec!["tram".to_string(), "line".to_string()])
            .collect();
        docs.push(vec!["unique".to_string()]);
        let (vectors, vocab) = tfidf(&docs);
        assert!(!vocab.contains(&"unique".to_string()));
        // A document whose every word is too rare to weigh has no vector, and so no topic.
        assert!(vectors[10].is_empty());
    }

    #[test]
    fn clusters_nothing_from_nothing_and_leaves_an_empty_vector_unassigned() {
        let none = kmeans(&[], 3, 1);
        assert!(none.assignments.is_empty() && none.centroids.is_empty());
        let empty = kmeans(&[BTreeMap::new(), BTreeMap::new()], 2, 1);
        assert_eq!(empty.assignments, vec![None, None]);
        assert!(empty.centroids.is_empty());
        let mut one = BTreeMap::new();
        one.insert(0u32, 1.0f32);
        let mixed = kmeans(&[one.clone(), BTreeMap::new(), one], 2, 1);
        assert_eq!(mixed.assignments[1], None);
        assert_eq!(
            cosine_similarity(&BTreeMap::new(), &mixed.centroids[0]),
            0.0
        );
    }

    #[test]
    fn reads_only_a_real_date_into_a_week() {
        for wrong in [
            "2026-10",
            "2026-13-01",
            "2026-02-30",
            "2026-00-10",
            "abcd-10-10",
        ] {
            assert_eq!(iso_week(wrong), None, "{wrong}");
        }
        // A Friday in March: its Thursday is in February.
        assert_eq!(iso_week("2030-03-01"), Some("2030-W09".to_string()));
        assert_eq!(days_in_month(2030, 13), 30);
        assert_eq!(monday_of_iso_week("2030"), None);
        assert_eq!(monday_of_iso_week("2030-X09"), None);
    }

    #[test]
    fn shares_a_week_with_no_article_as_zero_and_ignores_a_topic_past_k() {
        assert!(weekly_share_with_k(&[], &[], 2).is_empty());
        assert!(weekly_share_with_k(&[Some("2030-03-04".to_string())], &[Some(0)], 0).is_empty());
        let dates = vec![
            Some("2030-03-04".to_string()),
            Some("2030-03-18".to_string()),
            Some("2030-03-18".to_string()),
        ];
        let weeks = weekly_share_with_k(&dates, &[Some(0), Some(1), Some(7)], 2);
        assert_eq!(
            weeks.iter().map(|w| w.week.as_str()).collect::<Vec<_>>(),
            vec!["2030-W10", "2030-W11", "2030-W12"]
        );
        assert_eq!(weeks[1].shares, vec![0.0, 0.0]);
        assert_eq!(weeks[2].shares, vec![0.0, 1.0]);
    }

    #[test]
    fn leaves_an_article_of_only_stop_words_unassigned() {
        let input = r#"{
            "articles": [
                { "id": "words", "title": "Tram line to Kalasatama opens", "published": "2026-09-01T10:00:00Z" },
                { "id": "stop", "title": "the and of", "published": "2026-09-01T10:00:00Z" }
            ],
            "k": 2
        }"#;
        let out: AnalysisOutput = serde_json::from_str(&analyse(input)).expect("parse output");
        assert_eq!(out.unassigned, vec!["stop"]);
    }
}
