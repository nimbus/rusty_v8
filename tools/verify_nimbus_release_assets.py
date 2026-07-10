#!/usr/bin/env python3
"""Verify the exact Nimbus rusty_v8 binary release asset contract."""

from __future__ import annotations

import argparse
import hashlib
import shutil
from collections import defaultdict
from pathlib import Path


TARGETS = (
    "aarch64-apple-darwin",
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
)


def expected_assets() -> tuple[str, ...]:
    assets: list[str] = []
    for target in TARGETS:
        assets.extend(
            (
                f"librusty_v8_release_{target}.a.gz",
                f"librusty_v8_simdutf_release_{target}.a.gz",
                f"librusty_v8_ptrcomp_simdutf_release_{target}.a.gz",
                f"src_binding_release_{target}.rs",
                f"src_binding_simdutf_release_{target}.rs",
                f"src_binding_ptrcomp_simdutf_release_{target}.rs",
            )
        )
    assets.extend(
        (
            "rusty_v8_release_x86_64-pc-windows-msvc.lib.gz",
            "rusty_v8_simdutf_release_x86_64-pc-windows-msvc.lib.gz",
            "src_binding_release_x86_64-pc-windows-msvc.rs",
            "src_binding_simdutf_release_x86_64-pc-windows-msvc.rs",
        )
    )
    return tuple(assets)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for chunk in iter(lambda: file.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


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
    failures: list[str] = []

    def files_by_name() -> dict[str, list[Path]]:
        result: dict[str, list[Path]] = defaultdict(list)
        for path in directory.rglob("*"):
            if path.is_file():
                result[path.name].append(path)
        return dict(result)

    paths_by_name = files_by_name()
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
        if args.write_checksums:
            sidecar.write_text(expected_line, encoding="utf-8")
        if not sidecar.is_file():
            failures.append(f"missing checksum: {sidecar.name}")
        elif sidecar.read_text(encoding="utf-8") != expected_line:
            failures.append(f"checksum mismatch: {sidecar.name}")

    expected_names = set(assets) | {f"{name}.sha256" for name in assets}
    paths_by_name = files_by_name()
    actual_names = set(paths_by_name)
    unexpected = sorted(actual_names - expected_names)
    missing = sorted(expected_names - actual_names)
    failures.extend(f"unexpected release file: {name}" for name in unexpected)
    failures.extend(f"missing release file: {name}" for name in missing)

    if failures:
        for failure in failures:
            print(f"ERROR: {failure}")
        return 1

    if args.flatten_to is not None:
        output: Path = args.flatten_to
        output.mkdir(parents=True, exist_ok=True)
        existing = [path for path in output.iterdir()]
        if existing:
            print(f"ERROR: flatten output must be empty: {output}")
            return 1
        for name in sorted(expected_names):
            source = paths_by_name[name][0]
            shutil.copyfile(source, output / name)

    print(f"OK: verified {len(assets)} assets and {len(assets)} checksums")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
