#!/usr/bin/env python3
"""AGM gate: enforce agm.json on a pull request.

Computes the risk zone from the changed files, checks the PR body for the
declared zone, the required evidence sections, and the human-confirmation
box. Writes the review packet to the job summary. Exits non-zero when a
gate fails.

Inputs (environment):
  BASE            base commit sha (git diff BASE...HEAD)
  BODY            pull-request body
  CHANGED_FILES   newline-separated override for the diff (tests)
  GITHUB_STEP_SUMMARY  summary file path (optional)
"""

import fnmatch
import json
import os
import re
import subprocess
import sys

SEVERITY = ["low", "medium", "high", "critical"]


def changed_files():
    override = os.environ.get("CHANGED_FILES")
    if override is not None:
        return [f for f in override.splitlines() if f.strip()]
    base = os.environ["BASE"]
    out = subprocess.run(
        ["git", "diff", "--name-only", f"{base}...HEAD"],
        capture_output=True,
        text=True,
        check=True,
    )
    return [f for f in out.stdout.splitlines() if f.strip()]


def file_zone(path, zones):
    best = "low"
    for zone in zones:
        if any(fnmatch.fnmatch(path, pat) for pat in zone["paths"]):
            if SEVERITY.index(zone["level"]) > SEVERITY.index(best):
                best = zone["level"]
    return best


def main():
    manifest = json.load(open("agm.json", encoding="utf-8"))
    body = os.environ.get("BODY") or ""
    files = changed_files()

    per_file = {f: file_zone(f, manifest["risk_zones"]) for f in files}
    computed = max(
        per_file.values(), key=SEVERITY.index, default="low"
    )

    lines = ["# AGM review packet", "", "| File | Zone |", "|---|---|"]
    for f, z in sorted(per_file.items()):
        lines.append(f"| `{f}` | {z} |")
    lines += ["", f"**Computed zone: {computed}**", ""]

    failures = []

    if computed == "low":
        lines.append("Zone low: no evidence package required. Gate passes.")
    else:
        m = re.search(r"Risk zone:\s*(\w+)", body)
        declared = m.group(1).lower() if m else None
        if declared is None:
            failures.append("Declare the risk zone in the PR body (`Risk zone: ...`).")
        elif declared != computed:
            failures.append(
                f"Declared zone `{declared}` does not match computed zone `{computed}`."
            )

        required = manifest["evidence"][computed]
        sections = manifest["sections"]
        for key in required:
            spec = sections[key]
            if key == "confirmation":
                if spec["checkbox"] not in body:
                    failures.append(
                        "Human confirmation box is not checked "
                        "(and only a human may check it)."
                    )
            elif spec["heading"] not in body:
                failures.append(f"Missing section `{spec['heading']}`: {spec['means']}")

    lines.append("")
    if failures:
        lines.append("## Missing evidence")
        lines.append("")
        for f in failures:
            lines.append(f"- {f}")
    else:
        lines.append("All mechanical gates pass. Maintainer review remains.")

    report = "\n".join(lines) + "\n"
    print(report)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as fh:
            fh.write(report)

    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
