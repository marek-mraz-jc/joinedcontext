"""The deterministic judge of the assistant's answer evals, in Python for the gate (T-7788):
vendor/joinedcontext-platform/crates/assistant/tests/answer_evals_tests.rs `judge` and its
helpers, line for line. tests/test_gate.py runs the Rust file's own judge cases against it; a
change there is made here too. One difference: a question names a list of hosts, since a data
answer cites the cluster's own pages and a page answer the city's site."""

from __future__ import annotations

from urllib.parse import urlsplit

SK_WORDS = ["sa", "sú", "alebo", "ktorý", "ktorá", "ktoré", "ktorých", "môže", "aj", "nie", "pre", "som",
            "sme", "ste", "budú", "ako", "kedy", "aby", "byť"]
CS_WORDS = ["se", "jsou", "nebo", "který", "která", "které", "kterých", "může", "také", "není", "pro",
            "jsem", "jsme", "jste", "budou", "jako", "kdy", "aby", "být"]
SLAVIC_WORDS = ["je", "v", "na", "od", "do", "to", "tento", "toto", "však", "bude"]
FI_WORDS = ["ja", "on", "ei", "että", "oli", "ovat", "tai", "kun", "myös", "mutta", "voi", "jos", "tämä",
            "joka", "jotka", "sekä", "kanssa", "mukaan", "vuonna", "olla", "milloin", "mitä", "mikä",
            "missä", "mihin", "kuinka", "paljon"]
EN_WORDS = ["the", "and", "is", "of", "to", "in", "that", "for", "are", "was", "with", "it", "this", "by",
            "from", "has", "have", "be", "not", "you", "how", "many", "what", "when", "which", "where",
            "since", "do", "does", "its", "an", "at", "as", "or", "they", "their", "will", "can", "there"]


def language(text: str) -> str | None:
    lower = text.lower()
    words, w = [], ""
    for c in lower:
        if c.isalpha():
            w += c
        elif w:
            words.append(w)
            w = ""
    if w:
        words.append(w)

    def count(lst):
        return sum(1 for x in words if x in lst)

    def letters(chars):
        return sum(1 for c in lower if c in chars)

    sk = count(SK_WORDS) + letters("ľĺŕô")
    cs = count(CS_WORDS) + letters("řůě")
    slavic = sk + cs + count(SLAVIC_WORDS) + letters("čšžýáíéúňťď")
    fi = count(FI_WORDS) + letters("äö")
    en = count(EN_WORDS)
    best = max(slavic, fi, en)
    if best == 0:
        return None
    if slavic == best:
        return "cs" if cs > sk else "sk"
    return "fi" if fi == best else "en"


def marker(inside: str) -> list[int] | None:
    out = []
    for n in inside.split(","):
        n = n.strip()
        if not n.isdigit() or not n.isascii():
            return None
        out.append(int(n))
    return out


def markers(text: str) -> set[int]:
    found, rest = set(), text
    while "[" in rest:
        rest = rest[rest.index("[") + 1:]
        if "]" in rest:
            found.update(marker(rest[:rest.index("]")]) or [])
    return found


def numbers(text: str) -> list[str]:
    plain, rest = "", text
    while "[" in rest:
        o = rest.index("[")
        plain += rest[:o]
        close = rest[o:].find("]")
        if close >= 0 and marker(rest[o + 1:o + close]) is not None:
            rest = rest[o + close + 1:]
        else:
            plain += "["
            rest = rest[o + 1:]
    plain += rest
    digit = lambda c: "0" <= c <= "9"  # noqa: E731  (Rust's is_ascii_digit)
    runs, run = [], ""
    for i, c in enumerate(plain):
        if digit(c):
            run += c
            continue
        g = plain[i + 1:i + 4]
        group = len(g) == 3 and all(digit(x) for x in g) and not (i + 4 < len(plain) and digit(plain[i + 4]))
        if run and c in (",", " ", " ", " ") and group:
            continue
        if run:
            runs.append(run)
            run = ""
    if run:
        runs.append(run)
    return [r.lstrip("0") or "0" for r in runs]


def sentences(text: str) -> list[str]:
    out = []
    for line in text.splitlines():
        start = 0
        for i, c in enumerate(line):
            if c in ".!?" and i + 2 < len(line) and line[i + 1].isspace() and line[i + 2].isupper():
                out.append(line[start:i + 1])
                start = i + 1
        out.append(line[start:])
    return [s.strip() for s in out if s.strip()]


def normal(text: str) -> str:
    return " ".join(text.split()).lower()


def on_host(url: str, host: str) -> bool:
    try:
        h = urlsplit(url).hostname
    except ValueError:
        return False
    return bool(h) and (h == host or h.endswith(f".{host}"))


def judge(question: dict, hosts: list[str] | str, answer: dict) -> list[str]:
    """Why `answer` fails `question`, empty when it passes."""
    hosts = [hosts] if isinstance(hosts, str) else hosts
    ok = lambda url: any(on_host(url, h) for h in hosts)  # noqa: E731
    why = []
    text = answer.get("text") or ""
    citations = [c for c in answer.get("citations") or [] if isinstance(c, dict)]
    lang = question.get("lang") or ""
    if not text.strip():
        return ["no answer"]
    found = language(text)
    if found != lang:
        why.append(f"answered in {found}, asked in {lang}")
    cited = {c["n"]: c for c in citations if isinstance(c.get("n"), int)}
    for n in sorted(markers(text)):
        c = cited.get(n)
        if c is None:
            why.append(f"[{n}] has no citation")
        elif isinstance(c.get("url"), str) and not ok(c["url"]):
            why.append(f"[{n}] cites {c['url']}, not {' or '.join(hosts)}")
        elif not isinstance(c.get("url"), str) and c.get("tool") is None:
            why.append(f"[{n}] cites nothing")
    if question.get("refuse") is True:
        if citations:
            why.append("answers a question the site does not cover, with citations")
        return why
    if not citations:
        why.append("no citation")
    said = normal(text)
    for group in question.get("facts") or []:
        if not any(normal(a) in said for a in group):
            why.append(f"none of {group}")
    known = set(numbers(f"{question.get('ask', '')} {question.get('evidence', '')}"))
    # Gate only (T-7788): "4 sources" over an answer citing four is a count of its own listing.
    known.add(str(len(markers(text))))

    def resolves(n):
        c = cited.get(n)
        return c is not None and isinstance(c.get("url"), str) and ok(c["url"])

    for s in sentences(text):
        if any(resolves(n) for n in markers(s)):
            continue
        for number in numbers(s):
            if number not in known:
                why.append(f"{number} is in no source")
    return why
