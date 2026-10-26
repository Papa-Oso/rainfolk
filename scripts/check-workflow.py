"""Verify the producer snapshot and explicit workflow files, without live access."""
import json
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
WORKFLOW = {
    "AGENTS.md",
    "docs/ENGINEERING.md",
    "docs/decisions/0001-engineering-workflow.md",
    ".github/ISSUE_TEMPLATE/task.yml",
    ".github/pull_request_template.md",
    ".github/workflows/offline-producer.yml",
    "scripts/check-workflow.py",
}


def safe_path(name):
    path = PurePosixPath(name)
    if not name or path.is_absolute() or ".." in path.parts or "\\" in name or ":" in name:
        raise ValueError("Unsafe release path")
    full = ROOT / path
    if any(p.is_symlink() or (hasattr(p, "is_junction") and p.is_junction())
           for p in [full, *full.parents]):
        raise ValueError("Symlinks and junctions are not permitted")
    if not full.is_file():
        raise ValueError("Missing release file: " + name)
    return full


def main():
    manifest = json.loads((ROOT / "docs/release-files.json").read_text(encoding="utf-8"))
    names = [row["path"] for row in manifest["files"]]
    if len(names) != len(set(names)) or set(names) & WORKFLOW:
        raise ValueError("Duplicate or overlapping release entries")
    producer = set(names) | {"docs/release-files.json"}
    actual = set(subprocess.check_output(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"], cwd=ROOT
    ).decode("utf-8").split("\0")) - {""}
    if actual != producer | WORKFLOW:
        raise ValueError("Unexpected or missing tracked/workflow files")
    for name in producer | WORKFLOW:
        safe_path(name)
    patterns = [
        r"[A-Za-z]:[\\/](?:Users|Windows)[\\/]",
        r"/(?:home|Users)/[A-Za-z0-9_.-]+/",
        r"-----BEGIN (?:RSA |OPENSSH |EC )?PRIVATE KEY-----",
        r"\b(?:ghp_|github_pat_|sk-proj-)[A-Za-z0-9_-]{16,}",
    ]
    for name in WORKFLOW:
        content = safe_path(name).read_text(encoding="utf-8")
        if any(re.search(pattern, content) for pattern in patterns):
            raise ValueError("Sensitive pattern in workflow file: " + name)
    scratch = ROOT / ".temp-gev"
    if scratch.is_symlink() or (hasattr(scratch, "is_junction") and scratch.is_junction()):
        raise ValueError("Temporary directory must not be a symlink or junction")
    scratch.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="producer-check-", dir=scratch) as directory:
        stage = Path(directory)
        for name in producer:
            destination = stage / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(safe_path(name), destination)
        subprocess.run(["git", "init", "--quiet", str(stage)], check=True)
        subprocess.run(["git", "-c", "core.autocrlf=false", "add", "-f", "--", *sorted(producer)],
                       cwd=stage, check=True)
        subprocess.run([sys.executable, "-B", "scripts/check-release.py"], cwd=stage, check=True)
    print(f"Workflow files: {len(WORKFLOW)}; producer snapshot verified; no live requests")


if __name__ == "__main__":
    main()
