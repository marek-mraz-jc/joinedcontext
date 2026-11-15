"""A Pipeline's sources and outputs in either manifest shape (PL-52, PL-53).

v1alpha1 names one `source`, one `output` and its `targetEndpoint`; v1alpha2 names `sources` and
`outputs`, each output carrying its own `targetEndpoint`. A seed test reads both through these
two functions, so a pipeline moved to v1alpha2 is judged by the same rule and not skipped.
"""


def sources(spec: dict) -> list[dict]:
    return spec["sources"] if "sources" in spec else [spec["source"]]


def outputs(spec: dict) -> list[dict]:
    if "outputs" in spec:
        return spec["outputs"]
    return [{"targetEndpoint": spec["targetEndpoint"], **spec["output"]}]


def test_both_shapes_read_alike():
    one = {"source": {"dataSourceRef": {"name": "a"}}, "targetEndpoint": "urn:x", "output": {"type": "T", "mode": "upsert"}}
    two = {"sources": [{"dataSourceRef": {"name": "a"}}], "outputs": [{"targetEndpoint": "urn:x", "type": "T", "mode": "upsert"}]}
    assert sources(one) == sources(two)
    assert outputs(one) == outputs(two)
