"""Helsinki's Linked Events register per publisher (T-5666).

Pages through every upcoming event (start=today, 100 a page, 1 s apart) and counts per publisher the events
that carry a name, a start and a place whose position lies in Helsinki with the 1 km margin the
linked-events pipeline keeps (helsinki-pipeline-events-bento.yaml), then names each publisher from
/organization/. A publisher is one source of the inventory: `usable` with one such event or more.

    python3 seeds/city-events/linkedevents.py > linkedevents.csv
"""
import csv
import datetime as dt
import json
import sys
import time
import urllib.request
from collections import Counter

API = "https://api.hel.fi/linkedevents/v1/"
UA = "joinedcontext-platform/1.0 (+https://dev.joinedcontext.com; public events)"


def get(url: str) -> dict:
    req = urllib.request.Request(url, headers={"User-Agent": UA, "Accept": "application/json"})
    with urllib.request.urlopen(req, timeout=60) as r:
        return json.loads(r.read(16 << 20))


def pages(url: str):
    while url:
        page = get(url)
        yield from page["data"]
        url = page["meta"]["next"]
        time.sleep(1)


def main() -> None:
    seen, full = Counter(), Counter()
    for e in pages(API + "event/?start=today&page_size=100&include=location"):
        pub = e.get("publisher") or "-"
        seen[pub] += 1
        pos = ((e.get("location") or {}).get("position") or {}).get("coordinates")
        inside = bool(pos) and 24.76 <= pos[0] <= 25.29 and 59.91 <= pos[1] <= 60.31
        full[pub] += bool((e.get("name") or {}) and e.get("start_time") and inside)
    names = {o["id"]: (o.get("name") or "") for o in pages(API + "organization/?page_size=100")}
    w = csv.writer(sys.stdout)
    w.writerow(["publisher", "name", "upcoming", "complete", "read_on", "verdict", "reason"])
    for pub, n in seen.most_common():
        ok = full[pub] > 0
        w.writerow([pub, names.get(pub, ""), n, full[pub], dt.date.today().isoformat(),
                    "usable" if ok else "skip",
                    f"{full[pub]} upcoming with name, start and place in Helsinki" if ok else "no place in Helsinki"])


if __name__ == "__main__":
    main()
