#!/usr/bin/env python3
"""Exercise the public session as a fresh, independent Git dependency."""

import argparse
import json
from pathlib import Path
import re
import shutil
import subprocess
import tempfile

from check_downstream_core import PORTABLE_DEPENDENCIES


RUNTIME_DEPENDENCIES = PORTABLE_DEPENDENCIES | {
    "rara-runtime", "cfg-if", "getrandom", "libc", "log", "pin-project-lite",
    "tokio", "tokio-macros", "uuid", "r-efi",
}


def check_graph(metadata, revision):
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    runtime, = (package for package in packages.values() if package["name"] == "rara-runtime")
    source = runtime["source"] or ""
    if not source.startswith("git+") or not source.endswith("#" + revision):
        raise RuntimeError(f"runtime is not from the requested Git revision: {source}")
    if metadata["workspace_members"] != [metadata["resolve"]["root"]]:
        raise RuntimeError("downstream fixture inherited another workspace")
    pending = [runtime["id"]]
    visited = set()
    while pending:
        package_id = pending.pop()
        if package_id in visited:
            continue
        visited.add(package_id)
        package = packages[package_id]
        if package["name"] not in RUNTIME_DEPENDENCIES:
            raise RuntimeError(f"unexpected runtime dependency: {package['name']}")
        if package["name"].startswith("rara-") and package["source"] != source:
            raise RuntimeError(f"mixed project dependency sources: {package['name']}")
        pending.extend(
            dependency["pkg"] for dependency in nodes[package_id]["deps"]
            if any(kind["kind"] != "dev" for kind in dependency["dep_kinds"])
        )
    print("Runtime dependency closure: " + ", ".join(sorted(
        packages[package_id]["name"] for package_id in visited
    )), flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rev", required=True, help="Full remote Git commit SHA")
    parser.add_argument("--repository", default="https://github.com/linkerdog/rara.git")
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9a-f]{40}", args.rev):
        parser.error("--rev must be a full lowercase Git commit SHA")
    if not args.repository.startswith("https://"):
        parser.error("--repository must be a remote HTTPS Git URL")
    source = Path(__file__).resolve().parents[1] / "crates/rara-runtime/tests/host.rs"
    host = next(line.removeprefix("host: ") for line in subprocess.check_output(
        ["rustc", "-vV"], text=True,
    ).splitlines() if line.startswith("host: "))
    with tempfile.TemporaryDirectory(prefix="rara-downstream-runtime-") as directory:
        project = Path(directory)
        if project.resolve().is_relative_to(source.parents[3]):
            raise RuntimeError("downstream project must be outside the repository")
        (project / "tests").mkdir()
        shutil.copyfile(source, project / "tests/host.rs")
        (project / "Cargo.toml").write_text(
            '[package]\nname = "downstream-runtime-check"\nversion = "0.0.0"\n'
            'edition = "2024"\npublish = false\n\n[workspace]\n\n[dependencies]\n'
            f'rara-runtime = {{ git = {json.dumps(args.repository)}, rev = "{args.rev}" }}\n'
            '\n[dev-dependencies]\nanyhow = "1"\nasync-trait = "0.1"\nserde_json = "1"\n'
            'tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync", "time"] }\n',
            encoding="utf-8",
        )
        metadata = json.loads(subprocess.check_output([
            "cargo", "metadata", "--format-version", "1", "--filter-platform", host,
        ], cwd=project, text=True))
        check_graph(metadata, args.rev)
        subprocess.run(["cargo", "test", "--locked", "--target", host], cwd=project, check=True)


if __name__ == "__main__":
    main()
