"""Every image pin is a digest (CLAUDE.md: images pinned by digest and signed).

A pin that is anything else renders an image reference no registry answers, and the pod never
starts: 3aa3eea wrote a commit sha (`656219a`) where the portal digest goes, and nothing in the
fast lane noticed. The pins are read as text so a malformed value cannot hide behind YAML.
"""

import re
from pathlib import Path

COMPONENTS = Path(__file__).resolve().parent.parent / "components"
DIGEST = re.compile(r"^sha256:[0-9a-f]{64}$")
PIN = re.compile(r"^\s*digest:\s*['\"]?([^'\"\s#]*)['\"]?\s*(?:#.*)?$")


def pins():
    for path in sorted(COMPONENTS.glob("*/images.yaml")):
        for number, line in enumerate(path.read_text().splitlines(), 1):
            found = PIN.match(line)
            if found:
                yield f"{path.relative_to(COMPONENTS)}:{number}", found.group(1)


def test_every_image_pin_is_a_sha256_digest():
    found = list(pins())
    assert len(found) > 20, "the pins were not found; the glob or the pattern is wrong"
    wrong = [f"{where}: {value!r}" for where, value in found if not DIGEST.match(value)]
    assert wrong == [], "a pin that is not sha256:<64 hex>:\n" + "\n".join(wrong)
