//! Bloblang's `fake(function)`: the go-faker v4.3.0 generators Bento 1.22 exposes, ported from
//! github.com/go-faker/faker (MIT License, Copyright (c) 2017 Iman Tumorang). Each output has the
//! shape go-faker gives; the numbers come from Go's math/rand algorithm seeded from the OS, so no two
//! runs repeat, as in Bento.

use std::sync::{Mutex, OnceLock, PoisonError};

use jiff::Timestamp;

use crate::fake_data::*;
use crate::gorand::GoRand;
use crate::value::Value;

/// Every name Bento 1.22's `fake` accepts, in the order its function spec lists them.
pub const FAKER_FUNCTIONS: &[&str] = &[
    "latitude",
    "longitude",
    "unix_time",
    "date",
    "time_string",
    "month_name",
    "year_string",
    "day_of_week",
    "day_of_month",
    "timestamp",
    "century",
    "timezone",
    "time_period",
    "email",
    "mac_address",
    "domain_name",
    "url",
    "username",
    "ipv4",
    "ipv6",
    "password",
    "jwt",
    "word",
    "sentence",
    "paragraph",
    "cc_type",
    "cc_number",
    "currency",
    "amount_with_currency",
    "title_male",
    "title_female",
    "first_name",
    "first_name_male",
    "first_name_female",
    "last_name",
    "name",
    "gender",
    "chinese_first_name",
    "chinese_last_name",
    "chinese_name",
    "phone_number",
    "toll_free_phone_number",
    "e164_phone_number",
    "uuid_hyphenated",
    "uuid_digit",
];

/// go-faker's credit cards: (type, length, prefixes).
const CREDIT_CARDS: &[(&str, usize, &[u32])] = &[
    (
        "VISA",
        16,
        &[4539, 4556, 4916, 4532, 4929, 40240071, 4485, 4716, 4],
    ),
    ("MasterCard", 16, &[51, 52, 53, 54, 55]),
    ("American Express", 15, &[34, 37]),
    ("Discover", 16, &[6011]),
    ("JCB", 16, &[3528, 3538, 3548, 3558, 3568, 3578, 3588]),
    ("Diners Club", 14, &[36, 38, 39]),
];

struct Faker {
    rng: GoRand,
    /// go-faker keeps the first card type it draws for the life of the process (`cacheCreditCard`),
    /// so Bento's `cc_type` and `cc_number` repeat one type per process; kept for parity.
    card: Option<usize>,
}

fn faker() -> &'static Mutex<Faker> {
    static FAKER: OnceLock<Mutex<Faker>> = OnceLock::new();
    FAKER.get_or_init(|| {
        let b = *uuid::Uuid::new_v4().as_bytes();
        let seed = i64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[7], b[9]]);
        Mutex::new(Faker {
            rng: GoRand::new(seed),
            card: None,
        })
    })
}

/// Runs the go-faker function `name` (case-insensitive, as Bento lowercases it); the empty name gives
/// go-faker's default 25-letter string. An unknown name is Bento's error.
pub fn fake(name: &str) -> Result<Value, String> {
    let key = name.to_lowercase();
    // The state is an RNG and a cached card index: a panic elsewhere leaves it valid.
    let mut guard = faker().lock().unwrap_or_else(PoisonError::into_inner);
    guard
        .generate(&key)
        .ok_or_else(|| format!("invalid faker function: {name}"))
}

impl Faker {
    fn intn(&mut self, n: usize) -> usize {
        self.rng.int63n(n as i64) as usize
    }

    fn pick(&mut self, list: &[&'static str]) -> &'static str {
        list[self.intn(list.len())]
    }

    /// go-faker's `randomString` with LangENG: runes 65..122 minus 91..96, i.e. A-Z and a-y.
    fn letters(&mut self, n: usize) -> String {
        (0..n)
            .map(|_| loop {
                let r = 65 + self.intn(57) as u8;
                if !(91..=96).contains(&r) {
                    break char::from(r);
                }
            })
            .collect()
    }

    fn digits(&mut self, n: usize) -> String {
        (0..n)
            .map(|_| char::from(b'0' + self.intn(10) as u8))
            .collect()
    }

    /// Go's `rand.Perm(n)`.
    fn perm(&mut self, n: usize) -> Vec<usize> {
        let mut m = vec![0; n];
        for i in 0..n {
            let j = self.intn(i + 1);
            m[i] = m[j];
            m[j] = i;
        }
        m
    }

    /// go-faker's `RandomInt(min, max)`: the numbers min..=max in random order, joined.
    fn shuffled(&mut self, min: usize, max: usize) -> String {
        self.perm(max - min + 1)
            .into_iter()
            .map(|v| (v + min).to_string())
            .collect()
    }

    fn float32(&mut self) -> f32 {
        self.rng.int63n(1 << 24) as f32 / (1 << 24) as f32
    }

    fn float64(&mut self) -> f64 {
        self.rng.int63n(1 << 53) as f64 / (1u64 << 53) as f64
    }

    /// `rand.Int63n(time.Now().Unix())`, read in UTC.
    fn unix_time(&mut self) -> i64 {
        self.rng.int63n(Timestamp::now().as_second().max(1))
    }

    fn time(&mut self, fmt: &str) -> String {
        // `fmt` is one of the fixed formats below, each of which jiff prints for any instant
        let t = Timestamp::from_second(self.unix_time()).unwrap_or(Timestamp::UNIX_EPOCH);
        t.strftime(fmt).to_string()
    }

    fn sentence(&mut self) -> String {
        let words: Vec<&str> = (0..6).map(|_| self.pick(WORDS)).collect();
        let mut out = String::new();
        for (i, w) in words.iter().enumerate() {
            if i == 0 {
                let mut c = w.chars();
                out.extend(c.next().into_iter().flat_map(char::to_uppercase));
                out.push_str(c.as_str());
            } else {
                out.push(' ');
                out.push_str(w);
            }
        }
        out.push('.');
        out
    }

    fn domain(&mut self) -> String {
        format!("{}.{}", self.letters(7).to_lowercase(), self.pick(TLD))
    }

    fn card(&mut self) -> usize {
        match self.card {
            Some(c) => c,
            None => {
                let c = self.intn(CREDIT_CARDS.len());
                self.card = Some(c);
                c
            }
        }
    }

    fn hex_bytes(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| self.intn(256) as u8).collect()
    }

    fn generate(&mut self, name: &str) -> Option<Value> {
        let s = match name {
            "latitude" => return Some(Value::Float(f64::from(self.float32() * 180.0 - 90.0))),
            "longitude" => return Some(Value::Float(f64::from(self.float32() * 360.0 - 180.0))),
            "unix_time" => return Some(Value::Int(self.unix_time())),
            "" => self.letters(25),
            "date" => self.time("%Y-%m-%d"),
            "time_string" => self.time("%H:%M:%S"),
            "month_name" => self.time("%B"),
            "year_string" => self.time("%Y"),
            "day_of_week" => self.time("%A"),
            "day_of_month" => self.time("%e"),
            "timestamp" => self.time("%Y-%m-%d %H:%M:%S"),
            "time_period" => self.time("%p"),
            "century" => self.pick(CENTURY).to_string(),
            "timezone" => self.pick(TIMEZONES).to_string(),
            "email" => {
                let user = self.letters(7);
                let host = self.letters(7);
                format!("{user}@{host}.{}", self.pick(TLD))
            }
            "mac_address" => self
                .hex_bytes(6)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join(":"),
            "domain_name" => self.domain(),
            "url" => {
                let format = self.pick(URL_FORMATS);
                let domain = self.domain();
                let mut out = format.replacen("%s", &domain, 1);
                if out.contains("%s") {
                    out = out.replacen("%s", &self.letters(7), 1);
                }
                out
            }
            "username" => self.letters(7),
            "ipv4" => {
                let b = self.hex_bytes(4);
                std::net::Ipv4Addr::new(b[0], b[1], b[2], b[3]).to_string()
            }
            "ipv6" => {
                let mut b = [0u8; 16];
                b.copy_from_slice(&self.hex_bytes(16));
                std::net::Ipv6Addr::from(b).to_string()
            }
            "password" => self.letters(50),
            "jwt" => {
                let part = self.letters(40);
                format!("{part}.{part}.{part}")
            }
            "word" => self.pick(WORDS).to_string(),
            "sentence" => self.sentence(),
            "paragraph" => {
                let n = self.intn(10) + 1;
                (0..n)
                    .map(|_| self.sentence())
                    .collect::<Vec<_>>()
                    .join(" ")
            }
            "cc_type" => CREDIT_CARDS[self.card()].0.to_string(),
            "cc_number" => {
                let (_, len, prefixes) = CREDIT_CARDS[self.card()];
                let prefix = prefixes[self.intn(prefixes.len())].to_string();
                let rest = self.digits(len.saturating_sub(prefix.len()));
                prefix + &rest
            }
            "currency" => self.pick(CURRENCIES).to_string(),
            "amount_with_currency" => {
                let scale = 10f64.powi(self.intn(8) as i32);
                let raw = self.float64() * scale;
                let div = 10f64.powi(self.intn(2) as i32 + 1);
                let amount = (raw * div).trunc() / div;
                format!("{} {amount:.6}", self.pick(CURRENCIES))
            }
            "title_male" => self.pick(TITLES_MALE).to_string(),
            "title_female" => self.pick(TITLES_FEMALE).to_string(),
            "first_name" => {
                let n = self.intn(FIRST_NAMES_MALE.len() + FIRST_NAMES_FEMALE.len());
                FIRST_NAMES_MALE
                    .get(n)
                    .or_else(|| FIRST_NAMES_FEMALE.get(n - FIRST_NAMES_MALE.len()))
                    .map(|s| s.to_string())
                    .unwrap_or_default()
            }
            "first_name_male" => self.pick(FIRST_NAMES_MALE).to_string(),
            "first_name_female" => self.pick(FIRST_NAMES_FEMALE).to_string(),
            "last_name" => self.pick(LAST_NAMES).to_string(),
            "name" => {
                let (titles, firsts) = if self.intn(100) > 50 {
                    (TITLES_FEMALE, FIRST_NAMES_FEMALE)
                } else {
                    (TITLES_MALE, FIRST_NAMES_MALE)
                };
                let t = self.pick(titles);
                let f = self.pick(firsts);
                format!("{t} {f} {}", self.pick(LAST_NAMES))
            }
            "gender" => self.pick(GENDERS).to_string(),
            "chinese_first_name" => self.pick(CHINESE_FIRST_NAMES).to_string(),
            "chinese_last_name" => self.pick(CHINESE_LAST_NAMES).to_string(),
            "chinese_name" => {
                let f = self.pick(CHINESE_FIRST_NAMES);
                format!("{f}{}", self.pick(CHINESE_LAST_NAMES))
            }
            "phone_number" => {
                let s = self.shuffled(1, 10);
                format!("{}-{}-{}", &s[..3], &s[3..6], &s[6..10])
            }
            "toll_free_phone_number" => {
                let s = self.shuffled(1, 9);
                let start = self.pick(&["777", "888"]);
                format!("({start}) {}-{}", &s[..3], &s[3..])
            }
            "e164_phone_number" => {
                let s = self.shuffled(1, 10);
                format!("+{}{s}", self.pick(&["7", "8"]))
            }
            "uuid_hyphenated" => uuid::Uuid::new_v4().hyphenated().to_string(),
            "uuid_digit" => uuid::Uuid::new_v4().simple().to_string(),
            _ => return None,
        };
        Some(Value::String(s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use regex::Regex;

    fn s(name: &str) -> String {
        let v = fake(name);
        assert!(matches!(v, Ok(Value::String(_))), "{name}: {v:?}");
        v.map(|v| v.to_string_coerced()).unwrap_or_default()
    }

    /// The shape of each go-faker field, read from Bento 1.22's own output
    /// (`echo '{}' | bento blobl 'root = fake("<name>")'`).
    #[test]
    fn every_listed_function_gives_bentos_shape() {
        let shapes: &[(&str, &str)] = &[
            ("date", r"^\d{4}-\d{2}-\d{2}$"),
            ("time_string", r"^\d{2}:\d{2}:\d{2}$"),
            (
                "month_name",
                r"^(January|February|March|April|May|June|July|August|September|October|November|December)$",
            ),
            ("year_string", r"^\d{4}$"),
            (
                "day_of_week",
                r"^(Monday|Tuesday|Wednesday|Thursday|Friday|Saturday|Sunday)$",
            ),
            ("day_of_month", r"^[ 123]\d$"),
            ("timestamp", r"^\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}$"),
            ("century", r"^[IVX]+$"),
            ("timezone", r"^[A-Z][A-Za-z0-9_+\-]*(/[A-Za-z0-9_+\-]+)*$"),
            ("time_period", r"^(AM|PM)$"),
            (
                "email",
                r"^[A-Za-y]{7}@[A-Za-y]{7}\.(com|biz|info|net|org|ru|edu|top)$",
            ),
            ("mac_address", r"^[0-9a-f]{2}(:[0-9a-f]{2}){5}$"),
            ("domain_name", r"^[a-z]{7}\.[a-z]+$"),
            (
                "url",
                r"^https?://(www\.)?[a-z]{7}\.[a-z]+/([A-Za-y]{7}(\.[a-z]+)?)?$",
            ),
            ("username", r"^[A-Za-y]{7}$"),
            ("ipv4", r"^\d{1,3}(\.\d{1,3}){3}$"),
            ("ipv6", r"^[0-9a-f:]+$"),
            ("password", r"^[A-Za-y]{50}$"),
            ("jwt", r"^([A-Za-y]{40})\.[A-Za-y]{40}\.[A-Za-y]{40}$"),
            ("word", r"^[a-z]+$"),
            ("sentence", r"^[A-Z][a-z]*( [a-z]+){5}\.$"),
            (
                "paragraph",
                r"^[A-Z][a-z]*( [a-z]+){5}\.( [A-Z][a-z]*( [a-z]+){5}\.){0,9}$",
            ),
            (
                "cc_type",
                r"^(VISA|MasterCard|American Express|Discover|JCB|Diners Club)$",
            ),
            ("cc_number", r"^\d{14,16}$"),
            ("currency", r"^[A-Z]{3}$"),
            ("amount_with_currency", r"^[A-Z]{3} \d+\.\d{6}$"),
            ("title_male", r"^[A-Z][a-z]+\.?$"),
            ("title_female", r"^[A-Z][a-z]+\.?$"),
            // go-faker's names hold one hyphenated name, "Xiu-Mei" (fake_data.rs): 1 in ~1 500 draws.
            ("first_name", r"^[A-Z][a-z'A-Z-]+$"),
            ("first_name_male", r"^[A-Z][a-z'A-Z-]+$"),
            ("first_name_female", r"^[A-Z][a-z'A-Z-]+$"),
            ("last_name", r"^[A-Z][a-z'A-Z-]+$"),
            (
                "name",
                r"^[A-Z][a-z]+\.? [A-Z][a-z'A-Z-]+ [A-Z][a-z'A-Z-]+$",
            ),
            ("gender", r"^(Male|Female|Prefer to skip)$"),
            ("chinese_first_name", r"^\p{Han}{1,4}$"),
            ("chinese_last_name", r"^\p{Han}{1,4}$"),
            ("chinese_name", r"^\p{Han}{2,8}$"),
            ("phone_number", r"^\d{3}-\d{3}-\d{4}$"),
            ("toll_free_phone_number", r"^\((777|888)\) \d{3}-\d{6}$"),
            ("e164_phone_number", r"^\+[78]\d{11}$"),
            (
                "uuid_hyphenated",
                r"^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$",
            ),
            (
                "uuid_digit",
                r"^[0-9a-f]{12}4[0-9a-f]{3}[89ab][0-9a-f]{15}$",
            ),
        ];
        let numeric = ["latitude", "longitude", "unix_time"];
        let tested: Vec<&str> = shapes.iter().map(|(n, _)| *n).chain(numeric).collect();
        for name in FAKER_FUNCTIONS {
            assert!(tested.contains(name), "{name} has no shape test");
        }
        for (name, re) in shapes {
            let re = Regex::new(re).unwrap();
            for _ in 0..50 {
                let v = s(name);
                assert!(re.is_match(&v), "{name}: {v:?} does not match {re}");
            }
        }
        for _ in 0..50 {
            let drawn = (fake("latitude"), fake("longitude"), fake("unix_time"));
            assert!(
                matches!(
                    drawn,
                    (Ok(Value::Float(_)), Ok(Value::Float(_)), Ok(Value::Int(_)))
                ),
                "{drawn:?}"
            );
            let num =
                |v: Result<Value, String>| v.ok().and_then(|v| v.as_f64()).unwrap_or(f64::NAN);
            let (lat, lon, t) = (num(drawn.0), num(drawn.1), num(drawn.2));
            assert!((-90.0..90.0).contains(&lat), "{lat}");
            assert!((-180.0..180.0).contains(&lon), "{lon}");
            assert!(
                (0.0..Timestamp::now().as_second() as f64).contains(&t),
                "{t}"
            );
        }
    }

    #[test]
    fn names_are_case_insensitive_and_unknown_ones_are_refused_as_bento_does() {
        assert!(s("EMAIL").contains('@'));
        assert_eq!(s("").len(), 25);
        assert_eq!(
            fake(" email").unwrap_err(),
            "invalid faker function:  email"
        );
        assert_eq!(
            fake("russian_first_name_male").unwrap_err(),
            "invalid faker function: russian_first_name_male"
        );
    }

    #[test]
    fn a_panic_while_the_faker_is_held_does_not_break_later_calls() {
        let poisoner = std::thread::spawn(|| {
            let _held = faker().lock();
            panic!("poison the faker");
        });
        assert!(poisoner.join().is_err());
        assert!(faker().is_poisoned());
        assert!(s("email").contains('@'));
    }

    #[test]
    fn a_fresh_faker_draws_its_card_type_and_unix_times_from_its_stream() {
        let fresh = |seed: i64| Faker {
            rng: GoRand::new(seed),
            card: None,
        };
        let cards: std::collections::HashSet<usize> =
            (0..20).map(|seed| fresh(seed).card()).collect();
        assert!(cards.len() > 2, "{cards:?}");
        let mut f = fresh(7);
        let first = f.card();
        assert!((0..5).all(|_| f.card() == first));
        let now = Timestamp::now().as_second();
        let times: std::collections::HashSet<i64> = (0..10).map(|_| f.unix_time()).collect();
        assert!(
            times.len() > 5 && times.iter().all(|t| (0..now).contains(t)),
            "{times:?}"
        );
    }

    #[test]
    fn a_name_takes_the_female_lists_exactly_when_the_draw_is_over_50() {
        let mut boundary = 0;
        for seed in 0..1000 {
            let mut probe = GoRand::new(seed);
            let draw = probe.int63n(100);
            boundary += usize::from(draw == 50);
            let title_list = if draw > 50 {
                TITLES_FEMALE
            } else {
                TITLES_MALE
            };
            let title = title_list[probe.int63n(title_list.len() as i64) as usize];
            let mut f = Faker {
                rng: GoRand::new(seed),
                card: None,
            };
            let name = f
                .generate("name")
                .map(|v| v.to_string_coerced())
                .unwrap_or_default();
            assert!(
                name.starts_with(&format!("{title} ")),
                "seed {seed} draw {draw}: {name}"
            );
        }
        // Draws of exactly 50 (male) tell `>` from `>=`.
        assert!(boundary > 0);
    }

    #[test]
    fn first_names_and_names_draw_from_both_lists() {
        let firsts: Vec<String> = (0..300).map(|_| s("first_name")).collect();
        assert!(firsts
            .iter()
            .any(|n| FIRST_NAMES_MALE.contains(&n.as_str())));
        assert!(firsts
            .iter()
            .any(|n| FIRST_NAMES_FEMALE.contains(&n.as_str())));
        let female: std::collections::HashSet<&String> = firsts
            .iter()
            .filter(|n| FIRST_NAMES_FEMALE.contains(&n.as_str()))
            .collect();
        assert!(female.len() > 5, "{female:?}");
        // go-faker picks the female title and name list when intn(100) > 50: about half the time.
        let only_female =
            |w: &str| FIRST_NAMES_FEMALE.contains(&w) && !FIRST_NAMES_MALE.contains(&w);
        let share = (0..400)
            .filter(|_| s("name").split(' ').nth(1).is_some_and(only_female))
            .count();
        assert!((100..300).contains(&share), "{share} of 400");
    }

    #[test]
    fn coordinates_and_amounts_are_uniform_fractions_scaled_into_range() {
        // A refusal or a non-number reads as NaN, which the range checks below refuse.
        let num = |name: &str| fake(name).ok().and_then(|v| v.as_f64()).unwrap_or(f64::NAN);
        let lat: Vec<f64> = (0..40).map(|_| num("latitude")).collect();
        let lon: Vec<f64> = (0..40).map(|_| num("longitude")).collect();
        assert!(lat.iter().all(|v| (-90.0..90.0).contains(v)), "{lat:?}");
        assert!(lon.iter().all(|v| (-180.0..180.0).contains(v)), "{lon:?}");
        assert!(lat.iter().any(|v| *v != lat[0]) && lon.iter().any(|v| *v != lon[0]));
        assert!(lat.iter().any(|v| *v < 0.0) && lon.iter().any(|v| *v < 0.0));
        // float64() * 10^0..7, cut to one or two decimals: below 10^7 and seldom a whole number.
        let amounts: Vec<f64> = (0..60)
            .map(|_| {
                let text = s("amount_with_currency");
                let (_, amount) = text.split_once(' ').expect("currency then amount");
                amount.parse::<f64>().expect("a number")
            })
            .collect();
        assert!(
            amounts.iter().all(|a| (0.0..1e7).contains(a)),
            "{amounts:?}"
        );
        assert!(amounts.iter().any(|a| a.fract() != 0.0), "{amounts:?}");
    }

    #[test]
    fn values_vary_and_the_card_type_sticks_as_go_fakers_cache_does() {
        let a: std::collections::HashSet<String> = (0..20).map(|_| s("email")).collect();
        assert!(a.len() > 15);
        let t = s("cc_type");
        assert!((0..10).all(|_| s("cc_type") == t));
        let prefix_len = |n: &str| {
            CREDIT_CARDS
                .iter()
                .find(|c| c.0 == t)
                .map(|c| (c.1, n.len()))
        };
        let n = s("cc_number");
        let (len, got) = prefix_len(&n).unwrap();
        assert_eq!(len, got, "{t} number {n}");
    }
}
