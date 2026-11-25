"""Signs the Mac build's screenshot run in as the test account (ZF_TEST_USER, ZF_TEST_PASS) by writing the app's own
cookie file, or with --out signs that device out again so a build machine never keeps one of the account's places.
Usage: mac_signin.py <base url> <data dir> [--out]"""
import json
import os
import sys
import time
import urllib.request
from http.cookiejar import CookieJar
from urllib.parse import urlparse

base, data = sys.argv[1].rstrip("/"), sys.argv[2]
jar_file = os.path.join(data, "cookies.json")
host = urlparse(base).hostname
# (Cloudflare turns away Python's own User-Agent)
headers = {"Content-Type": "application/json", "User-Agent": "zunflix-mac-build"}


def post(path, body, cookie=None):
    jar = CookieJar()
    opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))
    request = urllib.request.Request(base + path, json.dumps(body).encode(), dict(headers, **({"Cookie": cookie} if cookie else {})))
    opener.open(request, timeout=30).read()
    return jar


if "--out" in sys.argv:
    saved = json.load(open(jar_file)) if os.path.exists(jar_file) else []
    post("/api/signout", {}, "; ".join(f"{c['n']}={c['v']}" for c in saved))
    print("signed out")
else:
    jar = post("/api/signin", {"username": os.environ["ZF_TEST_USER"], "password": os.environ["ZF_TEST_PASS"]})
    month = int(time.time() * 1000) + 30 * 86400 * 1000
    cookies = [{"n": c.name, "v": c.value, "e": month, "p": c.path or "/", "d": host, "h": True, "s": bool(c.secure), "o": True} for c in jar]
    os.makedirs(data, exist_ok=True)
    with open(jar_file, "w") as f:
        json.dump(cookies, f)
    print("signed in:", ", ".join(c["n"] for c in cookies))
