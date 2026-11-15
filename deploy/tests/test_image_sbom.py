"""OPS-41: every image this repository builds ships with a CycloneDX SBOM attested to its digest.

The same rule the platform, the Portal and the conformance suite check in
`scripts/ci/check-workflow-pins.py`: an image a workflow signs by digest also has
`cosign attest --type cyclonedx` against that digest, so a signed image never ships without its
bill of materials. Line-based, like that checker, because the rule is decidable from the text.
"""

import re
from pathlib import Path

WORKFLOWS = Path(__file__).resolve().parent.parent / ".github/workflows"
EXPRESSION = re.compile(r"\$\{\{\s*(.*?)\s*\}\}")


def pushed_image(line):
    tight = EXPRESSION.sub(lambda m: "${{" + m.group(1).replace(" ", "") + "}}", line)
    refs = [t.strip("\"'") for t in tight.split() if "@${{" in t and "outputs.digest" in t]
    return refs[-1] if refs else None


def unattested(text):
    signed, attested = {}, set()
    for number, line in enumerate(text.splitlines(), 1):
        code = line.split("#", 1)[0]
        if "cosign sign " in code and (image := pushed_image(code)):
            signed.setdefault(image, number)
        if "cosign attest " in code and "--type cyclonedx" in code and (image := pushed_image(code)):
            attested.add(image)
    return [(number, image) for image, number in signed.items() if image not in attested]


def test_every_image_signed_by_digest_carries_an_attested_cyclonedx_sbom():
    """OPS-41: checked on every workflow; at least one of them builds an image."""
    signing = [p for p in sorted(WORKFLOWS.glob("*.yml")) if "outputs.digest" in p.read_text() and "cosign sign " in p.read_text()]
    assert signing, "no workflow here builds and signs an image; the check would prove nothing"
    problems = [f"{p.name}:{n}: {image}" for p in signing for n, image in unattested(p.read_text())]
    assert not problems, f"signed without a CycloneDX SBOM attested to the digest (OPS-41): {problems}"


def test_an_image_signed_without_its_sbom_is_named():
    """OPS-41: the rule the test above applies turns red when the attest step goes."""
    signed_only = (
        "      - run: cosign sign --yes ghcr.io/${{ github.repository_owner }}/ckan@${{ steps.push.outputs.digest }}\n"
        "      - run: cosign attest --yes --type slsaprovenance ghcr.io/${{ github.repository_owner }}/ckan@${{ steps.push.outputs.digest }}\n"
    )
    assert unattested(signed_only) == [(1, "ghcr.io/${{github.repository_owner}}/ckan@${{steps.push.outputs.digest}}")]
    with_sbom = signed_only + (
        "      - run: cosign attest --yes --type cyclonedx --predicate sbom.cdx.json "
        "ghcr.io/${{ github.repository_owner }}/ckan@${{ steps.push.outputs.digest }}\n"
    )
    assert unattested(with_sbom) == []
