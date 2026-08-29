#!/usr/bin/env python3
"""Single source of truth for Nimbus rusty_v8 release configurations."""

from __future__ import annotations

import argparse
import json
import re
import tomllib
from dataclasses import asdict, dataclass
from pathlib import Path


@dataclass(frozen=True)
class TargetConfig:
    os: str
    target: str
    pointer_compression: bool
    build_only: bool = False
    feature_suffixes: tuple[str, ...] = ("", "simdutf")


def _parse_bool(value: str) -> bool:
    if value not in {"true", "false"}:
        raise ValueError(f"invalid release-manifest boolean: {value}")
    return value == "true"


def _load_target_configs() -> tuple[TargetConfig, ...]:
    path = Path(__file__).with_name("nimbus_release_targets.tsv")
    configs: list[TargetConfig] = []
    for line_number, line in enumerate(path.read_text().splitlines(), 1):
        if not line or line.startswith("#"):
            continue
        fields = line.split("\t")
        if len(fields) != 5:
            raise ValueError(
                f"{path}:{line_number}: expected five tab-separated fields"
            )
        os_name, target, pointer_compression, build_only, suffixes = fields
        parsed_suffixes = tuple(
            "" if suffix == "default" else suffix
            for suffix in suffixes.split(",")
        )
        if not parsed_suffixes or len(parsed_suffixes) != len(set(parsed_suffixes)):
            raise ValueError(f"{path}:{line_number}: invalid feature suffixes")
        configs.append(
            TargetConfig(
                os_name,
                target,
                _parse_bool(pointer_compression),
                _parse_bool(build_only),
                parsed_suffixes,
            )
        )
    if not configs or len({config.target for config in configs}) != len(configs):
        raise ValueError(f"{path}: targets must be present and unique")
    return tuple(configs)


TARGET_CONFIGS = _load_target_configs()


def crate_version() -> str:
    manifest = Path(__file__).resolve().parent.parent / "Cargo.toml"
    package = tomllib.loads(manifest.read_text(encoding="utf-8"))["package"]
    return str(package["version"])


def release_revision() -> str:
    path = Path(__file__).with_name("nimbus_release_revision")
    revision = path.read_text(encoding="utf-8").strip()
    if re.fullmatch(r"[1-9][0-9]*", revision) is None:
        raise ValueError(
            f"{path}: expected a positive integer without leading zeros"
        )
    return revision


def validate_release_tag(
    tag: str, package_version: str, expected_revision: str
) -> None:
    pattern = (
        rf"v{re.escape(package_version)}-nimbus\."
        rf"{re.escape(expected_revision)}"
    )
    if re.fullmatch(pattern, tag) is None:
        raise ValueError(
            f"release tag {tag!r} must match the source revision "
            f"v{package_version}-nimbus.{expected_revision}"
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
    return {
        "include": [
            {
                key: value
                for key, value in asdict(config).items()
                if key != "feature_suffixes"
            }
            for config in TARGET_CONFIGS
        ]
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--github-matrix", action="store_true")
    parser.add_argument("--asset-count", action="store_true")
    parser.add_argument("--verify-tag")
    args = parser.parse_args()
    selected = sum(
        (args.github_matrix, args.asset_count, args.verify_tag is not None)
    )
    if selected != 1:
        parser.error("select exactly one output mode")
    if args.github_matrix:
        print(json.dumps(github_matrix(), separators=(",", ":"), sort_keys=True))
    elif args.asset_count:
        print(len(expected_assets()))
    else:
        try:
            validate_release_tag(
                args.verify_tag, crate_version(), release_revision()
            )
        except ValueError as error:
            parser.error(str(error))
        print(f"verified release tag {args.verify_tag}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
