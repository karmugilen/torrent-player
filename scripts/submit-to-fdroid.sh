#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
python3 - <<'PY'
import base64
import json
from pathlib import Path
import re
import subprocess


def git(*args):
    return subprocess.check_output(["git", *args], text=True).strip()


def api(endpoint, method="GET", payload=None):
    args = ["glab", "api", endpoint, "--method", method]
    if payload is not None:
        args += ["--input", "-"]
    return json.loads(subprocess.check_output(
        args, input=json.dumps(payload) if payload is not None else None, text=True
    ))


content = Path("webtor.app.yml").read_text()
commit = re.search(r"^    commit: ([0-9a-f]{40})$", content, re.M)
version = re.search(r"^  - versionName: ([0-9.]+)$", content, re.M)
code = re.search(r"^    versionCode: ([0-9]+)$", content, re.M)
if not all((commit, version, code)):
    raise SystemExit("Expected one release recipe with a full commit SHA and version.")
commit, version, code = commit[1], version[1], code[1]
gradle = git("show", f"{commit}:android/app/build.gradle.kts")
if not re.search(rf'\bversionName\s*=\s*"{re.escape(version)}"', gradle) or not re.search(
    rf"\bversionCode\s*=\s*{code}\b", gradle
):
    raise SystemExit("Recipe version does not match its upstream commit.")
remote_tags = git("ls-remote", "https://github.com/karmugilen/torrent-player.git",
                  f"refs/tags/v{version}", f"refs/tags/v{version}^{{}}")
if not any(line.split()[0] == commit for line in remote_tags.splitlines()):
    raise SystemExit(f"Push the tested v{version} release tag before updating F-Droid.")

mr = api("projects/36528/merge_requests/48893")
if (mr["state"], mr["source_project_id"], mr["source_branch"]) != (
    "opened", 86445591, "add-webtor-app"
):
    raise SystemExit("The existing MR changed or is closed; review its status first.")
endpoint = "projects/86445591/repository/files/metadata%2Fwebtor.app.yml"
remote = api(endpoint + "?ref=add-webtor-app")
if base64.b64decode(remote["content"]).decode() == content:
    print("Recipe is already up to date:", mr["web_url"])
else:
    result = api("projects/86445591/repository/commits", "POST", {
        "branch": "add-webtor-app",
        "commit_message": f"Build Torrent Player {version} and native dependencies from source",
        "actions": [{
            "action": "update",
            "file_path": "metadata/webtor.app.yml",
            "content": content,
            "last_commit_id": remote["last_commit_id"],
        }],
    })
    print("Updated recipe commit:", result["id"])
    print("Existing submission:", mr["web_url"])
PY
