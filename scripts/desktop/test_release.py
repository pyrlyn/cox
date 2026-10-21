# Copyright (c) 2026 Ivan Tugay
# SPDX-License-Identifier: GPL-3.0-or-later
# Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

"""Tests for the notarized app's release scripts (T37.32.2).

The app cask's text, the refusals that keep an unsigned or unnotarized build from passing as a
release (no secrets, no bundled CLI), and, where Sparkle's tools have been resolved by an app
build, a real appcast for a scratch DMG signed with a throwaway EdDSA key and verified. Nothing
here reads a real key or the Keychain.
"""

import base64
import os
import plistlib
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
CASK = ROOT / "scripts" / "cask.sh"
SHA = "a" * 64
SPARKLE_BIN = Path(
    os.environ.get(
        "SPARKLE_BIN",
        ROOT / "desktop/macos/build/DerivedData/SourcePackages/artifacts/sparkle/Sparkle/bin",
    )
)

# The secrets the scripts read, removed so a developer's shell cannot leak into a run.
SECRETS = ("NOTARY_KEY", "NOTARY_KEY_ID", "NOTARY_ISSUER", "SPARKLE_ED_KEY", "COX_CLI")


def run(*args, env=None):
    clean = {k: v for k, v in os.environ.items() if k not in SECRETS}
    return subprocess.run(
        ["bash", *map(str, args)], capture_output=True, text=True, env={**clean, **(env or {})}
    )


class AppCask(unittest.TestCase):
    def test_the_app_cask_names_the_notarized_dmg_of_the_release(self):
        out = run(CASK, "--app", "v0.1.2", "57", SHA)
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertIn('cask "cox-app" do', out.stdout)
        self.assertIn('version "0.1.2,57"', out.stdout)
        self.assertIn(f'sha256 "{SHA}"', out.stdout)
        self.assertIn(
            "releases/download/v#{version.csv.first}/"
            "Cox-#{version.csv.first}-#{version.csv.second}-arm64.dmg",
            out.stdout,
        )
        self.assertIn("auto_updates true", out.stdout)
        self.assertIn("strategy :sparkle", out.stdout)
        self.assertNotIn("binary ", out.stdout)

    def test_a_bad_build_number_or_checksum_is_refused(self):
        self.assertEqual(run(CASK, "--app", "0.1.2", "57a", SHA).returncode, 2)
        self.assertEqual(run(CASK, "--app", "0.1.2", "57", "abc").returncode, 2)
        self.assertEqual(run(CASK, "--app", "0.1.2", SHA).returncode, 2)

    def test_the_cli_cask_is_unchanged(self):
        out = run(CASK, "0.1.2", SHA)
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertIn('cask "cox" do', out.stdout)
        self.assertIn("cox-aarch64-apple-darwin.tar.xz", out.stdout)


class Refusals(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        self.dmg = self.tmp / "Cox-0.1.2-1-arm64.dmg"
        self.dmg.write_bytes(b"not a dmg")

    def test_notarize_needs_all_three_app_store_connect_secrets(self):
        out = run(HERE / "notarize.sh", self.dmg, env={"NOTARY_KEY": "a2V5", "NOTARY_KEY_ID": "K"})
        self.assertEqual(out.returncode, 2)
        self.assertIn("NOTARY_ISSUER", out.stderr)
        self.assertNotIn("a2V5", out.stdout + out.stderr)

    def test_the_appcast_needs_the_eddsa_key_and_a_release_tag(self):
        self.assertEqual(run(HERE / "appcast.sh", self.dmg, "v0.1.2").returncode, 2)
        out = run(HERE / "appcast.sh", self.dmg, "v0.1.2/../x", env={"SPARKLE_ED_KEY": "k"})
        self.assertEqual(out.returncode, 2)
        self.assertIn("not a release tag", out.stderr)

    def test_a_release_app_build_needs_the_cli(self):
        out = run(HERE / "app.sh", env={"COX_CONFIGURATION": "Release"})
        self.assertEqual(out.returncode, 1)
        self.assertIn("COX_CLI", out.stderr)

    def test_a_release_dmg_needs_the_bundled_cli(self):
        scripts = self.tmp / "repo/scripts/desktop"
        scripts.mkdir(parents=True)
        shutil.copy(HERE / "dmg.sh", scripts)
        (self.tmp / "repo/desktop/macos/build/Cox.app/Contents").mkdir(parents=True)
        out = run(scripts / "dmg.sh", env={"COX_CONFIGURATION": "Release"})
        self.assertEqual(out.returncode, 1)
        self.assertIn("no bundled CLI", out.stderr)


def throwaway_key():
    """A fresh Ed25519 pair in Sparkle's format: the base64 seed and the base64 public key."""
    der = subprocess.run(
        ["openssl", "genpkey", "-algorithm", "ed25519", "-outform", "DER"],
        capture_output=True, check=True,
    ).stdout
    public = subprocess.run(
        ["openssl", "pkey", "-inform", "DER", "-pubout", "-outform", "DER"],
        input=der, capture_output=True, check=True,
    ).stdout
    return base64.b64encode(der[-32:]).decode(), base64.b64encode(public[-32:]).decode()


@unittest.skipUnless((SPARKLE_BIN / "generate_appcast").exists(), "Sparkle tools not resolved")
class Appcast(unittest.TestCase):
    def test_the_appcast_is_signed_and_points_at_the_release_asset(self):
        tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, tmp, ignore_errors=True)
        private, public = throwaway_key()
        app = tmp / "stage/Cox.app/Contents"
        (app / "MacOS").mkdir(parents=True)
        shutil.copy("/usr/bin/true", app / "MacOS/Cox")
        with open(app / "Info.plist", "wb") as plist:
            plistlib.dump(
                {
                    "CFBundleExecutable": "Cox",
                    "CFBundleIdentifier": "io.github.pyrlyn.cox",
                    "CFBundlePackageType": "APPL",
                    "CFBundleShortVersionString": "0.1.2",
                    "CFBundleVersion": "57",
                    "LSMinimumSystemVersion": "26.0",
                    "SUPublicEDKey": public,
                    "SURequireSignedFeed": True,
                },
                plist,
            )
        subprocess.run(["codesign", "--force", "--sign", "-", tmp / "stage/Cox.app"], check=True)
        dmg = tmp / "Cox-0.1.2-57-arm64.dmg"
        subprocess.run(
            ["hdiutil", "create", "-quiet", "-volname", "Cox", "-srcfolder", tmp / "stage",
             "-format", "UDZO", dmg],
            check=True,
        )
        out = run(HERE / "appcast.sh", dmg, "v0.1.2", env={"SPARKLE_ED_KEY": private})
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)
        feed = (tmp / "appcast.xml").read_text()
        self.assertIn(
            "https://github.com/pyrlyn/cox/releases/download/v0.1.2/Cox-0.1.2-57-arm64.dmg", feed
        )
        self.assertIn("<sparkle:version>57</sparkle:version>", feed)
        self.assertNotIn(private, out.stdout + out.stderr + feed)

        # A feed edited after signing no longer verifies.
        (tmp / "appcast.xml").write_text(feed.replace("0.1.2", "9.9.9"))
        tampered = subprocess.run(
            [SPARKLE_BIN / "sign_update", "--verify", "--ed-key-file", "-", tmp / "appcast.xml"],
            input=private, capture_output=True, text=True,
        )
        self.assertNotEqual(tampered.returncode, 0)


if __name__ == "__main__":
    unittest.main()
