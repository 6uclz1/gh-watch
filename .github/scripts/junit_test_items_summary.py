#!/usr/bin/env python3

from __future__ import annotations

import argparse
from dataclasses import dataclass
from pathlib import Path
import xml.etree.ElementTree as ET


@dataclass(frozen=True)
class TestCaseResult:
    classname: str
    name: str
    status: str

    @property
    def test_id(self) -> str:
        return f"{self.classname}::{self.name}" if self.classname else self.name


def parse_junit_cases(junit_path: str | Path) -> list[TestCaseResult]:
    xml_path = Path(junit_path)
    root = ET.parse(xml_path).getroot()
    cases: list[TestCaseResult] = []

    for testcase in root.iter("testcase"):
        name = testcase.get("name")
        if not name:
            raise ValueError("testcase.name is required in JUnit XML")

        classname = (testcase.get("classname") or "").strip()
        if testcase.find("failure") is not None or testcase.find("error") is not None:
            status = "FAILED"
        elif testcase.find("skipped") is not None:
            status = "SKIPPED"
        else:
            status = "PASSED"

        cases.append(TestCaseResult(classname=classname, name=name, status=status))

    return sorted(cases, key=lambda case: (case.classname, case.name))


def build_summary_markdown(cases: list[TestCaseResult]) -> str:
    total = len(cases)
    passed = sum(1 for case in cases if case.status == "PASSED")
    failed = sum(1 for case in cases if case.status == "FAILED")
    skipped = sum(1 for case in cases if case.status == "SKIPPED")

    icon_map = {
        "PASSED": "✅",
        "FAILED": "❌",
        "SKIPPED": "⏭️",
    }

    lines = [
        "## Test Items Summary",
        "",
        "| Total | Passed | Failed | Skipped |",
        "| --- | --- | --- | --- |",
        f"| {total} | {passed} | {failed} | {skipped} |",
        "",
        "<details>",
        "<summary>Show all test items</summary>",
        "",
    ]

    for case in cases:
        lines.append(f"- {icon_map[case.status]} {case.test_id}")

    lines.extend(["", "</details>", ""])
    return "\n".join(lines)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Generate GitHub Actions job summary for JUnit test items."
    )
    parser.add_argument("--junit-path", required=True, help="Path to JUnit XML file")
    parser.add_argument(
        "--summary-path",
        required=True,
        help="Path to GitHub step summary file",
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    cases = parse_junit_cases(args.junit_path)
    summary = build_summary_markdown(cases)
    Path(args.summary_path).write_text(summary, encoding="utf-8")


if __name__ == "__main__":
    main()
