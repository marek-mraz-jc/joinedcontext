"""The shared YAML helpers of conftest.py (T-5550).

`load_yaml_all` swaps PyYAML's pure-Python parser for libyaml; the swap is only safe while it
gives the same objects. `seed_documents` caches the parsed seed per process; the cache is what
took test_public_assistant_seed from 278 s to seconds.
"""

from pathlib import Path

import yaml
from conftest import SEED, load_yaml, load_yaml_all, seed_documents

# Anchors and merge keys, YAML 1.1 bool-likes (KNOWLEDGE relies on bare on/no being bool),
# timestamps, an int-looking string and a duplicate key (the last one wins in both loaders).
SAMPLE = """\
base: &base
  enabled: on
  retries: 3
first:
  <<: *base
  name: one
  flag: no
  when: 2026-10-24T19:06:00Z
  day: 2026-10-24
  zip: "08"
  octal: 010
  key: a
  key: b
---
- &x {a: 1}
- *x
- null
- ~
---
"""


def test_load_yaml_all_gives_what_safe_load_all_gives():
    assert load_yaml_all(SAMPLE) == list(yaml.safe_load_all(SAMPLE))
    first = load_yaml_all(SAMPLE)[0]["first"]
    assert first["enabled"] is True and first["flag"] is False
    assert first["key"] == "b"
    assert load_yaml("a: yes\n") == yaml.safe_load("a: yes\n") == {"a": True}


def test_seed_documents_parses_the_seed_once_per_process(monkeypatch):
    reads = []
    read_text = Path.read_text

    def counting(self, *args, **kwargs):
        reads.append(self)
        return read_text(self, *args, **kwargs)

    monkeypatch.setattr(Path, "read_text", counting)
    seed_documents.cache_clear()
    first = seed_documents("helsinki")
    once = len(reads)
    assert once == len(list(SEED.rglob("*.yaml")))
    assert seed_documents("helsinki") is first
    seed_documents("bbsk")
    assert len(reads) == once, "a second call and another project read no file again"
    assert first and all(path.is_relative_to(SEED / "helsinki") for path, _ in first)
