#!/bin/sh
# Installer for claude-statusline. Safe to re-run.
# Builds the binary, installs it to ~/.local/bin, and points
# ~/.claude/settings.json at it (with a backup of the previous value).
set -eu

say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

command -v cargo >/dev/null 2>&1 || die "Rust is required. Install it from https://rustup.rs"
command -v git >/dev/null 2>&1 || die "git is required"

say "› building (release)"
cargo build --release

BIN="$HOME/.local/bin/claude-statusline"
install -d "$HOME/.local/bin"
install -m 755 target/release/claude-statusline "$BIN"
say "› installed $BIN"

say "› verifying binary"
echo '{}' | "$BIN" >/dev/null || die "binary failed the smoke test"

SETTINGS="$HOME/.claude/settings.json"
if command -v python3 >/dev/null 2>&1; then
    python3 - "$SETTINGS" "$BIN" <<'PY'
import json, pathlib, sys

path, bin_path = pathlib.Path(sys.argv[1]), sys.argv[2]
settings = {}
if path.exists():
    try:
        settings = json.loads(path.read_text())
    except ValueError as e:
        sys.exit(f"error: {path} is not valid JSON ({e}). Fix it first; refusing to overwrite.")
    backup = path.with_suffix(".json.bak")
    backup.write_text(path.read_text())
    print(f"› backup written to {backup}")

previous = settings.get("statusLine")
if previous:
    print(f"› previous statusLine (rollback value): {json.dumps(previous)}")
settings["statusLine"] = {"type": "command", "command": bin_path}
path.parent.mkdir(parents=True, exist_ok=True)
path.write_text(json.dumps(settings, indent=2) + "\n")
print(f"› {path} updated")
PY
else
    say "python3 not found — add this to $SETTINGS yourself:"
    say "  \"statusLine\": { \"type\": \"command\", \"command\": \"$BIN\" }"
fi

say "done. The statusline appears on the next Claude Code render."
