import hashlib
import importlib.util
from pathlib import Path
import struct
import unittest

spec = importlib.util.spec_from_file_location("installer", Path(__file__).with_name("prepare-web-installer.py"))
installer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(installer)


class PackagingTests(unittest.TestCase):
    def setUp(self):
        self.image = bytearray(b"\xff" * 0x1000000)
        self.image[0] = 0xE9
        table = b"".join(struct.pack("<HBBII16sI", 0x50AA, kind, subtype, offset, size, name.encode(), 0)
                         for kind, subtype, offset, size, name in installer.LAYOUT)
        table += b"\xeb\xeb" + b"\xff" * 14 + hashlib.md5(table).digest()
        self.image[0x8000:0x8000 + len(table)] = table
        self.app = b"\xe9" + b"firmware" * 600
        self.image[0x200000:0x200000 + len(self.app)] = self.app

    def test_preserves_settings_and_selects_new_application(self):
        parts = installer.flash_parts(self.image, self.app)
        device = bytearray(b"\x42" * 0x1000000)
        for _, offset, data in parts:
            # Model sector erasure as performed before each serial flash write.
            end = (offset + len(data) + 4095) // 4096 * 4096
            device[offset:end] = b"\xff" * (end - offset)
            device[offset:offset + len(data)] = data
        for _, _, offset, size, name in installer.LAYOUT:
            if name in ("lic", "nvs", "phy_init", "map", "ota_1"):
                self.assertEqual(device[offset:offset + size], b"\x42" * size, name)
        self.assertEqual(device[0x9000:0xB000], b"\xff" * 8192)
        self.assertEqual(device[0x200000:0x200000 + len(self.app)], self.app)

    def test_rejects_changed_layout(self):
        struct.pack_into("<I", self.image, 0x8000 + 4 * 32 + 4, 0x14000)
        with self.assertRaisesRegex(ValueError, "layout changed"):
            installer.flash_parts(self.image, self.app)

    def test_rejects_mismatched_application(self):
        with self.assertRaisesRegex(ValueError, "images differ"):
            installer.flash_parts(self.image, b"\xe9wrong release")

    def test_rejects_corrupt_partition_table(self):
        self.image[0x8000 + len(installer.LAYOUT) * 32 + 16] ^= 1
        with self.assertRaisesRegex(ValueError, "checksum"):
            installer.flash_parts(self.image, self.app)


if __name__ == "__main__":
    unittest.main()
