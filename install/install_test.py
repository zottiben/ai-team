"""Offline installer regressions; every installation target and tool is isolated."""

import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest


INSTALLER = Path(__file__).with_name("install.sh").resolve()
VERSION = "0.7.5"
ASSET = f"ai-team-v{VERSION}-macos-universal.tar.gz"


def executable(path, content):
    path.write_text(content)
    path.chmod(0o755)


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="ai-team-installer-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.home = self.root / "home"
        self.bin = self.home / ".local/bin"
        self.tools = self.root / "tools"
        self.app = self.root / "Applications/ai-team.app"
        for directory in (self.bin, self.tools, self.app, self.root / "tmp"):
            directory.mkdir(parents=True)
        executable(self.bin / "ait", "#!/bin/sh\necho 'ait 0.7.4'\n")
        (self.app / "old-app").write_text("keep the old app")
        self.env = {
            "PATH": f"{self.tools}:{self.bin}:{os.environ['PATH']}",
            "HOME": str(self.home),
            "TMPDIR": str(self.root / "tmp"),
            "AI_TEAM_APP_DIR": str(self.app),
            "FIXTURE": str(self.root),
            "REAL_CP": shutil.which("cp"),
            "REAL_RM": shutil.which("rm"),
            "REAL_MV": shutil.which("mv"),
        }
        executable(self.tools / "uname", "#!/bin/sh\ncase \"$1\" in -s) echo \"${FAKE_OS:-Darwin}\";; -m) echo \"${FAKE_ARCH:-arm64}\";; esac\n")
        executable(self.tools / "sudo", "#!/bin/sh\nexit 98\n")
        executable(self.tools / "cargo", "#!/bin/sh\ntouch \"$FIXTURE/cargo-called\"\nexit 0\n")
        executable(self.tools / "curl", """#!/usr/bin/env python3
import os, pathlib, shutil, sys
root = pathlib.Path(os.environ['FIXTURE'])
args = sys.argv[1:]
url = next(a for a in args if a.startswith('https://'))
with (root / 'curl-calls').open('a') as log: log.write(url + '\\n')
mode = os.environ.get('MODE', '')
if 'api.github.com' in url or mode == 'lookup-failure': sys.exit(22)
if url.endswith('/releases/latest'):
    print('https://example.invalid/tag/v0.7.5' if mode == 'bad-redirect' else
          'https://github.com/zottiben/ai-team/releases/tag/v0.7.5', end='')
    sys.exit(0)
if mode == 'download-failure': sys.exit(22)
if mode == 'checksum-failure' and url.endswith('/checksums.txt'): sys.exit(22)
name = url.rsplit('/', 1)[1]
source = root / name
if not source.is_file(): sys.exit(97)
shutil.copyfile(source, args[args.index('-o') + 1])
""")
        # Fail closed even if a regression ignores the alternate app directory.
        executable(self.tools / "cp", """#!/usr/bin/env python3
import os, pathlib, subprocess, sys
root = pathlib.Path(os.environ['FIXTURE'])
paths = [pathlib.Path(a).resolve() for a in sys.argv[1:] if not a.startswith('-')]
if not all(p.is_relative_to(root) for p in paths): sys.exit(96)
if os.environ.get('MODE') == 'copy-failure': sys.exit(1)
sys.exit(subprocess.call([os.environ['REAL_CP'], *sys.argv[1:]]))
""")
        executable(self.tools / "rm", """#!/usr/bin/env python3
import os, pathlib, subprocess, sys
root = pathlib.Path(os.environ['FIXTURE'])
paths = [pathlib.Path(a).resolve() for a in sys.argv[1:] if not a.startswith('-')]
if not all(p.is_relative_to(root) for p in paths): sys.exit(96)
sys.exit(subprocess.call([os.environ['REAL_RM'], *sys.argv[1:]]))
""")
        executable(self.tools / "mv", """#!/usr/bin/env python3
import os, pathlib, subprocess, sys
root = pathlib.Path(os.environ['FIXTURE'])
paths = [pathlib.Path(a).resolve() for a in sys.argv[1:] if not a.startswith('-')]
if not all(p.is_relative_to(root) for p in paths): sys.exit(96)
mode = os.environ.get('MODE')
if mode == 'cli-swap-failure' and paths[-1] == root / 'home/.local/bin/ait': sys.exit(1)
if mode == 'app-swap-failure' and '.install.' in str(paths[0]): sys.exit(1)
sys.exit(subprocess.call([os.environ['REAL_MV'], *sys.argv[1:]]))
""")
        self.archive()

    def archive(self, *, app=True, version=VERSION):
        stage = self.root / "stage"
        stage.mkdir(exist_ok=True)
        executable(stage / "ait", f"#!/bin/sh\necho 'ait {version}'\n")
        with tarfile.open(self.root / ASSET, "w:gz") as archive:
            archive.add(stage / "ait", arcname="ait")
            if app:
                fresh = stage / "ai-team.app"
                fresh.mkdir(exist_ok=True)
                (fresh / "new-app").write_text(VERSION)
                archive.add(fresh, arcname="ai-team.app")
        digest = hashlib.sha256((self.root / ASSET).read_bytes()).hexdigest()
        (self.root / "checksums.txt").write_text(f"{digest}  {ASSET}\n")

    def run_installer(self, mode=""):
        return subprocess.run(
            ["sh", str(INSTALLER)], env={**self.env, "MODE": mode},
            cwd=self.root, text=True, capture_output=True, timeout=15,
        )

    def assert_preserved(self, result):
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertFalse((self.root / "cargo-called").exists())
        self.assertIn("0.7.4", (self.bin / "ait").read_text())
        self.assertEqual((self.app / "old-app").read_text(), "keep the old app")
        self.assertFalse((self.home / ".ai-team/install-method").exists())

    def test_release_lookup_failure_does_not_build_only_the_cli(self):
        self.assert_preserved(self.run_installer("lookup-failure"))

    def test_redirect_installs_both_without_the_rate_limited_api(self):
        result = self.run_installer()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(VERSION, (self.bin / "ait").read_text())
        self.assertEqual((self.app / "new-app").read_text(), VERSION)
        self.assertFalse((self.app / "old-app").exists())
        self.assertFalse((self.root / "cargo-called").exists())
        self.assertNotIn("api.github.com", (self.root / "curl-calls").read_text())
        self.assertEqual((self.home / ".ai-team/install-method").read_text(), "release\n")

    def test_download_failure_does_not_fall_back(self):
        self.assert_preserved(self.run_installer("download-failure"))

    def test_unexpected_redirect_is_refused(self):
        self.assert_preserved(self.run_installer("bad-redirect"))

    def test_missing_mac_app_is_not_a_successful_cli_only_update(self):
        self.archive(app=False)
        self.assert_preserved(self.run_installer())

    def test_failed_app_staging_preserves_both_installations(self):
        self.assert_preserved(self.run_installer("copy-failure"))

    def test_failed_cli_swap_restores_the_previous_app(self):
        self.assert_preserved(self.run_installer("cli-swap-failure"))

    def test_failed_app_swap_restores_the_previous_app(self):
        self.assert_preserved(self.run_installer("app-swap-failure"))

    def test_replacement_preserves_old_cli_inode_and_app_backup(self):
        old_inode = self.root / "old-cli"
        os.link(self.bin / "ait", old_inode)
        result = self.run_installer()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("0.7.4", old_inode.read_text())
        backups = list(self.app.parent.glob("ai-team.app.backup.*/ai-team.app/old-app"))
        self.assertEqual(len(backups), 1)
        self.assertEqual(backups[0].read_text(), "keep the old app")

    def test_wrong_cli_version_is_refused_before_replacement(self):
        self.archive(version="0.7.4")
        self.assert_preserved(self.run_installer())

    def test_missing_checksums_refuses_installation(self):
        self.assert_preserved(self.run_installer("checksum-failure"))

    def test_linux_archives_update_only_the_cli(self):
        for arch in ("x86_64", "aarch64"):
            with self.subTest(arch=arch):
                self.archive(app=False)
                name = f"ai-team-v{VERSION}-linux-{arch}.tar.gz"
                shutil.copyfile(self.root / ASSET, self.root / name)
                sums = self.root / "checksums.txt"
                sums.write_text(sums.read_text().replace(ASSET, name))
                self.env.update(FAKE_OS="Linux", FAKE_ARCH=arch)
                result = self.run_installer()
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                self.assertIn(VERSION, (self.bin / "ait").read_text())
                self.assertTrue((self.app / "old-app").exists())

    def test_bad_checksum_preserves_both_installations(self):
        (self.root / "checksums.txt").write_text(f"{'0' * 64}  {ASSET}\n")
        self.assert_preserved(self.run_installer())


if __name__ == "__main__":
    unittest.main()
