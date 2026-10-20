"""Exercise verification behavior without downloads or a Docker daemon."""
import gzip
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


SCRIPTS = Path(__file__).resolve().parent


class VerificationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="mina-verification-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.env = dict(os.environ, PATH=f"{self.bin}:{os.environ['PATH']}")
        for name in ("PACKAGES", "IMAGES", "AUTOMODE", "CODENAMES", "VERIFY_MODE"):
            self.env.pop(name, None)
        self.env["OUTPUT_DIR"] = str(self.root / "results")

    def executable(self, name, text):
        path = self.bin / name
        path.write_text(text)
        path.chmod(0o755)

    def run_script(self, name, *args):
        return subprocess.run(
            [str(SCRIPTS / name), *args], cwd=self.root, env=self.env,
            text=True, capture_output=True, timeout=20,
        )

    def mock_curl(self, release_codename):
        # A response larger than the pipe buffer reproduces grep -q closing the
        # pipe while curl is still writing. The response must be fully consumed.
        packages = gzip.compress(b"Package: mina-devnet\nVersion: 1\n")
        self.executable("curl", f'''#!/usr/bin/env python3
import sys
if '-w' in sys.argv:
    sys.stdout.write('200')
elif sys.argv[-1].endswith('/Release'):
    sys.stdout.write('Codename: {release_codename}\\n' + '# padding\\n' * 50000)
elif sys.argv[-1].endswith('/Packages.gz'):
    sys.stdout.buffer.write({packages!r})
else:
    sys.stdout.write('{{"count": 1}}')
''')

    def results(self):
        return [line.split("\t") for line in
                (self.root / "results/results.tsv").read_text().splitlines()]

    def test_canary_consumes_large_valid_release(self):
        self.mock_curl("noble")
        result = self.run_script("check-infra.sh", "--codenames", "noble")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("checks=8 failures=0", result.stdout)

    def test_canary_rejects_wrong_codename(self):
        self.mock_curl("jammy")
        result = self.run_script("check-infra.sh", "--codenames", "noble")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("does not declare codename noble", result.stdout)

    def test_default_runner_covers_both_networks_and_supported_distributions(self):
        self.executable("docker", "#!/bin/bash\necho 'INSTALLED-VERSION: 1.0'\n")
        self.env["VERIFY_MODE"] = "artifacts"
        result = self.run_script("run-verification.sh")
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        rows = self.results()
        self.assertEqual(len(rows), 27)
        self.assertEqual({row[2] for row in rows}, {"noble", "bookworm", "jammy"})
        self.assertTrue(all(row[0] == "PASS" for row in rows))
        targets = {row[3] for row in rows}
        self.assertTrue({"mina-devnet@alpha", "mina-mainnet@stable",
                         "mina-devnet-automode@alpha", "mina-mainnet-automode@stable"}
                        .issubset(targets))

    def test_automode_reports_apt_download_error(self):
        self.executable("docker", """#!/bin/bash
echo 'E: Failed to fetch prefork.deb File has unexpected size'
exit 1
""")
        self.env.update(VERIFY_MODE="artifacts", CODENAMES="bookworm",
                        AUTOMODE="mina-mainnet-automode@stable")
        result = self.run_script("run-verification.sh")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        row, = self.results()
        self.assertEqual(row[0], "FAIL")
        self.assertIn("File has unexpected size", row[4])

    def test_automode_reports_unmet_dependency(self):
        self.executable("docker", """#!/bin/bash
echo 'The following packages have unmet dependencies:'
echo ' mina-devnet-automode : Depends: mina-devnet-prefork-mesa (= X) but Y is to be installed'
echo 'E: Unable to correct problems, you have held broken packages.'
exit 1
""")
        self.env.update(VERIFY_MODE="artifacts", CODENAMES="bookworm",
                        AUTOMODE="mina-devnet-automode")
        result = self.run_script("run-verification.sh")
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        row, = self.results()
        self.assertIn("mina-devnet-prefork-mesa (= X) but Y", row[4])


if __name__ == "__main__":
    unittest.main()
