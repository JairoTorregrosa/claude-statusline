#!/usr/bin/env python3
"""AGM gate: enforce the provenance declaration on a pull request.

A pull request declares what produced it: the models, the harness, the
tokens, the cost. This gate checks that each line agm.json asks for is
present and carries a value. It cannot verify the values, and does not
try — the declaration is the contributor's word, on the record.

Inputs (environment):
  BODY                 pull-request body
  GITHUB_STEP_SUMMARY  summary file path (optional)
"""

import json
import os
import re
import sys

# `- Model: Opus 5`, `* model: none`, `Model:Opus 5` all match.
LINE = r"^[ \t]*[-*+]?[ \t]*{key}[ \t]*:[ \t]*(?P<value>.*?)[ \t]*$"

# An unedited template placeholder is not a declaration.
PLACEHOLDER = re.compile(r"^(<!--.*|<.*>|\.\.\.|TODO|N/?A)$", re.IGNORECASE)

# The template explains each field inside an HTML comment. Those lines look
# exactly like declarations, so a body that keeps the comment and deletes the
# real rows would pass with the template's own prose. Comments are not
# declarations: drop them before matching, unterminated ones included.
COMMENT = re.compile(r"<!--.*?(?:-->|\Z)", re.DOTALL)

# Same reasoning for a fenced block: the docs show the four lines as an
# example, and an example pasted into a summary is an illustration, not a
# declaration. The template asks for a plain list for exactly this reason.
FENCE = re.compile(
    # A list marker may precede the opener: `- ```` opens a fence nested in
    # a list item, and the lines under it are still illustration.
    r"^[ \t]*(?:[-*+][ \t]+)?(?P<mark>`{3,}|~{3,}).*?(?:^[ \t]*(?P=mark)[ \t]*$|\Z)",
    re.DOTALL | re.MULTILINE,
)


def declared(body, key):
    """Return the value declared for `key`, or None when it is missing.

    Every occurrence is considered, not the first: the template ships the
    four rows empty, and a contributor who fills them in further down the
    body rather than in place has still declared.
    """
    pattern = LINE.format(key=re.escape(key))
    for m in re.finditer(pattern, body, re.IGNORECASE | re.MULTILINE):
        value = m.group("value").strip()
        if value and not PLACEHOLDER.match(value):
            return value
    return None


def main():
    manifest = json.load(open("agm.json", encoding="utf-8"))
    declaration = manifest["declaration"]
    # GitHub sends the body with CRLF endings; `$` would keep the \r.
    body = (os.environ.get("BODY") or "").replace("\r\n", "\n")
    body = FENCE.sub("", COMMENT.sub("", body))

    found, missing = {}, []
    for field in declaration["fields"]:
        key = field["key"]
        value = declared(body, key)
        if value is None:
            missing.append(field)
        else:
            found[key] = value

    lines = ["# AGM provenance", ""]
    if found:
        lines += ["| Field | Declared |", "|---|---|"]
        lines += [f"| {k} | {v} |" for k, v in found.items()]
        lines.append("")

    if missing:
        lines.append("## Not declared")
        lines.append("")
        for field in missing:
            lines.append(f"- `{field['key']}:` — {field['means']}")
        lines += ["", declaration["no_agent"], "", declaration["unknown"]]
    else:
        lines.append("Provenance declared. Maintainer review remains.")

    report = "\n".join(lines) + "\n"
    print(report)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as fh:
            fh.write(report)

    sys.exit(1 if missing else 0)


if __name__ == "__main__":
    main()
