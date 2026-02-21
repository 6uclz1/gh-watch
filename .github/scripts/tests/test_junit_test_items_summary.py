import importlib.util
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[3]
SCRIPT_PATH = REPO_ROOT / ".github" / "scripts" / "junit_test_items_summary.py"
FIXTURE_PATH = Path(__file__).resolve().parent / "fixtures" / "sample-junit.xml"


def load_module():
    spec = importlib.util.spec_from_file_location("junit_test_items_summary", SCRIPT_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("failed to load junit_test_items_summary module")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


class JunitTestItemsSummaryTest(unittest.TestCase):
    def test_parse_junit_cases_classifies_status_and_test_id(self):
        module = load_module()
        cases = module.parse_junit_cases(FIXTURE_PATH)
        actual = [(case.test_id, case.status) for case in cases]

        self.assertEqual(
            actual,
            [
                ("test_no_class", "PASSED"),
                ("crate::mod_a::test_alpha", "FAILED"),
                ("crate::mod_a::test_skip", "SKIPPED"),
                ("crate::mod_b::test_zeta", "PASSED"),
                ("crate::mod_c::test_error", "FAILED"),
            ],
        )

    def test_build_summary_markdown_includes_counts_and_sorted_details(self):
        module = load_module()
        cases = module.parse_junit_cases(FIXTURE_PATH)
        markdown = module.build_summary_markdown(cases)

        self.assertIn("## Test Items Summary", markdown)
        self.assertIn("| Total | Passed | Failed | Skipped |", markdown)
        self.assertIn("| 5 | 2 | 2 | 1 |", markdown)

        line_positions = [
            markdown.index("- ✅ test_no_class"),
            markdown.index("- ❌ crate::mod_a::test_alpha"),
            markdown.index("- ⏭️ crate::mod_a::test_skip"),
            markdown.index("- ✅ crate::mod_b::test_zeta"),
            markdown.index("- ❌ crate::mod_c::test_error"),
        ]
        self.assertEqual(line_positions, sorted(line_positions))

    def test_cli_writes_summary_markdown_file(self):
        with tempfile.TemporaryDirectory() as tmp_dir:
            summary_path = Path(tmp_dir) / "summary.md"
            result = subprocess.run(
                [
                    "python3",
                    str(SCRIPT_PATH),
                    "--junit-path",
                    str(FIXTURE_PATH),
                    "--summary-path",
                    str(summary_path),
                ],
                check=False,
                capture_output=True,
                text=True,
            )

            self.assertEqual(result.returncode, 0, msg=result.stderr)
            self.assertTrue(summary_path.exists())
            text = summary_path.read_text(encoding="utf-8")
            self.assertIn("## Test Items Summary", text)
            self.assertIn("| 5 | 2 | 2 | 1 |", text)

    def test_cli_fails_when_junit_file_is_missing(self):
        with tempfile.TemporaryDirectory() as tmp_dir:
            summary_path = Path(tmp_dir) / "summary.md"
            missing_path = Path(tmp_dir) / "missing-junit.xml"
            result = subprocess.run(
                [
                    "python3",
                    str(SCRIPT_PATH),
                    "--junit-path",
                    str(missing_path),
                    "--summary-path",
                    str(summary_path),
                ],
                check=False,
                capture_output=True,
                text=True,
            )

            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(summary_path.exists())

    def test_cli_fails_when_junit_file_is_invalid_xml(self):
        with tempfile.TemporaryDirectory() as tmp_dir:
            invalid_junit_path = Path(tmp_dir) / "invalid-junit.xml"
            summary_path = Path(tmp_dir) / "summary.md"
            invalid_junit_path.write_text("<testsuites><testsuite>", encoding="utf-8")
            result = subprocess.run(
                [
                    "python3",
                    str(SCRIPT_PATH),
                    "--junit-path",
                    str(invalid_junit_path),
                    "--summary-path",
                    str(summary_path),
                ],
                check=False,
                capture_output=True,
                text=True,
            )

            self.assertNotEqual(result.returncode, 0)
            self.assertFalse(summary_path.exists())


if __name__ == "__main__":
    unittest.main()
