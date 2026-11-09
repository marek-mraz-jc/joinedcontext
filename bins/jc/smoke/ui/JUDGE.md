# UI judge for the `ui` smoke suite

The judge is a separate Claude subagent (owner 2026-10-29, KNOWLEDGE.md "UI judge"): it reads the
screenshots of a run and nothing else, and scores each page per width. The suite folds its answer in
with `--judge-file`.

## Input

`<out>/shots.json`: one row per page and width (`page`, `width`, `file` relative to `<out>`, `h1`,
`findings` the DOM checks already raised). The pictures are `<out>/shots/<persona>/<width>/*.png`,
each the page's whole height in the viewport.

`python3 -I bins/jc/smoke/ui/judge.py pack --out <out> [--batch 40]` writes `<out>/judge/batch-NN.json`
(rows of one batch) so one subagent reads at most 40 pictures.

## Prompt (one subagent per batch)

> You are the UI judge of the joinedcontext Portal and its sibling pages (dataroom /opendata, daiya
> dashboards, shako /git). Read every picture listed in `<batch file>` with the Read tool. For each
> row score 1-5: `layout` (structure, alignment, nothing cut or overlapping), `density` (neither
> cramped nor empty), `readability` (words, not ids, JSON or keys; contrast; type sizes), `states`
> (loading, empty, error and refusal states designed and saying what to do) and `phone` (at 400 px:
> usable, nothing wider than the screen, touch targets; at 1440 score how the layout uses the width).
> List findings with severity high (a person cannot use the page or is misled: raw JSON or URNs where
> words belong, an error with no way on, content cut off, a blank page), medium or low. Judge what
> the picture shows: a permission refusal that says why is a designed state, not a defect. Answer
> only JSON: `[{"page": ..., "width": ..., "scores": {...}, "findings": [{"severity": ..., "text": ...}]}]`.

`python3 -I bins/jc/smoke/ui/judge.py merge --out <out> <answer files...>` writes `<out>/judge.json`;
rerun the suite with `--judge-file <out>/judge.json`, or `judge.py fold --out <out>` to update
results.json and REPORT.md of the same run without walking again.

A page passes the judge with every score >= 4 and no high finding. A judge round is noisy: fix what
holds over two rounds or names a concrete text contradiction (KNOWLEDGE.md, T-6604).
