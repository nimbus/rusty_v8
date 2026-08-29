#!/usr/bin/env python3
"""Self-tests for the Nimbus release manifest and exact asset verifier."""

from __future__ import annotations

import gzip
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

TOOLS = Path(__file__).resolve().parent
sys.path.insert(0, str(TOOLS))

from nimbus_release_manifest import (  # noqa: E402
    TARGET_CONFIGS,
    asset_names,
    expected_assets,
    github_matrix,
    validate_configuration,
)
from verify_nimbus_release_assets import (  # noqa: E402
    flatten_release_tree,
    files_by_name,
    sha256,
    verify_release_tree,
)


class ManifestTests(unittest.TestCase):
    def test_exact_matrix_and_asset_count(self) -> None:
        self.assertEqual(len(TARGET_CONFIGS), 7)
        self.assertEqual(len(expected_assets()), 44)
        self.assertEqual(len(set(expected_assets())), 44)
        self.assertEqual(len(github_matrix()["include"]), 7)

    def test_musl_names_and_no_pointer_compression(self) -> None:
        for target in (
            "x86_64-unknown-linux-musl",
            "aarch64-unknown-linux-musl",
        ):
            self.assertEqual(
                asset_names(target, ""),
                (
                    f"librusty_v8_release_{target}.a.gz",
                    f"src_binding_release_{target}.rs",
                ),
            )
            self.assertEqual(
                asset_names(target, "simdutf"),
                (
                    f"librusty_v8_simdutf_release_{target}.a.gz",
                    f"src_binding_simdutf_release_{target}.rs",
                ),
            )
            with self.assertRaisesRegex(ValueError, "unsupported.*configuration"):
                validate_configuration(target, "ptrcomp")
            with self.assertRaisesRegex(ValueError, "unsupported.*configuration"):
                validate_configuration(target, "ptrcomp_simdutf")

    def test_pointer_compression_names(self) -> None:
        for target in (
            "aarch64-apple-darwin",
            "x86_64-apple-darwin",
            "x86_64-unknown-linux-gnu",
            "aarch64-unknown-linux-gnu",
        ):
            self.assertEqual(
                asset_names(target, "ptrcomp"),
                (
                    f"librusty_v8_ptrcomp_release_{target}.a.gz",
                    f"src_binding_ptrcomp_release_{target}.rs",
                ),
            )

    def test_unknown_target_is_unselectable(self) -> None:
        with self.assertRaisesRegex(ValueError, "unsupported.*target"):
            asset_names("mips64-unknown-linux-musl", "")


class VerifierTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        for index, name in enumerate(expected_assets()):
            asset = self.root / name
            asset.write_bytes(f"asset-{index}".encode())
            asset.with_name(f"{name}.sha256").write_text(
                f"{sha256(asset)}  {name}\n", encoding="utf-8"
            )

    def tearDown(self) -> None:
        self.temp.cleanup()

    def failures(self) -> list[str]:
        failures, _ = verify_release_tree(self.root)
        return failures

    def test_valid_tree(self) -> None:
        self.assertEqual(self.failures(), [])

    def test_missing_and_empty_assets(self) -> None:
        missing, empty = expected_assets()[:2]
        (self.root / missing).unlink()
        (self.root / f"{missing}.sha256").unlink()
        (self.root / empty).write_bytes(b"")
        failures = self.failures()
        self.assertTrue(any("missing or empty asset" in item for item in failures))
        self.assertTrue(any("missing release file" in item for item in failures))

    def test_unexpected_and_misnamed_assets(self) -> None:
        name = expected_assets()[0]
        (self.root / name).rename(self.root / f"misnamed-{name}")
        (self.root / "unexpected.bin").write_bytes(b"unexpected")
        failures = self.failures()
        self.assertTrue(any("unexpected release file" in item for item in failures))
        self.assertTrue(any("missing release file" in item for item in failures))

    def test_duplicate_asset(self) -> None:
        name = expected_assets()[0]
        duplicate_dir = self.root / "duplicate"
        duplicate_dir.mkdir()
        shutil.copyfile(self.root / name, duplicate_dir / name)
        failures = self.failures()
        self.assertTrue(any("duplicate release file" in item for item in failures))

    def test_checksum_mismatch(self) -> None:
        name = expected_assets()[0]
        (self.root / f"{name}.sha256").write_text("bad\n", encoding="utf-8")
        self.assertTrue(
            any("checksum mismatch" in item for item in self.failures())
        )

    def test_flatten_verified_tree(self) -> None:
        failures, paths = verify_release_tree(self.root)
        self.assertEqual(failures, [])
        output = self.root / "flattened"
        names = set(paths)
        self.assertEqual(flatten_release_tree(paths, names, output), [])
        self.assertEqual(set(files_by_name(output)), names)

    def test_nonempty_flatten_output_is_rejected(self) -> None:
        output = self.root / "flattened"
        output.mkdir()
        (output / "occupied").write_bytes(b"occupied")
        failures = flatten_release_tree(
            files_by_name(self.root),
            set(expected_assets())
            | {f"{name}.sha256" for name in expected_assets()},
            output,
        )
        self.assertEqual(len(failures), 1)
        self.assertIn("must be empty", failures[0])


class PackagerTests(unittest.TestCase):
    def test_musl_package_names_and_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            target = "aarch64-unknown-linux-musl"
            built = root / "target" / target / "release" / "gn_out"
            (built / "obj").mkdir(parents=True)
            (built / "obj" / "librusty_v8.a").write_bytes(b"library")
            (built / "src_binding.rs").write_bytes(b"binding")
            output = root / "assets"
            result = subprocess.run(
                [
                    sys.executable,
                    str(TOOLS / "package_nimbus_release_assets.py"),
                    "--target",
                    target,
                    "--features-suffix",
                    "simdutf",
                    "--target-dir",
                    str(root / "target"),
                    "--output-dir",
                    str(output),
                ],
                check=False,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(
                set(files_by_name(output)),
                set(asset_names(target, "simdutf"))
                | {
                    f"{name}.sha256"
                    for name in asset_names(target, "simdutf")
                },
            )
            archive_name, binding_name = asset_names(target, "simdutf")
            self.assertEqual(
                gzip.decompress((output / archive_name).read_bytes()), b"library"
            )
            self.assertEqual((output / binding_name).read_bytes(), b"binding")
            for name in (archive_name, binding_name):
                self.assertEqual(
                    (output / f"{name}.sha256").read_text(encoding="utf-8"),
                    f"{sha256(output / name)}  {name}\n",
                )

    def test_musl_pointer_compression_is_rejected(self) -> None:
        result = subprocess.run(
            [
                sys.executable,
                str(TOOLS / "package_nimbus_release_assets.py"),
                "--target",
                "x86_64-unknown-linux-musl",
                "--features-suffix",
                "ptrcomp_simdutf",
            ],
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 2)
        self.assertIn("unsupported Nimbus release configuration", result.stderr)


if __name__ == "__main__":
    unittest.main()
