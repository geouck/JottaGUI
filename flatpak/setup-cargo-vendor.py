#!/usr/bin/env python3
"""Download and set up vendored cargo sources for offline builds.

Usage: setup-cargo-vendor.py <cargo-sources.json> <vendor-dir>

Reads a cargo-sources.json (flatpak-cargo-generator format), downloads all
crates from static.crates.io, and creates a vendor directory that cargo can
use with CARGO_NET_OFFLINE=true.
"""

import hashlib
import json
import os
import shutil
import sys
import tarfile
import urllib.request


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def main():
    if len(sys.argv) != 3:
        print(f"Usage: {sys.argv[0]} <cargo-sources.json> <vendor-dir>", file=sys.stderr)
        sys.exit(1)

    sources_file = sys.argv[1]
    vendor_dir = sys.argv[2]

    with open(sources_file) as f:
        data = json.load(f)

    sources = data["cargo"]["sources"]
    print(f"Setting up {len(sources)} vendored crates in {vendor_dir}/")

    os.makedirs(vendor_dir, exist_ok=True)

    tmp_dir = vendor_dir + ".tmp"
    os.makedirs(tmp_dir, exist_ok=True)

    for i, src in enumerate(sources):
        url = src["url"]
        expected_sha = src["sha256"]
        dest = src["dest"]
        crate_name = os.path.basename(dest)
        crate_file = os.path.join(tmp_dir, crate_name + ".crate")

        # Download
        if not os.path.exists(crate_file):
            print(f"[{i+1}/{len(sources)}] Downloading {crate_name}...")
            urllib.request.urlretrieve(url, crate_file)
        else:
            print(f"[{i+1}/{len(sources)}] Cached {crate_name}")

        # Verify checksum
        actual_sha = sha256_file(crate_file)
        if actual_sha != expected_sha:
            print(f"ERROR: sha256 mismatch for {crate_name}", file=sys.stderr)
            print(f"  expected: {expected_sha}", file=sys.stderr)
            print(f"  actual:   {actual_sha}", file=sys.stderr)
            sys.exit(1)

        # Extract
        extract_dir = os.path.join(vendor_dir, crate_name)
        if os.path.exists(extract_dir):
            shutil.rmtree(extract_dir)
        os.makedirs(extract_dir, exist_ok=True)

        with tarfile.open(crate_file, "r:gz") as tf:
            # Crates extract to a top-level dir like "name-version/"
            # We extract and move contents up one level
            tmp_extract = os.path.join(tmp_dir, "extract_" + crate_name)
            if os.path.exists(tmp_extract):
                shutil.rmtree(tmp_extract)
            os.makedirs(tmp_extract, exist_ok=True)
            tf.extractall(tmp_extract)
            # Move the single top-level dir contents to extract_dir
            entries = os.listdir(tmp_extract)
            if len(entries) == 1 and os.path.isdir(os.path.join(tmp_extract, entries[0])):
                src_dir = os.path.join(tmp_extract, entries[0])
                for item in os.listdir(src_dir):
                    shutil.move(os.path.join(src_dir, item), os.path.join(extract_dir, item))
            else:
                for item in entries:
                    shutil.move(os.path.join(tmp_extract, item), os.path.join(extract_dir, item))
            shutil.rmtree(tmp_extract)

        # Create .cargo-checksum.json (required by cargo vendor format)
        checksum_file = os.path.join(extract_dir, ".cargo-checksum.json")
        with open(checksum_file, "w") as cf:
            json.dump({"files": {}, "package": expected_sha}, cf)

    # Cleanup tmp
    shutil.rmtree(tmp_dir, ignore_errors=True)

    print(f"Done. {len(sources)} crates vendored in {vendor_dir}/")


if __name__ == "__main__":
    main()
