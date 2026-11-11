"""Release console regression. Run with --exe <built or installed repomeow-cli.exe>."""
import argparse
from concurrent.futures import ThreadPoolExecutor
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import struct
import subprocess
import tempfile
from threading import Thread


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--exe", required=True, type=Path)
    args = parser.parse_args()
    exe = args.exe.resolve(strict=True)
    if os.name == "nt":
        binary = exe.read_bytes()
        pe = struct.unpack_from("<I", binary, 0x3C)[0]
        assert struct.unpack_from("<H", binary, pe + 24 + 68)[0] == 3, "Expected console subsystem"

    with tempfile.TemporaryDirectory(prefix="repomeow-console-") as temporary:
        root = Path(temporary)
        data = root / "data"
        env = dict(os.environ, REPOMEOW_DATA_DIR=str(data))
        calls = 0

        def call(*command, code=0, selected_env=env):
            nonlocal calls
            p = subprocess.run([str(exe), *map(str, command)], stdin=subprocess.DEVNULL,
                               capture_output=True, env=selected_env, timeout=60)
            assert p.returncode == code, (command, p.returncode, p.stdout, p.stderr)
            stdout = p.stdout.decode("utf-8", errors="strict")
            stderr = p.stderr.decode("utf-8", errors="strict")
            calls += 1
            if code:
                assert not stdout, (command, stdout)
                failure = json.loads(stderr)
                assert set(failure) == {"code", "message", "detail"}
                return failure
            assert not stderr, (command, stderr)
            return stdout if command[0] in ("--help", "--version") else json.loads(stdout)

        call("--version")
        call("--help")
        assert not data.exists(), "Help/version should not initialize business data"
        assert call("project", "add", code=2)["code"] == "invalid_arguments"
        assert not data.exists(), "Parse failures should not publish invalidation"
        assert call("unknown-command", code=2)["code"] == "invalid_arguments"
        assert call("project", "get", "-p", root / "missing", code=1)["code"] == "project_not_found"
        project = root / "中文项目"
        project.mkdir()
        added = call("project", "add", "-p", project, "--name", "中文项目", "--description", "初始")
        assert "中文项目" in json.dumps(added, ensure_ascii=False)
        call("project", "update", "-p", project, "--name", "更新名称", "--description", "更新")
        assert "更新名称" in json.dumps(call("project", "list"), ensure_ascii=False)
        tag = call("tag", "create", "--name", "中文标签")
        call("tag", "update", "--id", tag["id"], "--name", "中文标签", "--color", "#ff0000")
        call("tag", "set-project", "-p", project, "--tag-ids", tag["id"])
        script = call("script", "create", "-p", project, "--name", "custom", "--command", "echo first")
        call("script", "update", "--id", script["id"], "--name", "custom", "--command", "echo updated")
        call("pin", "set", "-p", project, "--kind", "customCommand", "--target-key", script["id"],
             "--pinned", "true", "--label", "custom", "--command", "echo updated")
        call("hidden", "set", "-p", project, "--kind", "packageFile", "--target-key", "package.json", "--hidden", "true")
        call("hidden", "set", "-p", project, "--kind", "packageFile", "--target-key", "package.json", "--hidden", "false")
        call("script", "delete", "--id", script["id"])
        before_failure = (data / "projects.db").stat().st_size
        call("tag", "create", "--name", "中文标签", code=1)
        assert (data / "projects.db").stat().st_size >= before_failure
        call("project", "archive", "-p", project)
        assert not call("project", "list")["projects"]
        call("project", "unarchive", "-p", project)
        call("file", "save", "-p", project, "--path", "test.txt", "--content", "中文 UTF-8")
        assert (project / "test.txt").read_bytes() == "中文 UTF-8".encode("utf-8")
        call("file", "save", "-p", project, "--path", "../escape.txt", "--content", "bad", code=1)
        assert not (root / "escape.txt").exists()
        config = call("ai", "config-get")
        valid_config = data / "ai-config.json"
        initial = valid_config.read_bytes()
        invalid = root / "invalid.json"
        invalid.write_text("{", encoding="utf-8")
        call("ai", "config-save", "--file", invalid, code=1)
        assert valid_config.read_bytes() == initial
        valid = root / "config.json"
        valid.write_text(json.dumps(config), encoding="utf-8")
        call("ai", "config-save", "--file", valid)
        schedules = root / "schedules.json"
        schedules.write_text("[]", encoding="utf-8")
        call("report", "schedules-save", "--file", schedules)
        call("report", "system-schedule-save", "--enabled", "false", "--interval-minutes", "3")
        call("ai", "usage-clear")
        class AccountFixture(BaseHTTPRequestHandler):
            def do_GET(self):
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(b'{"username":"local-fixture"}')

            def log_message(self, *_):
                pass

        with ThreadingHTTPServer(("127.0.0.1", 0), AccountFixture) as server:
            thread = Thread(target=server.serve_forever)
            thread.start()
            try:
                account = call("account", "add", "--provider", "gitlab", "--label", "fixture",
                               "--base-url", f"http://127.0.0.1:{server.server_port}", "--token", "test-only-token")
                call("account", "update", "--id", account["id"], "--label", "updated")
                call("account", "remove", "--id", account["id"])
                assert not call("account", "list")["accounts"]
            finally:
                server.shutdown()
                thread.join(timeout=5)
        call("wiki", "config-set", "-p", project, "--concurrency", "3")
        call("wiki", "delete", "-p", project)

        # Independent console processes contend on the same data root.
        with ThreadPoolExecutor(max_workers=8) as pool:
            list(pool.map(lambda n: call("tag", "create", "--name", f"parallel-{n}"), range(8)))
        assert len(call("tag", "list")["tags"]) == 9
        commit, report = root / "commit.md", root / "report.md"
        commit.write_text("提交提示词", encoding="utf-8")
        report.write_text("报告提示词", encoding="utf-8")
        with ThreadPoolExecutor(max_workers=2) as pool:
            a = pool.submit(call, "prompt", "set", "--commit-file", commit)
            b = pool.submit(call, "prompt", "set", "--report-file", report)
            a.result(), b.result()
        prompts = call("prompt", "get")
        assert prompts["commit"] == "提交提示词" and prompts["report"] == "报告提示词"
        with ThreadPoolExecutor(max_workers=4) as pool:
            list(pool.map(lambda n: call("file", "save", "-p", project, "--path", "test.txt",
                                        "--content", str(n) * 10000), range(4)))
        assert (project / "test.txt").read_text(encoding="utf-8") in [str(n) * 10000 for n in range(4)]
        other_env = dict(env, REPOMEOW_DATA_DIR=str(root / "other"))
        assert not call("project", "list", selected_env=other_env)["projects"]
        assert call("project", "list")["projects"]
        if os.name == "nt":
            # cmd preserves native UTF-8 bytes and inherits the installed PATH entry.
            shell_env = dict(env, PATH=str(exe.parent) + os.pathsep + env.get("PATH", ""))
            for command, code in [("project list", 0), ("project add", 2),
                                  ("project get -p missing-project", 1)]:
                p = subprocess.run(f'cmd /d /s /c "repomeow-cli {command} >out.json 2>err.json"',
                                   env=shell_env, cwd=root, stdin=subprocess.DEVNULL, timeout=60)
                assert p.returncode == code
                out = (root / "out.json").read_bytes().decode("utf-8", errors="strict")
                err = (root / "err.json").read_bytes().decode("utf-8", errors="strict")
                if code:
                    assert not out and json.loads(err)["code"]
                else:
                    assert not err and json.loads(out)["projects"][0]["name"] == "更新名称"
                calls += 1
        revisions = json.loads((data / "cli-revisions.json").read_text(encoding="utf-8"))
        assert all(domain in revisions for domain in ["project", "tag", "file", "wiki", "ai", "prompt"])
        call("tag", "delete", "--id", tag["id"])
        call("project", "delete", "-p", project)
        assert not call("project", "list")["projects"]
        print(json.dumps({"passed": True, "calls": calls, "exe": str(exe), "tag": tag}, ensure_ascii=True))


if __name__ == "__main__":
    main()
