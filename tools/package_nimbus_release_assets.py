#!/usr/bin/env python3
"""Package one built rusty_v8 configuration using the release filename ABI."""

from __future__ import annotations

import argparse
import gzip
import shutil
from pathlib import Path


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
    stem = "rusty_v8" if windows else "librusty_v8"
    extension = "lib" if windows else "a"
    feature_part = f"_{args.features_suffix}" if args.features_suffix else ""
    archive_name = f"{stem}{feature_part}_release_{args.target}.{extension}.gz"
    binding_name = f"src_binding{feature_part}_release_{args.target}.rs"

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
