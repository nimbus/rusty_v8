#!/usr/bin/env python3
"""Single source of truth for Nimbus rusty_v8 release configurations."""

from __future__ import annotations

import argparse
import json
from dataclasses import asdict, dataclass


@dataclass(frozen=True)
class TargetConfig:
    os: str
    target: str
    pointer_compression: bool
    build_only: bool = False

    @property
    def feature_suffixes(self) -> tuple[str, ...]:
        suffixes = ("", "simdutf")
        if self.pointer_compression:
            return (*suffixes, "ptrcomp_simdutf")
        return suffixes


TARGET_CONFIGS = (
    TargetConfig("macos-15", "aarch64-apple-darwin", True),
    TargetConfig("ubuntu-22.04", "x86_64-unknown-linux-gnu", True),
    TargetConfig("ubuntu-22.04", "aarch64-unknown-linux-gnu", True),
    TargetConfig("windows-2022", "x86_64-pc-windows-msvc", False),
    TargetConfig(
        "ubuntu-22.04", "x86_64-unknown-linux-musl", False, build_only=True
    ),
    TargetConfig(
        "ubuntu-22.04", "aarch64-unknown-linux-musl", False, build_only=True
    ),
)


def target_config(target: str) -> TargetConfig:
    for config in TARGET_CONFIGS:
        if config.target == target:
            return config
    raise ValueError(f"unsupported Nimbus release target: {target}")


def validate_configuration(target: str, features_suffix: str) -> TargetConfig:
    config = target_config(target)
    if features_suffix not in config.feature_suffixes:
        rendered = features_suffix or "default"
        raise ValueError(
            f"unsupported Nimbus release configuration: {target} / {rendered}"
        )
    return config


def asset_names(target: str, features_suffix: str) -> tuple[str, str]:
    validate_configuration(target, features_suffix)
    windows = "windows" in target
    stem = "rusty_v8" if windows else "librusty_v8"
    extension = "lib" if windows else "a"
    feature_part = f"_{features_suffix}" if features_suffix else ""
    return (
        f"{stem}{feature_part}_release_{target}.{extension}.gz",
        f"src_binding{feature_part}_release_{target}.rs",
    )


def expected_assets() -> tuple[str, ...]:
    assets: list[str] = []
    for config in TARGET_CONFIGS:
        for features_suffix in config.feature_suffixes:
            assets.extend(asset_names(config.target, features_suffix))
    if len(assets) != len(set(assets)):
        raise ValueError("release manifest produces duplicate asset names")
    return tuple(assets)


def github_matrix() -> dict[str, list[dict[str, object]]]:
    return {"include": [asdict(config) for config in TARGET_CONFIGS]}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--github-matrix", action="store_true")
    parser.add_argument("--asset-count", action="store_true")
    args = parser.parse_args()
    if args.github_matrix == args.asset_count:
        parser.error("select exactly one output mode")
    if args.github_matrix:
        print(json.dumps(github_matrix(), separators=(",", ":"), sort_keys=True))
    else:
        print(len(expected_assets()))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
