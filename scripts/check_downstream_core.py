#!/usr/bin/env python3
"""Check the portable contracts as a real Git dependency outside the workspace."""

import argparse
import json
from pathlib import Path
import re
import shutil
import subprocess
import tempfile


CORE_DEPENDENCIES = {
    "rara-core", "rara-observability", "anyhow", "async-trait", "serde",
    "serde_core", "serde_derive", "serde_json", "itoa", "memchr", "ryu", "zmij",
    "thiserror", "thiserror-impl", "proc-macro2", "quote", "syn", "unicode-ident",
}


def check_graph(metadata, revision):
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    core, = (package for package in packages.values() if package["name"] == "rara-core")
    if not core["source"].startswith("git+") or not core["source"].endswith("#" + revision):
        raise RuntimeError(f"core is not from the requested Git revision: {core['source']}")
    if metadata["workspace_members"] != [metadata["resolve"]["root"]]:
        raise RuntimeError("downstream fixture inherited another workspace")

    pending = [core["id"]]
    visited = set()
    while pending:
        package_id = pending.pop()
        if package_id in visited:
            continue
        visited.add(package_id)
        package = packages[package_id]
        if package["name"] not in CORE_DEPENDENCIES:
            raise RuntimeError(f"unexpected core dependency: {package['name']}")
        if package["name"].startswith("rara-") and package["source"] != core["source"]:
            raise RuntimeError(f"mixed project dependency sources: {package['name']}")
        pending.extend(
            dependency["pkg"] for dependency in nodes[package_id]["deps"]
            if any(kind["kind"] != "dev" for kind in dependency["dep_kinds"])
        )
    print("Core dependency closure: " + ", ".join(sorted(
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

    source = Path(__file__).resolve().parent / "fixtures" / "downstream_core.rs"
    # Neither the repository's Cargo config nor its lockfile participates.
    with tempfile.TemporaryDirectory(prefix="rara-downstream-core-") as directory:
        project = Path(directory)
        if project.resolve().is_relative_to(source.parents[2]):
            raise RuntimeError("temporary downstream project must be outside the repository")
        (project / "src").mkdir()
        shutil.copyfile(source, project / "src" / "lib.rs")
        (project / "Cargo.toml").write_text(
            '[package]\nname = "downstream-core-check"\nversion = "0.0.0"\n'
            'edition = "2024"\npublish = false\n\n[workspace]\n\n[dependencies]\n'
            'anyhow = "1"\nasync-trait = "0.1"\nserde_json = "1"\n'
            f'rara-core = {{ git = {json.dumps(args.repository)}, '
            f'rev = "{args.rev}" }}\n',
            encoding="utf-8",
        )
        metadata = json.loads(subprocess.check_output(
            ["cargo", "metadata", "--format-version", "1"], cwd=project, text=True,
        ))
        check_graph(metadata, args.rev)
        subprocess.run(["cargo", "test", "--locked"], cwd=project, check=True)
        subprocess.run([
            "cargo", "check", "--locked", "--tests", "--target",
            "wasm32-unknown-unknown",
        ], cwd=project, check=True)


if __name__ == "__main__":
    main()
