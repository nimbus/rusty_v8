#!/usr/bin/env python3
"""Package one built rusty_v8 configuration using the release filename ABI."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import shutil
from pathlib import Path

from nimbus_release_manifest import asset_names, validate_configuration


def write_sha256_sidecar(path: Path) -> None:
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for chunk in iter(lambda: file.read(1024 * 1024), b""):
            digest.update(chunk)
    path.with_name(f"{path.name}.sha256").write_text(
        f"{digest.hexdigest()}  {path.name}\n", encoding="utf-8"
    )


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True)
    parser.add_argument(
        "--features-suffix",
        choices=("", "simdutf", "ptrcomp", "ptrcomp_simdutf"),
        default="",
    )
    parser.add_argument("--target-dir", type=Path, default=Path("target"))
    parser.add_argument("--output-dir", type=Path, default=Path("release-assets"))
    args = parser.parse_args()

    try:
        validate_configuration(args.target, args.features_suffix)
    except ValueError as error:
        parser.error(str(error))

    windows = "windows" in args.target
    library_name = "rusty_v8.lib" if windows else "librusty_v8.a"
    library = (
        args.target_dir
        / args.target
        / "release"
        / "gn_out"
        / "obj"
        / library_name
    )
    binding = (
        args.target_dir
        / args.target
        / "release"
        / "gn_out"
        / "src_binding.rs"
    )
    if not library.is_file() or not binding.is_file():
        raise SystemExit(f"missing build outputs: {library} or {binding}")

    args.output_dir.mkdir(parents=True, exist_ok=True)
    archive_name, binding_name = asset_names(args.target, args.features_suffix)

    archive_output = args.output_dir / archive_name
    binding_output = args.output_dir / binding_name
    with library.open("rb") as source, archive_output.open("wb") as destination:
        with gzip.GzipFile(fileobj=destination, mode="wb", mtime=0) as compressed:
            shutil.copyfileobj(source, compressed)
    shutil.copyfile(binding, binding_output)
    write_sha256_sidecar(archive_output)
    write_sha256_sidecar(binding_output)
    print(f"packaged {archive_name}, {binding_name}, and build-time checksums")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
