#!/usr/bin/env python3
"""Fail an integration build that resolves multiple shared contract identities."""
import json
import sys

metadata = json.load(sys.stdin)
resolved = {node["id"] for node in metadata["resolve"]["nodes"]}
for name in ("zencodec", "zenmeta", "zencodec-testkit", "zenpixels", "zenpixels-convert"):
    packages = [
        package
        for package in metadata["packages"]
        if package["name"] == name and package["id"] in resolved
    ]
    if len(packages) != 1:
        raise SystemExit(f"expected one {name} identity, found {[p['id'] for p in packages]}")
    print(f"{name}: {packages[0]['id']}")
