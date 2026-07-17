#!/usr/bin/env python3
"""Verify the exact Nimbus rusty_v8 binary release asset contract."""

from __future__ import annotations

import argparse
import hashlib
import shutil
from collections import defaultdict
from pathlib import Path

from nimbus_release_manifest import expected_assets


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for chunk in iter(lambda: file.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def files_by_name(directory: Path) -> dict[str, list[Path]]:
    result: dict[str, list[Path]] = defaultdict(list)
    for path in directory.rglob("*"):
        if path.is_file():
            result[path.name].append(path)
    return dict(result)


def verify_release_tree(
    directory: Path, *, write_checksums: bool = False
) -> tuple[list[str], dict[str, list[Path]]]:
    assets = expected_assets()
    failures: list[str] = []
    paths_by_name = files_by_name(directory)

    for name, paths in sorted(paths_by_name.items()):
        if len(paths) > 1:
            rendered = ", ".join(str(path.relative_to(directory)) for path in paths)
            failures.append(f"duplicate release file: {name}: {rendered}")

    for name in assets:
        paths = paths_by_name.get(name, [])
        if len(paths) != 1 or paths[0].stat().st_size == 0:
            failures.append(f"missing or empty asset: {name}")
            continue
        path = paths[0]
        digest = sha256(path)
        sidecar = path.with_name(f"{name}.sha256")
        expected_line = f"{digest}  {name}\n"
        if write_checksums:
            sidecar.write_text(expected_line, encoding="utf-8")
        if not sidecar.is_file():
            failures.append(f"missing checksum: {sidecar.name}")
        elif sidecar.read_text(encoding="utf-8") != expected_line:
            failures.append(f"checksum mismatch: {sidecar.name}")

    expected_names = set(assets) | {f"{name}.sha256" for name in assets}
    paths_by_name = files_by_name(directory)
    for name, paths in sorted(paths_by_name.items()):
        if len(paths) > 1 and not any(
            failure.startswith(f"duplicate release file: {name}:")
            for failure in failures
        ):
            rendered = ", ".join(str(path.relative_to(directory)) for path in paths)
            failures.append(f"duplicate release file: {name}: {rendered}")

    actual_names = set(paths_by_name)
    failures.extend(
        f"unexpected release file: {name}"
        for name in sorted(actual_names - expected_names)
    )
    failures.extend(
        f"missing release file: {name}"
        for name in sorted(expected_names - actual_names)
    )
    return failures, paths_by_name


def flatten_release_tree(
    paths_by_name: dict[str, list[Path]],
    expected_names: set[str],
    output: Path,
) -> list[str]:
    output.mkdir(parents=True, exist_ok=True)
    if any(output.iterdir()):
        return [f"flatten output must be empty: {output}"]
    for name in sorted(expected_names):
        source = paths_by_name[name][0]
        shutil.copyfile(source, output / name)
    return []


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("directory", type=Path)
    parser.add_argument(
        "--write-checksums",
        action="store_true",
        help="write deterministic .sha256 sidecars before verification",
    )
    parser.add_argument(
        "--flatten-to",
        type=Path,
        help="copy a verified recursive artifact tree into an empty flat directory",
    )
    args = parser.parse_args()

    directory: Path = args.directory
    assets = expected_assets()
    expected_names = set(assets) | {f"{name}.sha256" for name in assets}
    failures, paths_by_name = verify_release_tree(
        directory, write_checksums=args.write_checksums
    )

    if failures:
        for failure in failures:
            print(f"ERROR: {failure}")
        return 1

    if args.flatten_to is not None:
        flatten_failures = flatten_release_tree(
            paths_by_name, expected_names, args.flatten_to
        )
        if flatten_failures:
            for failure in flatten_failures:
                print(f"ERROR: {failure}")
            return 1

    print(f"OK: verified {len(assets)} assets and {len(assets)} checksums")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
