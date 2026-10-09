#!/usr/bin/env python3
"""browser-smoke — headless-Chrome console probe for the sepia web UI.

Opens each URL via chromedriver, waits for hydration, drains the
browser console, and prints SEVERE entries (panics, hydration
mismatches, failed fetches). Exits 1 if any are found — agents can
loop on it after UI changes without human eyes.

Usage:
    tools/browser-smoke.py                       # default localhost:3000, all routes
    tools/browser-smoke.py http://127.0.0.1:3000/settings
    tools/browser-smoke.py --shot /tmp/app.png URL  # also save a screenshot

Exit codes: 0 clean, 1 console SEVERE / navigation error, 2 setup problem.

Requires: chromedriver (SEPIA_CHROMEDRIVER, target/webdriver/chromedriver,
or PATH) and Chrome/Chromium (CHROME_BIN or PATH).
"""

import base64
import json
import os
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SETTLE_S = float(os.environ.get("SMOKE_SETTLE", "6"))

ROUTES = ["/", "/agents", "/projects", "/nodes", "/settings"]

USAGE = "usage: browser-smoke.py [--shot PNG] [URL ...]"


def die(msg):
    print(msg, file=sys.stderr)
    sys.exit(2)


def find_binary(names, env_var):
    """Resolve a binary: env override (validated), else PATH search."""
    env = os.environ.get(env_var)
    if env:
        if os.path.isfile(env) and os.access(env, os.X_OK):
            return env
        die(f"{env_var}={env} is not an executable file")
    for name in names:
        for d in os.environ.get("PATH", "").split(os.pathsep):
            p = os.path.join(d, name)
            if os.path.isfile(p) and os.access(p, os.X_OK):
                return p
    return None


def chromedriver_bin():
    env = os.environ.get("SEPIA_CHROMEDRIVER")
    if env:
        return find_binary([], "SEPIA_CHROMEDRIVER")
    local = os.path.join(ROOT, "target/webdriver/chromedriver")
    if os.path.isfile(local) and os.access(local, os.X_OK):
        return local
    return find_binary(["chromedriver"], "SEPIA_CHROMEDRIVER")


def req(method, url, body=None, timeout=30):
    r = urllib.request.Request(
        url,
        data=json.dumps(body).encode() if body is not None else None,
        headers={"Content-Type": "application/json"},
        method=method,
    )
    return json.load(urllib.request.urlopen(r, timeout=timeout))


def parse_args(argv):
    shot = None
    urls = []
    i = 0
    while i < len(argv):
        a = argv[i]
        if a == "--shot":
            if i + 1 >= len(argv):
                die(f"--shot needs a path\n{USAGE}")
            shot = argv[i + 1]
            i += 2
        elif a.startswith("--"):
            die(f"unknown flag {a}\n{USAGE}")
        else:
            urls.append(a)
            i += 1
    return shot, urls


def kill_tree(proc):
    """Terminate proc and anything it spawned (chromedriver → chrome)."""
    try:
        os.killpg(proc.pid, signal.SIGTERM)
    except (ProcessLookupError, PermissionError, OSError):
        proc.terminate()
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(proc.pid, signal.SIGKILL)
        except (ProcessLookupError, PermissionError, OSError):
            proc.kill()
        proc.wait(timeout=5)


def main():
    shot, urls = parse_args(sys.argv[1:])

    chrome = find_binary(
        ["google-chrome-stable", "google-chrome", "chromium", "chromium-browser"],
        "CHROME_BIN",
    )
    driver = chromedriver_bin()
    if not chrome or not driver:
        die("need chrome + chromedriver (CHROME_BIN / SEPIA_CHROMEDRIVER)")

    if not urls:
        urls = [f"http://localhost:3000{r}" for r in ROUTES]

    # Bind an ephemeral port ourselves — chromedriver's --port=0 prints
    # the choice on stderr only.
    sock = socket.socket()
    sock.bind(("127.0.0.1", 0))
    port = sock.getsockname()[1]
    sock.close()

    try:
        drv = subprocess.Popen(
            [driver, f"--port={port}", "--allowed-origins=*"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            start_new_session=True,  # own pgid so kill_tree reaches chrome
        )
    except OSError as e:
        die(f"cannot launch chromedriver: {e}")
    wd = f"http://127.0.0.1:{port}"
    for _ in range(100):
        if drv.poll() is not None:
            die(f"chromedriver exited immediately (rc={drv.returncode})")
        try:
            req("GET", f"{wd}/status")
            break
        except Exception:
            time.sleep(0.15)
    else:
        kill_tree(drv)
        die("chromedriver never answered /status")

    profile = tempfile.mkdtemp(prefix="smoke-chrome-")
    rc = 0
    sess = None
    try:
        try:
            sess = req(
                "POST",
                f"{wd}/session",
                {
                    "capabilities": {
                        "alwaysMatch": {
                            "browserName": "chrome",
                            "goog:chromeOptions": {
                                "binary": chrome,
                                "args": [
                                    "--headless=new",
                                    "--no-sandbox",
                                    "--disable-gpu",
                                    "--disable-dev-shm-usage",
                                    f"--user-data-dir={profile}",
                                ],
                            },
                            # The SSR stream holds `load` open — eager returns
                            # at DOMContentLoaded; we settle-wait instead.
                            "pageLoadStrategy": "eager",
                            "goog:loggingPrefs": {"browser": "ALL"},
                        }
                    },
                },
                # Cold-starting chrome on a loaded box can exceed 30s.
                timeout=90,
            )["value"]["sessionId"]
        except Exception as e:
            die(f"webdriver session creation failed: {e}")
        for url in urls:
            try:
                req("POST", f"{wd}/session/{sess}/url", {"url": url})
            except Exception as e:
                print(f"{url}: navigation error: {e}")
                rc = 1
                continue
            time.sleep(SETTLE_S)
            try:
                entries = req("POST", f"{wd}/session/{sess}/log", {"type": "browser"})[
                    "value"
                ]
            except Exception as e:
                print(f"{url}: log drain error: {e}")
                rc = 1
                continue
            severe = [e for e in entries if e.get("level") == "SEVERE"]
            warn = [e for e in entries if e.get("level") == "WARNING"]
            tag = "FAIL" if severe else "ok"
            print(f"== {url}: {tag}")
            for e in severe:
                print(f"  [SEVERE] {e['message'][:1200]}")
            for e in warn[:3]:
                print(f"  [warn] {e['message'][:300]}")
            if severe:
                rc = 1
            if shot and url == urls[-1]:
                try:
                    png = req("GET", f"{wd}/session/{sess}/screenshot")["value"]
                    with open(shot, "wb") as f:
                        f.write(base64.b64decode(png))
                    print(f"  screenshot → {shot}")
                except Exception as e:
                    print(f"  screenshot failed: {e}")
                    rc = 1
    finally:
        if sess:
            try:
                req("DELETE", f"{wd}/session/{sess}")
            except Exception:
                pass
        kill_tree(drv)
        # Chrome may still be writing the profile while it exits —
        # retry a moment rather than leaking the dir.
        for _ in range(15):
            try:
                shutil.rmtree(profile)
                break
            except OSError:
                time.sleep(0.2)
    sys.exit(rc)


if __name__ == "__main__":
    main()
