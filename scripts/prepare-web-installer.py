#!/usr/bin/env python3
"""Package existing release assets for Pages without writing settings partitions."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import struct
import zlib


# Only offer preservation for the reviewed FilaScan 16 MB layout. Fail closed
# if a future release moves settings or changes the boot/application layout.
LAYOUT = [
    (1, 0, 0x9000, 0x2000, "otadata"),
    (0x40, 1, 0xB000, 0x1000, "lic"),
    (1, 2, 0xC000, 0x6000, "nvs"),
    (1, 1, 0x12000, 0x1000, "phy_init"),
    (0x40, 1, 0x13000, 0x100000, "map"),
    (0, 0x10, 0x200000, 0x700000, "ota_0"),
    (0, 0x11, 0x900000, 0x700000, "ota_1"),
]


def flash_parts(merged, app):
    if len(merged) != 0x1000000 or merged[0] != 0xE9:
        raise ValueError("Expected a complete 16 MB ESP32-S3 image")
    entries = []
    for index in range(len(LAYOUT)):
        start = 0x8000 + index * 32
        magic, kind, subtype, offset, size, name, flags = struct.unpack(
            "<HBBII16sI", merged[start:start + 32])
        if magic != 0x50AA or flags != 0:
            raise ValueError("Unsupported partition table")
        entries.append((kind, subtype, offset, size, name.rstrip(b"\0").decode()))
    if entries != LAYOUT:
        raise ValueError("Partition layout changed; review settings preservation first")
    end = 0x8000 + len(LAYOUT) * 32
    if merged[end:end + 16] != b"\xeb\xeb" + b"\xff" * 14:
        raise ValueError("Unexpected additional partition or missing table checksum")
    if merged[end + 16:end + 32] != hashlib.md5(merged[0x8000:end]).digest():
        raise ValueError("Partition table checksum mismatch")
    if not app or app[0] != 0xE9 or len(app) > 0x700000:
        raise ValueError("Invalid application image")
    if merged[0x200000:0x200000 + len(app)] != app:
        raise ValueError("USB and OTA application images differ")
    # Erased otadata selects ota_0 even when the old installation used ota_1.
    # Only these sectors are written; lic/nvs/phy_init/map are never included.
    return [
        ("bootloader.bin", 0, merged[:0x8000]),
        ("partitions.bin", 0x8000, merged[0x8000:0x9000]),
        ("ota-reset.bin", 0x9000, b"\xff" * 0x2000),
        ("application.bin", 0x200000, app),
    ]


def prepare(assets, output):
    metadata = (assets / "ota.toml").read_text()
    def field(name):
        match = re.search(r'^' + name + r'\s*=\s*"?([^"\s]+)"?\s*$', metadata, re.M)
        if not match:
            raise ValueError("Missing OTA field: " + name)
        return match.group(1)
    version = field("version")
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        raise ValueError("Invalid release version")
    filename = "FilaScan-" + version + "-ota.bin"
    if field("filename") != filename:
        raise ValueError("Unexpected OTA filename")
    app = (assets / filename).read_bytes()
    if len(app) != int(field("filesize")) or "%08x" % zlib.crc32(app) != field("crc32"):
        raise ValueError("OTA size or checksum mismatch")
    parts = flash_parts((assets / "FilaScan-esp32s3.bin").read_bytes(), app)
    output.mkdir(parents=True, exist_ok=True)
    shutil.copytree(Path(__file__).resolve().parents[1] / "website", output, dirs_exist_ok=True)
    firmware = output / "firmware" / version
    firmware.mkdir(parents=True, exist_ok=True)
    manifest_parts = []
    for name, offset, data in parts:
        (firmware / name).write_bytes(data)
        manifest_parts.append({"path": "firmware/" + version + "/" + name, "offset": offset})
    manifest = {
        "name": "FilaScan", "version": version,
        "new_install_prompt_erase": True,
        "new_install_improv_wait_time": 0,
        "builds": [{"chipFamily": "ESP32-S3", "parts": manifest_parts}],
    }
    (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
    # One deployment contains both the installer and the unchanged OTA channel.
    shutil.copyfile(assets / filename, output / filename)
    shutil.copyfile(assets / "ota.toml", output / "ota.toml")
    print("Prepared web installer and OTA channel for FilaScan " + version)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("assets", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    prepare(args.assets, args.output)
