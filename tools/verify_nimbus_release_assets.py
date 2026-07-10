#!/usr/bin/env python3
"""Verify the exact Nimbus rusty_v8 binary release asset contract."""

from __future__ import annotations

import argparse
import hashlib
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
    args = parser.parse_args()

    directory: Path = args.directory
    assets = expected_assets()
    failures: list[str] = []

    for name in assets:
        path = directory / name
        if not path.is_file() or path.stat().st_size == 0:
            failures.append(f"missing or empty asset: {name}")
            continue
        digest = sha256(path)
        sidecar = directory / f"{name}.sha256"
        expected_line = f"{digest}  {name}\n"
        if args.write_checksums:
            sidecar.write_text(expected_line, encoding="utf-8")
        if not sidecar.is_file():
            failures.append(f"missing checksum: {sidecar.name}")
        elif sidecar.read_text(encoding="utf-8") != expected_line:
            failures.append(f"checksum mismatch: {sidecar.name}")

    expected_names = set(assets) | {f"{name}.sha256" for name in assets}
    actual_names = {path.name for path in directory.iterdir() if path.is_file()}
    unexpected = sorted(actual_names - expected_names)
    missing = sorted(expected_names - actual_names)
    failures.extend(f"unexpected release file: {name}" for name in unexpected)
    failures.extend(f"missing release file: {name}" for name in missing)

    if failures:
        for failure in failures:
            print(f"ERROR: {failure}")
        return 1

    print(f"OK: verified {len(assets)} assets and {len(assets)} checksums")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
