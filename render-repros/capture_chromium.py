#!/usr/bin/env python3
"""Capture one deterministic Chromium reference with Playwright."""

import argparse
import json
import sys
from pathlib import Path

from playwright.sync_api import sync_playwright


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("url")
    parser.add_argument("output", type=Path)
    parser.add_argument("--width", type=int, default=900)
    parser.add_argument("--height", type=int, default=1000)
    parser.add_argument("--settle-ms", type=int, default=2000)
    parser.add_argument("--executable")
    args = parser.parse_args()

    launch = {"headless": True}
    if args.executable:
        launch["executable_path"] = args.executable

    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(**launch)
        context = browser.new_context(
            viewport={"width": args.width, "height": args.height},
            device_scale_factor=1,
            locale="en-US",
            timezone_id="UTC",
            color_scheme="light",
        )
        page = context.new_page()
        page.goto(args.url, wait_until="load", timeout=30_000)
        # Fixture text metrics must come from the declared webfonts, not from a
        # host font that happens to share the family name. Waiting here adds
        # harness latency only; it does not change browser defaults.
        fonts = page.evaluate(
            """async () => {
              await document.fonts.ready;
              return Array.from(document.fonts, (face) => ({
                family: face.family,
                weight: face.weight,
                style: face.style,
                status: face.status,
              }));
            }"""
        )
        failed = [face for face in fonts if face["status"] == "error"]
        if failed:
            print(
                json.dumps({"error": "font face failed to load", "fonts": failed}),
                file=sys.stderr,
            )
            sys.exit(1)
        page.wait_for_timeout(args.settle_ms)
        page.screenshot(path=str(args.output))
        print(
            json.dumps(
                {
                    "browser": "chromium",
                    "version": browser.version,
                    "viewport": [args.width, args.height],
                    "device_scale_factor": 1,
                    "settle_ms": args.settle_ms,
                    "fonts": fonts,
                }
            )
        )
        context.close()
        browser.close()


if __name__ == "__main__":
    main()
