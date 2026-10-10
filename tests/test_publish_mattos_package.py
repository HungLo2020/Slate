import importlib.util
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest.mock import Mock, patch


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


SCRIPT = b"print('pinned repository manager')\n"


def pinned_script():
    """Accept SCRIPT as the pinned repository manager."""
    return patch.object(publish, "REPOSITORY_SCRIPT_SHA256", publish.hashlib.sha256(SCRIPT).hexdigest())


class RecordingRun:
    """Stand-in for run(): records commands and the manager script they execute."""

    def __init__(self, status=0):
        self.status = status
        self.commands = []
        self.scripts = []

    def __call__(self, command, root):
        self.commands.append((command, root))
        if command[:2] == [publish.sys.executable, "-I"]:
            self.scripts.append((Path(command[2]), Path(command[2]).read_bytes()))
        return self.status

    def manager_arguments(self, root):
        """The manager's arguments, after checking how it was run."""
        managers = [command for command, _ in self.commands if command[:2] == [publish.sys.executable, "-I"]]
        assert len(managers) == 1, self.commands
        script, content = self.scripts[0]
        assert content == SCRIPT
        assert script.name == "ManageMattOSRepository.py"
        assert root not in script.parents, "the manager ran from the repository"
        assert not script.exists(), "the private copy was left behind"
        return managers[0][3:]


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
            run = RecordingRun()
            with (
                pinned_script(),
                patch.object(publish, "repository_root", return_value=root),
                patch.object(publish, "download_latest_script", return_value=SCRIPT) as download,
                patch.object(publish, "run", side_effect=run),
            ):
                self.assertEqual(publish.main(["doctor"]), 0)

            download.assert_called_once_with(root / publish.SCRIPT_RELATIVE_PATH)
            self.assertEqual(run.manager_arguments(root), ["--repo", "mattpackages", "doctor"])

    def test_manager_runs_isolated_from_a_private_copy_of_the_verified_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            # The cached download is replaced after verification: never run.
            cached = root / publish.SCRIPT_RELATIVE_PATH
            cached.parent.mkdir(parents=True)
            cached.write_text("raise SystemExit('replaced script ran')\n")
            run = RecordingRun(status=3)
            with pinned_script(), patch.object(publish, "run", side_effect=run):
                self.assertEqual(publish.run_manager(SCRIPT, ["doctor"], root), 3)
            self.assertEqual(run.manager_arguments(root), ["--repo", "mattpackages", "doctor"])
            # Bytes that are not the pinned script are refused before running.
            with self.assertRaisesRegex(ValueError, "pinned SHA-256"):
                publish.run_manager(b"print('other')\n", ["doctor"], root)

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
            run = RecordingRun()
            with (
                pinned_script(),
                patch.object(publish, "repository_root", return_value=root),
                patch.object(publish, "download_latest_script", return_value=SCRIPT) as download,
                patch.object(publish, "read_workspace_version", return_value="0.4.0"),
                patch.object(publish, "fetch_packages_index", return_value=PACKAGES_INDEX),
                patch.object(publish, "dpkg_version_greater", side_effect=simple_greater),
                patch.object(publish, "ensure_release_source", return_value="a" * 40),
                patch.object(publish, "validate_artifact_source"),
                patch.object(publish, "validate_deb_artifact") as validate,
                patch.object(publish, "run", side_effect=run),
            ):
                self.assertEqual(publish.main(["publish"]), 0)

            download.assert_called_once_with(root / publish.SCRIPT_RELATIVE_PATH)
            validate.assert_called_once_with(artifact.resolve(), "0.4.0")
            self.assertEqual(run.commands[0], (["bash", str(root / "DevUtils/Build.sh")], root))
            self.assertEqual(run.manager_arguments(root),
                             ["--repo", "mattpackages", "upload", "--no-overwrites", str(artifact.resolve())])

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
        def pinned(content):
            return publish.validate_script_content(content, publish.hashlib.sha256(content).hexdigest())

        self.assertIn("print", pinned(b"print('repository manager')\n"))
        with self.assertRaisesRegex(ValueError, "empty"):
            pinned(b" \n")
        with self.assertRaisesRegex(ValueError, "valid UTF-8 Python"):
            pinned(b"not valid python !!!")

    def test_script_is_pinned_to_a_commit_and_digest(self):
        self.assertRegex(publish.REPOSITORY_SCRIPT_COMMIT, r"^[0-9a-f]{40}$")
        self.assertRegex(publish.REPOSITORY_SCRIPT_SHA256, r"^[0-9a-f]{64}$")
        self.assertIn(f"/{publish.REPOSITORY_SCRIPT_COMMIT}/", publish.REPOSITORY_SCRIPT_URL)
        self.assertNotIn("/master/", publish.REPOSITORY_SCRIPT_URL)

    def test_rejects_script_that_does_not_match_the_pinned_digest(self):
        # Valid Python from a changed (or compromised) upstream is still refused.
        with self.assertRaisesRegex(ValueError, "does not match the pinned SHA-256"):
            publish.validate_script_content(b"print('changed upstream')\n")

    def test_download_saves_only_the_verified_pinned_script(self):
        content = b"print('pinned manager')\r\n"
        with tempfile.TemporaryDirectory() as directory:
            target = Path(directory) / "ManageMattOSRepository.py"
            target.write_text("old cached script", encoding="utf-8")
            opener = Mock(return_value=FakeResponse(b"print('tampered')\n"))
            with patch.object(publish, "REPOSITORY_SCRIPT_SHA256", publish.hashlib.sha256(content).hexdigest()):
                with self.assertRaisesRegex(ValueError, "pinned SHA-256"):
                    publish.download_latest_script(target, opener=opener)
                self.assertEqual(target.read_text(encoding="utf-8"), "old cached script")
                self.assertEqual(list(target.parent.glob("*.tmp")), [])

                opener = Mock(return_value=FakeResponse(content))
                self.assertEqual(publish.download_latest_script(target, opener=opener), content)
            self.assertEqual(target.read_bytes(), content)
            self.assertEqual(opener.call_args.args[0].full_url, publish.REPOSITORY_SCRIPT_URL)

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
            run = RecordingRun()
            with (
                pinned_script(),
                patch.object(publish, "repository_root", return_value=root),
                patch.object(publish, "download_latest_script", return_value=SCRIPT),
                patch.object(publish, "read_workspace_version", return_value="0.1.0"),
                patch.object(publish, "fetch_packages_index", return_value=""),
                patch.object(publish, "validate_deb_artifact") as validate,
                patch.object(publish, "run", side_effect=run),
            ):
                self.assertEqual(publish.main(["publish", "--dry-run", "--package", str(artifact)]), 0)
            validate.assert_called_once_with(artifact, "0.1.0")
            self.assertEqual(len(run.commands), 1)
            self.assertEqual(run.manager_arguments(root),
                             ["--repo", "mattpackages", "--dry-run", "upload", "--no-overwrites", str(artifact)])

    def test_build_failure_does_not_upload_a_stale_artifact(self):
        with (
            patch.object(publish, "download_latest_script"),
            patch.object(publish, "read_workspace_version", return_value="0.1.0"),
            patch.object(publish, "fetch_packages_index", return_value=""),
            patch.object(publish, "ensure_release_source", return_value="a" * 40),
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

class ReleaseSourceGateTests(unittest.TestCase):
    def run_output(self, text):
        result = Mock()
        result.stdout = text
        return result

    def test_clean_pushed_main_with_latest_success_is_required(self):
        head = "a" * 40
        success = publish.json.dumps([{"headSha": head, "status": "completed", "conclusion": "success"}])
        for changed, branch, remote, runs, allowed in [
            ("", "main", head, success, True),
            (" M README.md", "main", head, success, False),
            ("", "feature", head, success, False),
            ("", "main", "b" * 40, success, False),
            ("", "main", head, "[]", False),
            ("", "main", head, publish.json.dumps([{"headSha": head, "status": "in_progress", "conclusion": None}]), False),
            ("", "main", head, publish.json.dumps([{"headSha": head, "status": "completed", "conclusion": "failure"}]), False),
        ]:
            with self.subTest(changed=changed, branch=branch, remote=remote, runs=runs):
                outputs = [changed, branch, head, remote, runs]
                with patch.object(publish.subprocess, "run", side_effect=[self.run_output(s) for s in outputs]):
                    if allowed:
                        self.assertEqual(publish.ensure_release_source(Path("/source")), head)
                    else:
                        with self.assertRaises(ValueError):
                            publish.ensure_release_source(Path("/source"))

    def test_artifact_source_marker_must_match_clean_head(self):
        head = "a" * 40
        for commit, dirty, allowed in [(head,"false",True),(head,"true",False),("b"*40,"false",False),("","",False)]:
            with patch.object(publish.subprocess,"run",return_value=self.run_output(f"{commit}\n{dirty}\n")):
                if allowed:
                    publish.validate_artifact_source(Path("artifact.deb"),head)
                else:
                    with self.assertRaises(ValueError):
                        publish.validate_artifact_source(Path("artifact.deb"),head)
