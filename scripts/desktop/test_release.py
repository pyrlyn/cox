# Copyright (c) 2026 Ivan Tugay
# SPDX-License-Identifier: GPL-3.0-or-later
# Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

"""Tests for the macOS app's release scripts (T37.32.2): app.sh, sign.sh, dmg.sh, notarize.sh,
appcast.sh and the app cask from scripts/cask.sh.

sign.sh and dmg.sh run for real, ad-hoc, on a small app bundle built here from C stubs, so the
nested-code order, the bundled CLI's runtime and entitlements, and the DMG name are what
codesign and hdiutil really produce; app.sh runs with xcodegen and xcodebuild faked and the
same real signing. notarize.sh and appcast.sh talk to Apple's notary service
and to Sparkle's tools, so those run against fakes on PATH (and SPARKLE_BIN) that log their
arguments and standard input: the tests check the control flow, that a rejection fails the run,
and that no secret ever reaches an argument list.

    python3 -I -m unittest discover -s scripts/desktop -p 'test_release.py'
"""

import base64
import os
import plistlib
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
SHA = "0123456789abcdef" * 4
ON_MACOS = sys.platform == "darwin" and shutil.which("codesign") and shutil.which("cc")


def run(args, env=None, stdin=None):
    full = {**os.environ, **(env or {})}
    return subprocess.run(
        args, env=full, input=stdin, capture_output=True, text=True, check=False)


def write_exe(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def compile_stub(out: Path, dylib: bool = False) -> None:
    out.parent.mkdir(parents=True, exist_ok=True)
    flags = ["-dynamiclib"] if dylib else []
    subprocess.run(
        ["cc", "-arch", "arm64", *flags, "-x", "c", "-", "-o", str(out)],
        input="int stub(void) { return 0; }\nint main(void) { return 0; }\n",
        text=True, check=True)


def make_app(app: Path, helper: bool = True, sparkle: bool = True) -> None:
    """A minimal Cox.app: a main executable, the bundled CLI and a Sparkle-shaped framework."""
    contents = app / "Contents"
    (contents / "MacOS").mkdir(parents=True)
    with open(contents / "Info.plist", "wb") as f:
        plistlib.dump({
            "CFBundleIdentifier": "io.github.pyrlyn.cox", "CFBundleExecutable": "Cox",
            "CFBundlePackageType": "APPL", "CFBundleShortVersionString": "0.1.3",
            "CFBundleVersion": "42"}, f)
    compile_stub(contents / "MacOS" / "Cox")
    if helper:
        compile_stub(contents / "Helpers" / "cox")
    if sparkle:
        fw = contents / "Frameworks" / "Sparkle.framework"
        b = fw / "Versions" / "B"
        compile_stub(b / "Sparkle", dylib=True)
        compile_stub(b / "Autoupdate")
        (b / "Resources").mkdir(parents=True)
        with open(b / "Resources" / "Info.plist", "wb") as f:
            plistlib.dump({
                "CFBundleIdentifier": "org.sparkle-project.Sparkle",
                "CFBundleExecutable": "Sparkle", "CFBundlePackageType": "FMWK"}, f)
        (fw / "Versions" / "Current").symlink_to("B")
        (fw / "Sparkle").symlink_to("Versions/Current/Sparkle")
        (fw / "Resources").symlink_to("Versions/Current/Resources")


def display(path: Path) -> str:
    out = subprocess.run(
        ["codesign", "--display", "--verbose=2", "--entitlements", "-", str(path)],
        capture_output=True, text=True, check=True)
    return out.stdout + out.stderr


class CaskTests(unittest.TestCase):
    def cask(self, *args):
        return run(["bash", str(ROOT / "scripts" / "cask.sh"), *args])

    def test_app_cask_installs_the_release_dmg_and_links_the_bundled_cli(self):
        out = self.cask("--app", "v0.1.3", SHA)
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertIn('cask "cox-app" do', out.stdout)
        self.assertIn('version "0.1.3"', out.stdout)
        self.assertIn(f'sha256 "{SHA}"', out.stdout)
        self.assertIn("/releases/download/v#{version}/Cox-#{version}-arm64.dmg", out.stdout)
        self.assertIn('app "Cox.app"', out.stdout)
        self.assertIn('binary "#{appdir}/Cox.app/Contents/Helpers/cox"', out.stdout)
        self.assertIn("auto_updates true", out.stdout)
        self.assertNotIn("~/.cox", out.stdout)

    def test_cli_cask_is_unchanged_without_the_flag(self):
        out = self.cask("0.1.3", SHA)
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertIn('cask "cox" do', out.stdout)
        self.assertIn("cox-aarch64-apple-darwin.tar.xz", out.stdout)

    def test_app_cask_refuses_a_bad_checksum(self):
        out = self.cask("--app", "0.1.3", "nope")
        self.assertEqual(out.returncode, 2)
        self.assertEqual(out.stdout, "")


@unittest.skipUnless(ON_MACOS, "needs codesign and cc")
class SignTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)
        self.app = self.tmp / "Cox.app"

    def sign(self):
        env = {"COX_SIGN_IDENTITY": "", "SIGNING_KEYCHAIN": ""}
        return run(["bash", str(HERE / "sign.sh"), f"{self.app}/"], env=env)

    def test_bundled_cli_gets_the_runtime_its_identifier_and_the_plugin_entitlement(self):
        make_app(self.app)
        out = self.sign()
        self.assertEqual(out.returncode, 0, out.stderr)
        helper = display(self.app / "Contents" / "Helpers" / "cox")
        self.assertRegex(helper, r"flags=0x[0-9a-f]+\([^)]*runtime")
        self.assertIn("Identifier=io.github.pyrlyn.cox.cli", helper)
        self.assertIn("com.apple.security.cs.allow-unsigned-executable-memory", helper)

    def test_sparkle_autoupdate_is_signed_before_its_framework(self):
        make_app(self.app)
        autoupdate = self.app / "Contents/Frameworks/Sparkle.framework/Versions/B/Autoupdate"
        self.assertIn("linker-signed", display(autoupdate))
        out = self.sign()
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertNotIn("linker-signed", display(autoupdate))
        strict = run(["codesign", "--verify", "--deep", "--strict", str(self.app)])
        self.assertEqual(strict.returncode, 0, strict.stderr)

    def test_app_without_a_cli_signs_as_before(self):
        make_app(self.app, helper=False, sparkle=False)
        out = self.sign()
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertFalse((self.app / "Contents" / "Helpers").exists())


@unittest.skipUnless(ON_MACOS and shutil.which("hdiutil"), "needs codesign, cc and hdiutil")
class DmgTests(unittest.TestCase):
    """dmg.sh against a scratch copy of the repository layout it expects."""

    def setUp(self):
        self.root = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.root)
        (self.root / "scripts" / "desktop").mkdir(parents=True)
        for name in ("dmg.sh", "sign.sh"):
            shutil.copy(HERE / name, self.root / "scripts" / "desktop" / name)
        shutil.copy(ROOT / "scripts" / "cox.entitlements", self.root / "scripts")
        self.build = self.root / "desktop" / "macos" / "build"
        make_app(self.build / "Cox.app", sparkle=False)

    def dmg(self, configuration):
        env = {"COX_SIGN_IDENTITY": "", "SIGNING_KEYCHAIN": "", "COX_CONFIGURATION": configuration}
        return run(["bash", str(self.root / "scripts" / "desktop" / "dmg.sh")], env=env)

    def test_release_dmg_is_named_by_version_alone(self):
        out = self.dmg("Release")
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertEqual(out.stdout.strip().splitlines()[-1], str(self.build / "Cox-0.1.3-arm64.dmg"))

    def test_debug_dmg_keeps_its_build_number_and_tag(self):
        out = self.dmg("Debug")
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertTrue((self.build / "Cox-0.1.3-42-debug-arm64.dmg").is_file())

    def test_unknown_configuration_is_refused_before_signing(self):
        out = self.dmg("Profile")
        self.assertEqual(out.returncode, 2)
        self.assertEqual(list(self.build.glob("*.dmg")), [])


FAKE_XCODEBUILD = """#!/bin/bash
echo "xcodebuild $*" >> "$FAKE_LOG"
config=""; dd=""; prev=""
for a in "$@"; do
  [ "$prev" = -configuration ] && config="$a"
  [ "$prev" = -derivedDataPath ] && dd="$a"
  prev="$a"
done
mkdir -p "$dd/Build/Products/$config"
ditto "$FAKE_APP" "$dd/Build/Products/$config/Cox.app"
"""


@unittest.skipUnless(ON_MACOS, "needs codesign and cc")
class AppTests(unittest.TestCase):
    """app.sh against a scratch layout, with xcodegen and xcodebuild faked."""

    def setUp(self):
        self.root = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.root)
        (self.root / "scripts" / "desktop").mkdir(parents=True)
        for name in ("app.sh", "sign.sh"):
            shutil.copy(HERE / name, self.root / "scripts" / "desktop" / name)
        shutil.copy(ROOT / "scripts" / "cox.entitlements", self.root / "scripts")
        (self.root / "Cargo.toml").write_text(
            '[workspace]\nmembers = []\n\n[workspace.package]\nversion = "9.8.7"\n'
            'edition = "2024"\n\n[workspace.dependencies]\nversion = "nope"\n')
        self.build = self.root / "desktop" / "macos" / "build"
        (self.build / "CoxFFI.xcframework").mkdir(parents=True)
        self.bin = self.root / "bin"
        write_exe(self.bin / "xcodegen", '#!/bin/bash\necho "xcodegen $*" >> "$FAKE_LOG"\n')
        write_exe(self.bin / "xcodebuild", FAKE_XCODEBUILD)
        self.template = self.root / "template" / "Cox.app"
        make_app(self.template, helper=False, sparkle=False)
        self.cli = self.root / "cox"
        compile_stub(self.cli)
        self.log = self.root / "log"
        self.log.touch()

    def app(self, **env):
        base = {
            "PATH": f"{self.bin}:{os.environ['PATH']}", "FAKE_LOG": str(self.log),
            "FAKE_APP": str(self.template), "COX_SIGN_IDENTITY": "", "SIGNING_KEYCHAIN": ""}
        base.update(env)
        return run(["bash", str(self.root / "scripts" / "desktop" / "app.sh")], env=base)

    def test_release_build_bundles_the_cli_under_the_workspace_version(self):
        out = self.app(COX_CONFIGURATION="Release", COX_CLI=str(self.cli))
        self.assertEqual(out.returncode, 0, out.stderr)
        log = self.log.read_text()
        self.assertIn("-configuration Release", log)
        self.assertIn("MARKETING_VERSION=9.8.7", log)
        helper = self.build / "Cox.app" / "Contents" / "Helpers" / "cox"
        self.assertRegex(display(helper), r"flags=0x[0-9a-f]+\([^)]*runtime")
        strict = run(["codesign", "--verify", "--deep", "--strict", str(self.build / "Cox.app")])
        self.assertEqual(strict.returncode, 0, strict.stderr)

    def test_debug_build_without_a_cli_has_no_helpers(self):
        out = self.app()
        self.assertEqual(out.returncode, 0, out.stderr)
        self.assertIn("-configuration Debug", self.log.read_text())
        self.assertFalse((self.build / "Cox.app" / "Contents" / "Helpers").exists())

    def test_a_cli_that_is_not_an_executable_is_refused_before_the_build(self):
        out = self.app(COX_CLI=str(self.root / "missing"))
        self.assertEqual(out.returncode, 1)
        self.assertEqual(self.log.read_text(), "")


FAKE_XCRUN = """#!/bin/bash
key=""; prev=""
for a in "$@"; do [ "$prev" = --key ] && key="$a"; prev="$a"; done
echo "xcrun $1 $2 key-mode=$( [ -n "$key" ] && stat -f %Lp "$key" ) key=$( [ -n "$key" ] && cat "$key" )" >> "$FAKE_LOG"
echo "argv: $*" >> "$FAKE_LOG"
case "$1 $2" in
  "notarytool submit") printf '{"id":"sub-1","status":"%s"}' "$FAKE_STATUS" ;;
esac
"""

FAKE_SPCTL = """#!/bin/bash
echo "spctl $*" >> "$FAKE_LOG"
echo "${@: -1}: accepted"
echo "source=${FAKE_SOURCE:-Notarized Developer ID}"
"""

FAKE_HDIUTIL = """#!/bin/bash
echo "hdiutil $1" >> "$FAKE_LOG"
"""


class NotarizeTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)
        self.bin = self.tmp / "bin"
        write_exe(self.bin / "xcrun", FAKE_XCRUN)
        write_exe(self.bin / "spctl", FAKE_SPCTL)
        write_exe(self.bin / "hdiutil", FAKE_HDIUTIL)
        self.log = self.tmp / "log"
        self.log.touch()
        self.dmg = self.tmp / "Cox-0.1.3-arm64.dmg"
        self.dmg.write_bytes(b"dmg")
        self.secret = "-----BEGIN PRIVATE KEY-----\nsecret-key\n"

    def notarize(self, dmg=None, **env):
        base = {
            "PATH": f"{self.bin}:{os.environ['PATH']}", "FAKE_LOG": str(self.log),
            "FAKE_STATUS": "Accepted",
            "NOTARY_KEY": base64.b64encode(self.secret.encode()).decode(),
            "NOTARY_KEY_ID": "KEYID", "NOTARY_ISSUER": "ISSUER"}
        base.update(env)
        return run(["bash", str(HERE / "notarize.sh"), str(dmg or self.dmg)], env=base)

    def test_accepted_dmg_is_stapled_and_assessed(self):
        out = self.notarize()
        self.assertEqual(out.returncode, 0, out.stderr)
        log = self.log.read_text()
        calls = [line.split(" key-mode")[0] for line in log.splitlines() if line.startswith("xcrun")]
        self.assertEqual(calls, [
            "xcrun notarytool submit", "xcrun stapler staple", "xcrun stapler validate"])
        self.assertIn("key-mode=600 key=-----BEGIN PRIVATE KEY-----", log)
        self.assertIn("--type open", log)
        self.assertIn("--type execute", log)
        self.assertNotIn("secret-key", "".join(l for l in log.splitlines() if l.startswith("argv")))

    def test_rejected_submission_fetches_the_log_and_fails(self):
        out = self.notarize(FAKE_STATUS="Invalid")
        self.assertEqual(out.returncode, 1)
        log = self.log.read_text()
        self.assertIn("xcrun notarytool log", log)
        self.assertNotIn("stapler", log)

    def test_gatekeeper_must_name_a_notarized_developer_id(self):
        out = self.notarize(FAKE_SOURCE="Unnotarized Developer ID")
        self.assertEqual(out.returncode, 1)
        self.assertIn("does not accept", out.stderr)

    def test_missing_credentials_fail_before_any_submission(self):
        out = self.notarize(NOTARY_KEY_ID="")
        self.assertEqual(out.returncode, 1)
        self.assertEqual(self.log.read_text(), "")

    def test_debug_dmg_is_refused(self):
        debug = self.tmp / "Cox-0.1.3-42-debug-arm64.dmg"
        debug.write_bytes(b"dmg")
        out = self.notarize(dmg=debug)
        self.assertEqual(out.returncode, 2)
        self.assertEqual(self.log.read_text(), "")


FAKE_GENERATE_APPCAST = """#!/bin/bash
echo "generate_appcast stdin=$(cat)" >> "$FAKE_LOG"
echo "argv: $*" >> "$FAKE_LOG"
prefix=""; out=""; prev=""
for a in "$@"; do
  [ "$prev" = --download-url-prefix ] && prefix="$a"
  [ "$prev" = -o ] && out="$a"
  prev="$a"
done
dir="${@: -1}"
name="$(cd "$dir" && ls *.dmg)"
cat > "$out" <<EOF
<?xml version="1.0" standalone="yes"?>
<rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle" version="2.0">
  <channel><title>Cox</title>
    <item><title>$FAKE_SHORT</title><sparkle:version>42</sparkle:version>
      <sparkle:shortVersionString>$FAKE_SHORT</sparkle:shortVersionString>
      <enclosure url="$prefix$name" length="3" type="application/octet-stream" sparkle:edSignature="c2ln"/>
    </item>
  </channel>
</rss>
EOF
"""

FAKE_SIGN_UPDATE = """#!/bin/bash
echo "sign_update stdin=$(cat)" >> "$FAKE_LOG"
echo "argv: $*" >> "$FAKE_LOG"
[ -z "${FAKE_VERIFY_FAIL:-}" ]
"""


@unittest.skipUnless(shutil.which("xmllint"), "needs xmllint")
class AppcastTests(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)
        self.bin = self.tmp / "sparkle"
        write_exe(self.bin / "generate_appcast", FAKE_GENERATE_APPCAST)
        write_exe(self.bin / "sign_update", FAKE_SIGN_UPDATE)
        self.log = self.tmp / "log"
        self.log.touch()
        self.dmg = self.tmp / "Cox-0.1.3-arm64.dmg"
        self.dmg.write_bytes(b"dmg")
        self.out = self.tmp / "appcast.xml"

    def appcast(self, tag="v0.1.3", **env):
        base = {
            "SPARKLE_BIN": str(self.bin), "APPCAST_OUT": str(self.out),
            "FAKE_LOG": str(self.log), "FAKE_SHORT": "0.1.3", "SPARKLE_ED_KEY": "private-ed-key"}
        base.update(env)
        return run(["bash", str(HERE / "appcast.sh"), str(self.dmg), tag], env=base)

    def test_feed_names_the_release_download_and_both_signatures_verify(self):
        out = self.appcast()
        self.assertEqual(out.returncode, 0, out.stderr)
        feed = self.out.read_text()
        self.assertIn(
            'url="https://github.com/pyrlyn/cox/releases/download/v0.1.3/Cox-0.1.3-arm64.dmg"',
            feed)
        log = self.log.read_text().splitlines()
        self.assertEqual(sum(l.startswith("sign_update stdin=private-ed-key") for l in log), 2)
        self.assertIn(f"argv: --verify --ed-key-file - {self.dmg} c2ln", log)
        self.assertIn(f"argv: --verify --ed-key-file - {self.out}", log)

    def test_the_private_key_never_reaches_an_argument_list(self):
        self.appcast()
        argv = [l for l in self.log.read_text().splitlines() if l.startswith("argv")]
        self.assertTrue(argv)
        self.assertFalse(any("private-ed-key" in l for l in argv))

    def test_a_feed_for_another_version_is_refused(self):
        out = self.appcast(FAKE_SHORT="0.1.2")
        self.assertEqual(out.returncode, 1)
        self.assertIn("short version is 0.1.2", out.stderr)

    def test_a_signature_that_does_not_verify_fails(self):
        out = self.appcast(FAKE_VERIFY_FAIL="1")
        self.assertEqual(out.returncode, 1)
        self.assertIn("does not verify", out.stderr)

    def test_a_test_tag_gets_no_feed(self):
        out = self.appcast(tag="v0.1.3-test.1")
        self.assertEqual(out.returncode, 2)
        self.assertFalse(self.out.exists())

    def test_missing_key_fails_before_the_tools_run(self):
        out = self.appcast(SPARKLE_ED_KEY="")
        self.assertEqual(out.returncode, 1)
        self.assertEqual(self.log.read_text(), "")


if __name__ == "__main__":
    unittest.main()
