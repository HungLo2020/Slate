import importlib.util
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest.mock import Mock, call, patch


SCRIPT_PATH = Path(__file__).parents[1] / "DevUtils/PublishMattOSPackage.py"
SPEC = importlib.util.spec_from_file_location("publish_mattos_package", SCRIPT_PATH)
assert SPEC and SPEC.loader
publish = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(publish)


PACKAGES_INDEX = """Package: other
Version: 9.9.9

Package: slate
Version: 0.3.0
Depends: libc6 (>= 2.39)

Package: slate
Version: 0.2.0
"""


def simple_greater(candidate, existing):
    """Stand-in for dpkg --compare-versions for plain dotted versions."""
    return tuple(map(int, candidate.split("."))) > tuple(map(int, existing.split(".")))


class FakeResponse:
    def __init__(self, content):
        self.content = content

    def __enter__(self):
        return self

    def __exit__(self, *args):
        return False

    def read(self):
        return self.content


class VersionGuardTests(unittest.TestCase):
    def test_published_versions_reads_only_the_named_package(self):
        self.assertEqual(publish.published_versions(PACKAGES_INDEX), ["0.3.0", "0.2.0"])

    def test_refuses_versions_that_are_not_newer(self):
        for version in ("0.3.0", "0.2.5"):
            with self.subTest(version=version), self.assertRaises(ValueError):
                publish.ensure_version_is_new(version, ["0.3.0", "0.2.0"], simple_greater)

    def test_allows_a_newer_version(self):
        publish.ensure_version_is_new("0.3.1", ["0.3.0", "0.2.0"], simple_greater)

    def test_fetch_falls_back_to_gzip_index(self):
        def opener(request, timeout):
            if request.full_url.endswith(".gz"):
                return FakeResponse(publish.gzip.compress(PACKAGES_INDEX.encode()))
            raise publish.URLError("404")

        self.assertEqual(publish.fetch_packages_index(opener), PACKAGES_INDEX)

    def test_publish_stops_before_building_when_version_exists(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with (
                patch.object(publish, "repository_root", return_value=root),
                patch.object(publish, "download_latest_script"),
                patch.object(publish, "read_workspace_version", return_value="0.3.0"),
                patch.object(publish, "fetch_packages_index", return_value=PACKAGES_INDEX),
                patch.object(publish, "dpkg_version_greater", side_effect=simple_greater),
                patch.object(publish, "run", return_value=0) as run,
            ):
                self.assertEqual(publish.main(["publish"]), 1)

            run.assert_not_called()

    def test_reads_workspace_version(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "Cargo.toml").write_text(
                '[workspace]\nmembers = []\n\n[workspace.package]\nversion = "1.2.3"\n',
                encoding="utf-8",
            )
            self.assertEqual(publish.read_workspace_version(root), "1.2.3")

    def test_dpkg_comparison_matches_apt_ordering(self):
        if not shutil.which("dpkg"):
            self.skipTest("dpkg not available")
        self.assertTrue(publish.dpkg_version_greater("0.10.0", "0.9.0"))
        self.assertFalse(publish.dpkg_version_greater("0.3.0", "0.3.0"))


class PublishMattOSPackageTests(unittest.TestCase):
    def test_doctor_passes_explicit_repository(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with (
                patch.object(publish, "repository_root", return_value=root),
                patch.object(publish, "download_latest_script") as download,
                patch.object(publish, "run", return_value=0) as run,
            ):
                self.assertEqual(publish.main(["doctor"]), 0)

            download.assert_called_once_with(root / publish.SCRIPT_RELATIVE_PATH)
            run.assert_called_once_with(
                [publish.sys.executable, str(root / publish.SCRIPT_RELATIVE_PATH),
                 "--repo", "mattpackages", "doctor"],
                root,
            )

    def test_upload_passes_explicit_repository(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            artifact = root / "builds/pkg.deb"
            artifact.parent.mkdir()
            artifact.touch()
            (root / publish.BUILD_METADATA_RELATIVE_PATH).write_text(
                "BUILD_ARTIFACT_TYPE=deb\nBUILD_ARTIFACT_PATH=builds/pkg.deb\n",
                encoding="utf-8",
            )
            with (
                patch.object(publish, "repository_root", return_value=root),
                patch.object(publish, "download_latest_script") as download,
                patch.object(publish, "read_workspace_version", return_value="0.4.0"),
                patch.object(publish, "fetch_packages_index", return_value=PACKAGES_INDEX),
                patch.object(publish, "dpkg_version_greater", side_effect=simple_greater),
                patch.object(publish, "validate_deb_artifact") as validate,
                patch.object(publish, "run", return_value=0) as run,
            ):
                self.assertEqual(publish.main(["publish"]), 0)

            download.assert_called_once_with(root / publish.SCRIPT_RELATIVE_PATH)
            validate.assert_called_once_with(artifact.resolve(), "0.4.0")
            self.assertEqual(run.call_args_list, [
                call(["bash", str(root / "DevUtils/Build.sh")], root),
                call(
                    [publish.sys.executable, str(root / publish.SCRIPT_RELATIVE_PATH),
                     "--repo", "mattpackages", "upload", "--no-overwrites", str(artifact.resolve())],
                    root,
                ),
            ])

    def test_parse_build_metadata(self):
        metadata = publish.parse_build_metadata(
            "# generated\nBUILD_ARTIFACT_TYPE=deb\nBUILD_ARTIFACT_PATH=builds/pkg.deb\n"
        )
        self.assertEqual(metadata["BUILD_ARTIFACT_TYPE"], "deb")
        self.assertEqual(metadata["BUILD_ARTIFACT_PATH"], "builds/pkg.deb")

    def test_rejects_non_deb_artifact(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            with self.assertRaisesRegex(ValueError, "BUILD_ARTIFACT_TYPE=deb"):
                publish.resolve_deb_artifact(
                    {"BUILD_ARTIFACT_TYPE": "rpm", "BUILD_ARTIFACT_PATH": "pkg.rpm"}, root
                )

    def test_rejects_missing_package_path(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(ValueError, "package path does not exist"):
                publish.resolve_deb_artifact(
                    {"BUILD_ARTIFACT_TYPE": "deb", "BUILD_ARTIFACT_PATH": "missing.deb"},
                    Path(directory),
                )

    def test_validates_downloaded_script_content(self):
        self.assertIn("print", publish.validate_script_content(b"print('repository manager')\n"))
        with self.assertRaisesRegex(ValueError, "empty"):
            publish.validate_script_content(b" \n")
        with self.assertRaisesRegex(ValueError, "valid UTF-8 Python"):
            publish.validate_script_content(b"not valid python !!!")

    def test_failed_download_does_not_reuse_cached_script(self):
        class FailedResponse:
            def __enter__(self):
                raise OSError("network unavailable")

            def __exit__(self, *args):
                return False

        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "ManageMattOSRepository.py"
            target.write_text("old cached script", encoding="utf-8")

            opener = Mock(return_value=FailedResponse())
            with self.assertRaises(OSError):
                publish.download_latest_script(target, opener=opener)

            self.assertEqual(target.read_text(encoding="utf-8"), "old cached script")
            self.assertEqual(list(target.parent.glob("*.tmp")), [])
            request = opener.call_args.args[0]
            self.assertEqual(request.get_header("Cache-control"), "no-cache")


class UploadValidationTests(unittest.TestCase):
    def test_rejects_wrong_name_version_or_architecture(self):
        for fields in ("other\n0.1.0\namd64\n", "slate\n0.0.1\namd64\n", "slate\n0.1.0\narm64\n"):
            with self.subTest(fields=fields), patch.object(publish.subprocess, "run", return_value=Mock(stdout=fields)):
                with self.assertRaisesRegex(ValueError, "metadata does not match"):
                    publish.validate_deb_artifact(Path("test.deb"), "0.1.0")

    def test_accepts_matching_deb_metadata(self):
        with patch.object(publish.subprocess, "run", return_value=Mock(stdout="slate\n0.1.0\namd64\n")):
            publish.validate_deb_artifact(Path("test.deb"), "0.1.0")

    def test_existing_package_dry_run_skips_build_and_uses_server_dry_run(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            artifact = root / "slate.deb"
            artifact.touch()
            with (
                patch.object(publish, "repository_root", return_value=root),
                patch.object(publish, "download_latest_script"),
                patch.object(publish, "read_workspace_version", return_value="0.1.0"),
                patch.object(publish, "fetch_packages_index", return_value=""),
                patch.object(publish, "validate_deb_artifact") as validate,
                patch.object(publish, "run", return_value=0) as run,
            ):
                self.assertEqual(publish.main(["publish", "--dry-run", "--package", str(artifact)]), 0)
            validate.assert_called_once_with(artifact, "0.1.0")
            run.assert_called_once_with(
                [publish.sys.executable, str(root / publish.SCRIPT_RELATIVE_PATH),
                 "--repo", "mattpackages", "--dry-run", "upload", "--no-overwrites", str(artifact)], root,
            )

    def test_build_failure_does_not_upload_a_stale_artifact(self):
        with (
            patch.object(publish, "download_latest_script"),
            patch.object(publish, "read_workspace_version", return_value="0.1.0"),
            patch.object(publish, "fetch_packages_index", return_value=""),
            patch.object(publish, "run", return_value=7) as run,
        ):
            self.assertEqual(publish.main(["publish"]), 7)
        self.assertEqual(run.call_count, 1)
        self.assertEqual(run.call_args.args[0][0], "bash")

    def test_concurrent_publication_is_detected_after_building(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            artifact = root / "slate.deb"; artifact.touch()
            with (
                patch.object(publish, "repository_root", return_value=root),
                patch.object(publish, "download_latest_script"),
                patch.object(publish, "read_workspace_version", return_value="0.1.0"),
                patch.object(publish, "fetch_packages_index", side_effect=["", "Package: slate\nVersion: 0.1.0\n"]),
                patch.object(publish, "validate_deb_artifact"),
                patch.object(publish, "run", return_value=0) as run,
            ):
                self.assertEqual(publish.main(["publish", "--package", str(artifact)]), 1)
            run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
