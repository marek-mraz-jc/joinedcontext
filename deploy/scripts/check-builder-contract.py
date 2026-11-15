#!/usr/bin/env python3
"""Does a builder image run the workflow the seeded applications carry? (T-2633, AP-80, AP-82)

usage: check-builder-contract.py <lane.mjs> <build-app> <build.yml>...

Reads the commands `lane.mjs` accepts (its usage line) and the variables `build-app` requires
(its `for name in … do` loop), and fails when a workflow calls `lane.mjs <command>` the image
does not know, or runs `build-app` without setting a variable it requires. The runner image of
portal 080be67 wanted nine variables and knew only deps|functions, while the workflows called
upload and propose: every application failed in 0 s, and nothing caught it before dev.
"""

import re
import sys

import yaml


def lane_commands(lane: str) -> set[str]:
    usage = re.search(r"usage: lane\.mjs ([^\"'`]*)", lane)
    if usage is None:
        raise ValueError("lane.mjs has no usage line")
    return {part.split()[0] for part in usage.group(1).split("|") if part.strip()}


def required_variables(build_app: str) -> set[str]:
    loop = re.search(r"^for name in (.*?);\s*do", build_app.replace("\\\n", " "), re.M | re.S)
    if loop is None:
        raise ValueError("build-app has no `for name in … do` loop")
    return set(loop.group(1).split())


def problems(lane: str, build_app: str, workflow: dict, name: str) -> list[str]:
    commands, required = lane_commands(lane), required_variables(build_app)
    found = []
    for job_name, job in (workflow.get("jobs") or {}).items():
        for step in job.get("steps") or []:
            run = step.get("run") or ""
            for command in re.findall(r"lane\.mjs\s+([a-z-]+)", run):
                if command not in commands:
                    found.append(f"{name}: {job_name} calls lane.mjs {command}; the image knows {sorted(commands)}")
            if re.search(r"\bbuild-app\b", run):
                given = set(job.get("env") or {}) | set(step.get("env") or {}) | set(re.findall(r"\b([A-Z_]+)=", run))
                for variable in sorted(required - given):
                    found.append(f"{name}: {job_name} runs build-app without {variable}")
    return found


def main(argv: list[str]) -> int:
    if len(argv) < 4:
        print(__doc__.strip().splitlines()[2], file=sys.stderr)
        return 2
    lane, build_app = (open(path).read() for path in argv[1:3])
    found = []
    for path in argv[3:]:
        with open(path) as f:
            found += problems(lane, build_app, yaml.safe_load(f), path)
    for line in found:
        print(line, file=sys.stderr)
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
