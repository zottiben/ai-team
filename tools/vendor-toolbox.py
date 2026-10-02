#!/usr/bin/env python3
"""Refresh the frozen catalogue from an explicit checkout and pinned git revision.

No runtime checkout or standalone toolbox installation is consulted by ai-team.
Run: python3 tools/vendor-toolbox.py /path/to/ai-toolbox
"""
import hashlib
import pathlib
import subprocess
import sys

REVISION = "90659e82f0d040315ff99cfbd765d8798eb3e085"
ROOTS = ("hooks", "skills", "mcp", "starters", "templates", "background")
DEST = pathlib.Path(__file__).resolve().parents[1] / "crates/ai-team-core/assets/toolbox"


def main():
    checkout = pathlib.Path(sys.argv[1]).resolve()
    entries = subprocess.check_output(
        ["git", "-C", str(checkout), "ls-tree", "-rz", REVISION, "--", *ROOTS]
    ).split(b"\0")
    manifest = []
    for entry in filter(None, entries):
        metadata, name = entry.decode().split("\t", 1)
        mode, kind, oid = metadata.split()
        if kind != "blob" or mode not in ("100644", "100755"):
            raise ValueError(f"unsupported catalogue entry: {entry!r}")
        data = subprocess.check_output(["git", "-C", str(checkout), "cat-file", "blob", oid])
        target = DEST / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(data)
        target.chmod(int(mode[-3:], 8))
        manifest.append(f"{mode[-3:]} {hashlib.sha256(data).hexdigest()} {name}\n")
    (DEST / "FILES").write_text("".join(sorted(manifest)))
    (DEST / "REVISION").write_text(REVISION + "\n")
    print(f"Frozen {len(manifest)} catalogue assets from {REVISION}")


if __name__ == "__main__":
    main()
