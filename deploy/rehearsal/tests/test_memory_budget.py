"""Unit tests of deploy/rehearsal/memory_budget.py: pytest deploy/rehearsal/tests/test_memory_budget.py"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
import memory_budget as mb

HEADER = "ts,label,pid,VmRSS,VmHWM,RssAnon,RssFile,RssShmem,VmSwap,Threads,allocated,resident,poolHeld\n"


def run(tmp_path, samples: list[tuple[int, int, int]]) -> Path:
    """A memwatch CSV: (second, VmRSS MiB, VmSwap MiB) per sample."""
    lines = [
        f"2026-11-10T18:{s // 60:02d}:{s % 60:02d}Z,,1,{rss * 1024},0,0,0,0,{swap * 1024},1,,,\n"
        for s, rss, swap in samples
    ]
    path = tmp_path / "run.csv"
    path.write_text(HEADER + "".join(lines))
    return path


def test_a_jc_with_the_shared_embedder_is_within_budget_and_one_with_the_model_is_not(tmp_path):
    # The start window (first minute here) may exceed the bounds; only what follows counts.
    shared = [(0, 2600, 0)] + [(60 + 10 * i, 1200, 300) for i in range(20)] + [(300, 2200, 0)]
    assert mb.main([str(run(tmp_path, shared)), "--after-min", "1"]) == 0
    in_process = [(60 + 10 * i, 2181, 0) for i in range(20)]
    assert mb.main([str(run(tmp_path, in_process)), "--after-min", "1"]) == 1


def test_swap_counts_and_a_peak_alone_fails(tmp_path):
    swapped = [(10 * i, 900, 1000) for i in range(20)]
    assert mb.main([str(run(tmp_path, swapped)), "--after-min", "0"]) == 1
    spike = [(10 * i, 1000, 0) for i in range(20)] + [(300, 2500, 0)]
    assert mb.main([str(run(tmp_path, spike)), "--after-min", "0"]) == 1


def test_a_run_shorter_than_the_start_window_gives_no_verdict(tmp_path):
    assert mb.main([str(run(tmp_path, [(0, 1000, 0), (30, 1000, 0)])), "--after-min", "12"]) == 2
