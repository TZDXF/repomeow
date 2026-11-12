"""Check CLI shell tests with GitHub Actions' PowerShell exit-code wrapper."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--exe", required=True, type=Path)
    args = parser.parse_args()
    exe = args.exe.resolve(strict=True)
    script = Path(__file__).with_name("test-cli.ps1").resolve(strict=True)

    def quote(path):
        return "'" + str(path).replace("'", "''") + "'"

    results = []
    for shell in ("powershell.exe", "pwsh.exe"):
        for scenario in ("success", "assertion_failure", "cleanup_failure"):
            with tempfile.TemporaryDirectory(prefix="repomeow-actions-") as temporary:
                root = Path(temporary)
                wrapper = root / "step.ps1"
                # Match the runner's prefix, invocation and suffix. Do not reset
                # LASTEXITCODE here: that would conceal the original regression.
                wrapper.write_text(
                    "$ErrorActionPreference = 'stop'\n"
                    + ("function Remove-Item { Write-Error 'Injected cleanup failure' }\n"
                       if scenario == "cleanup_failure" else "")
                    + f"& {quote(script)} -Exe {quote(exe)}"
                    + (" -InjectAssertionFailure" if scenario == "assertion_failure" else "")
                    + "\nif ((Test-Path -LiteralPath variable:\\LASTEXITCODE)) { exit $LASTEXITCODE }\n",
                    encoding="utf-8-sig",
                )
                env = dict(os.environ, TEMP=str(root), TMP=str(root))
                result = subprocess.run(
                    [shell, "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
                     "-Command", f". {quote(wrapper)}"],
                    capture_output=True, env=env, timeout=60,
                )
                stdout = result.stdout.decode("utf-8", errors="replace")
                stderr = result.stderr.decode("utf-8", errors="replace")
                if scenario != "success":
                    assert result.returncode != 0, (shell, "Failure was swallowed", stdout, stderr)
                    expected = "Injected " + scenario.replace("_", " ")
                    assert expected in stderr, (shell, stderr)
                    assert not stdout.strip(), (shell, "Failure reported success", stdout)
                else:
                    assert result.returncode == 0, (shell, result.returncode, stdout, stderr)
                    assert not stderr, (shell, stderr)
                    assert json.loads(stdout)["passed"] is True, (shell, stdout)
                remaining = list(root.glob("repomeow-shell-*"))
                if scenario == "cleanup_failure":
                    assert remaining, (shell, "Cleanup failure was not exercised")
                else:
                    assert not remaining, (shell, "Test data was not cleaned up")
                results.append(dict(shell=shell, scenario=scenario, exit=result.returncode))
    print(json.dumps(dict(passed=True, cases=results)))


if __name__ == "__main__":
    main()
