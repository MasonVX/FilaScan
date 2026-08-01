#!/usr/bin/env python3
"""Poll the reader firmware and mirror Bambu filament definitions to Spoolman."""

from __future__ import annotations

import json
import logging
import os
import time
import urllib.error
import urllib.parse
import urllib.request


LOG = logging.getLogger("spoolman-bridge")


def env_bool(name: str, default: bool = False) -> bool:
    return os.getenv(name, str(default)).strip().lower() in {"1", "true", "yes", "on"}


def api_base(url: str) -> str:
    base = url.rstrip("/")
    return base if base.endswith("/api/v1") else f"{base}/api/v1"


def density_for(material: str) -> float:
    material = material.upper()
    densities = {
        "PLA": 1.24,
        "PETG": 1.27,
        "ABS": 1.04,
        "ASA": 1.07,
        "TPU": 1.21,
        "PA": 1.14,
        "PC": 1.20,
        "PVA": 1.23,
    }
    return next((density for prefix, density in densities.items() if material.startswith(prefix)), 1.24)


class JsonClient:
    def __init__(self, base_url: str, timeout: float = 10.0):
        self.base_url = base_url.rstrip("/")
        self.timeout = timeout

    def request(self, method: str, path: str, body: dict | None = None):
        data = None if body is None else json.dumps(body).encode("utf-8")
        request = urllib.request.Request(
            f"{self.base_url}{path}",
            data=data,
            method=method,
            headers={"Accept": "application/json", "Content-Type": "application/json"},
        )
        try:
            with urllib.request.urlopen(request, timeout=self.timeout) as response:
                return json.load(response)
        except urllib.error.HTTPError as error:
            details = error.read().decode("utf-8", "replace")
            raise RuntimeError(f"{method} {path}: HTTP {error.code}: {details}") from error


class SpoolmanSync:
    def __init__(self, base_url: str, create_spool: bool):
        self.client = JsonClient(api_base(base_url))
        self.create_spool = create_spool

    @staticmethod
    def filament_external_id(scan: dict) -> str:
        return f"bambu:{scan['material_id']}:{scan['color_hex']}"

    def find_one(self, resource: str, field: str, value: str):
        query = urllib.parse.urlencode({field: f'"{value}"', "limit": 10})
        rows = self.client.request("GET", f"/{resource}?{query}")
        return next((row for row in rows if row.get(field) == value), None)

    def ensure_vendor(self, scan: dict) -> dict:
        external_id = "bambu-lab"
        vendor = self.find_one("vendor", "external_id", external_id)
        if vendor:
            return vendor
        LOG.info("Creating Spoolman vendor %s", scan["vendor"])
        return self.client.request(
            "POST",
            "/vendor",
            {"name": scan["vendor"], "external_id": external_id},
        )

    def ensure_filament(self, scan: dict, vendor: dict) -> dict:
        external_id = self.filament_external_id(scan)
        filament = self.find_one("filament", "external_id", external_id)
        if filament:
            return filament

        display_name = " ".join(
            part for part in (scan["material"], scan.get("variant"), scan.get("color_name")) if part
        )[:64]
        payload = {
            "name": display_name,
            "vendor_id": vendor["id"],
            "material": scan["material"],
            "density": density_for(scan["material"]),
            "diameter": 1.75,
            "color_hex": scan["color_hex"],
            "article_number": scan["material_id"],
            "external_id": external_id,
            "comment": "Imported from a Bambu Lab factory RFID tag.",
        }
        if scan.get("nominal_weight_g", 0) > 0:
            payload["weight"] = scan["nominal_weight_g"]
        LOG.info("Creating Spoolman filament %s", display_name)
        return self.client.request("POST", "/filament", payload)

    def ensure_spool(self, scan: dict, filament: dict) -> dict | None:
        if not self.create_spool:
            return None
        # Both factory tags on one spool have different chip UIDs but share the
        # tray/spool UID stored in block 9. Prefer that stable physical identity.
        physical_id = scan.get("spool_uid") or scan["tag_id"]
        marker = f"Bambu spool UID: {physical_id}"
        spools = self.client.request("GET", "/spool?limit=1000")
        existing = next((spool for spool in spools if marker in (spool.get("comment") or "")), None)
        if existing:
            return existing
        payload = {"filament_id": filament["id"], "comment": marker}
        if scan.get("nominal_weight_g", 0) > 0:
            payload["initial_weight"] = scan["nominal_weight_g"]
        LOG.info("Creating Spoolman spool for UID %s", physical_id)
        return self.client.request("POST", "/spool", payload)

    def sync(self, scan: dict) -> None:
        vendor = self.ensure_vendor(scan)
        filament = self.ensure_filament(scan, vendor)
        spool = self.ensure_spool(scan, filament)
        LOG.info(
            "Synced scan %s -> filament #%s%s",
            scan["sequence"],
            filament["id"],
            f", spool #{spool['id']}" if spool else "",
        )


def main() -> None:
    logging.basicConfig(level=os.getenv("LOG_LEVEL", "INFO"), format="%(asctime)s %(levelname)s %(message)s")
    reader_url = os.environ["READER_URL"].rstrip("/")
    spoolman_url = os.environ["SPOOLMAN_URL"]
    interval = float(os.getenv("POLL_INTERVAL_SECONDS", "1"))
    sync = SpoolmanSync(spoolman_url, create_spool=env_bool("SPOOLMAN_CREATE_SPOOL"))
    reader = JsonClient(reader_url)
    last_sequence = None

    LOG.info("Polling %s and syncing to %s", reader_url, api_base(spoolman_url))
    while True:
        try:
            scan = reader.request("GET", "/api/reader/last-scan")
            if scan and scan.get("sequence") != last_sequence:
                sync.sync(scan)
                last_sequence = scan["sequence"]
        except (OSError, RuntimeError, ValueError, KeyError) as error:
            LOG.warning("Sync attempt failed: %s", error)
        time.sleep(interval)


if __name__ == "__main__":
    main()
