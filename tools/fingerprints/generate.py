#!/usr/bin/env python3
"""Генерирует набор комбинаций отпечатка для Ombra из BrowserForge.

Запуск из корня репозитория:
    python3 -m venv tools/fingerprints/.venv
    tools/fingerprints/.venv/bin/pip install -r tools/fingerprints/requirements.txt
    tools/fingerprints/.venv/bin/python tools/fingerprints/generate.py

Python нужен только здесь; приложение читает готовый src-tauri/resources/fingerprints.json.
"""
import argparse
import collections
import datetime
import json
import sys
from importlib.metadata import version

from browserforge.fingerprints import FingerprintGenerator

# Движок разработки — CloakBrowser 145: если в данных Chrome старше, BrowserForge пора обновить.
MIN_CHROME_MAJOR = 145
DEVICE_MEMORY = {2, 4, 8, 16, 32}


def is_apple(vendor, renderer):
    return "Apple" in vendor or "Apple" in renderer


def usable(arch, platform_version, width, height, cores, memory, vendor, renderer):
    """Те же правила, что в src-tauri/src/fingerprint/validate.rs."""
    return (
        arch in ("arm", "x86")
        and bool(platform_version)
        and 1024 <= width <= 7680
        and 640 <= height <= 4320
        and isinstance(cores, int) and 2 <= cores <= 32
        and memory in DEVICE_MEMORY
        and bool(vendor) and bool(renderer)
        and (arch == "arm") == is_apple(vendor, renderer)
    )


def main():
    parser = argparse.ArgumentParser(description="Набор комбинаций отпечатка из BrowserForge")
    parser.add_argument("--samples", type=int, default=6000)
    parser.add_argument("--limit", type=int, default=1500, help="сколько самых частых комбинаций оставить")
    parser.add_argument("--out", default="src-tauri/resources/fingerprints.json")
    args = parser.parse_args()

    generator = FingerprintGenerator(browser="chrome", os="macos", device="desktop")
    counts = collections.Counter()
    majors = collections.Counter()
    for _ in range(args.samples):
        fp = generator.generate()
        ua = fp.navigator.userAgentData or {}
        brands = {b.get("brand") for b in ua.get("brands", [])}
        if "Google Chrome" not in brands:
            continue
        key = (
            ua.get("architecture"), ua.get("platformVersion"), fp.screen.width, fp.screen.height,
            fp.navigator.hardwareConcurrency, fp.navigator.deviceMemory, fp.videoCard.vendor, fp.videoCard.renderer,
        )
        if not usable(*key):
            continue
        counts[key] += 1
        majors[(ua.get("uaFullVersion") or "").split(".")[0]] += 1

    if not counts:
        sys.exit("BrowserForge не дал ни одной подходящей комбинации")
    combos = [
        {
            "weight": n, "arch": arch, "os_version": pv,
            "screen": {"width": w, "height": h},
            "hardware_concurrency": cores, "device_memory": memory,
            "gpu": {"vendor": vendor, "renderer": renderer},
        }
        for (arch, pv, w, h, cores, memory, vendor, renderer), n in counts.most_common(args.limit)
    ]
    numeric = [int(m) for m in majors if m.isdigit()]
    newest = max(numeric) if numeric else 0
    catalog = {
        "generated_at": datetime.date.today().isoformat(),
        "browserforge": version("browserforge"),
        "chrome_versions": [str(m) for m in sorted(numeric, reverse=True)],
        "combos": combos,
    }
    with open(args.out, "w", encoding="utf-8") as f:
        json.dump(catalog, f, ensure_ascii=False, indent=1)
        f.write("\n")
    arches = collections.Counter(c["arch"] for c in combos)
    print(f"комбинаций: {len(combos)} (arm {arches['arm']}, x86 {arches['x86']}), самый новый Chrome: {newest}")
    if newest < MIN_CHROME_MAJOR:
        print(f"ВНИМАНИЕ: самый новый Chrome в данных {newest} < {MIN_CHROME_MAJOR} — обновите BrowserForge", file=sys.stderr)
        sys.exit(2)


if __name__ == "__main__":
    main()
