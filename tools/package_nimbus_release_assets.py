#!/usr/bin/env python3
"""Package one built rusty_v8 configuration using the release filename ABI."""

from __future__ import annotations

import argparse
import gzip
import shutil
from pathlib import Path

from nimbus_release_manifest import asset_names, validate_configuration


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True)
    parser.add_argument(
        "--features-suffix",
        choices=("", "simdutf", "ptrcomp_simdutf"),
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

    with library.open("rb") as source, (args.output_dir / archive_name).open(
        "wb"
    ) as destination:
        with gzip.GzipFile(fileobj=destination, mode="wb", mtime=0) as compressed:
            shutil.copyfileobj(source, compressed)
    shutil.copyfile(binding, args.output_dir / binding_name)
    print(f"packaged {archive_name} and {binding_name}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
