#!/usr/bin/env python3
"""Upload an App Bundle to a Google Play track and send it for review.

    play-upload.py KEY.json PACKAGE BUNDLE.aab TRACK "RELEASE NAME" NOTES.txt

KEY.json is a Google Cloud service account key that has release rights on the
app in Play Console (Users and permissions). TRACK is the API track name: the
closed test "Alpha" is `alpha`, production is `production`. With Managed
publishing off, committing the edit sends it for review and Play publishes it
once approved. Needs only the standard library and `cryptography`.
"""
import base64
import json
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import padding, rsa

API = "https://androidpublisher.googleapis.com/androidpublisher/v3/applications"
UPLOAD = "https://androidpublisher.googleapis.com/upload/androidpublisher/v3/applications"


def b64(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


def token(key: dict) -> str:
    now = int(time.time())
    head = b64(json.dumps({"alg": "RS256", "typ": "JWT"}).encode())
    claims = b64(json.dumps({
        "iss": key["client_email"],
        "scope": "https://www.googleapis.com/auth/androidpublisher",
        "aud": key["token_uri"],
        "iat": now,
        "exp": now + 3600,
    }).encode())
    pem = serialization.load_pem_private_key(key["private_key"].encode(), password=None)
    if not isinstance(pem, rsa.RSAPrivateKey):
        sys.exit("play-upload: the service account key is not an RSA key")
    sig = pem.sign(f"{head}.{claims}".encode(), padding.PKCS1v15(), hashes.SHA256())
    body = urllib.parse.urlencode({
        "grant_type": "urn:ietf:params:oauth:grant-type:jwt-bearer",
        "assertion": f"{head}.{claims}.{b64(sig)}",
    }).encode()
    with urllib.request.urlopen(key["token_uri"], body) as r:
        return json.load(r)["access_token"]


def call(method: str, url: str, auth: str, body=None, raw: "bytes | None" = None):
    headers = {"Authorization": f"Bearer {auth}"}
    if raw is not None:
        data, headers["Content-Type"] = raw, "application/octet-stream"
    elif body is not None:
        data, headers["Content-Type"] = json.dumps(body).encode(), "application/json"
    else:
        data = b""
    req = urllib.request.Request(url, data=data, method=method, headers=headers)
    try:
        with urllib.request.urlopen(req, timeout=600) as r:
            return json.load(r)
    except urllib.error.HTTPError as e:
        sys.exit(f"play-upload: {method} {url.split('?')[0]} -> {e.code}: {e.read().decode()[:800]}")


def main():
    if len(sys.argv) != 7:
        sys.exit(__doc__)
    key_path, package, bundle, track, name, notes_path = sys.argv[1:]
    with open(key_path) as f:
        key = json.load(f)
    with open(notes_path) as f:
        notes = f.read().strip()
    auth = token(key)
    base = f"{API}/{package}/edits"
    edit = call("POST", base, auth, body={})["id"]
    print(f"play-upload: edit {edit}")
    with open(bundle, "rb") as f:
        up = call("POST", f"{UPLOAD}/{package}/edits/{edit}/bundles?uploadType=media", auth, raw=f.read())
    code = str(up["versionCode"])
    print(f"play-upload: bundle {code} uploaded, sha256 {up.get('sha256', '?')}")
    call("PUT", f"{base}/{edit}/tracks/{track}", auth, body={
        "track": track,
        "releases": [{
            "name": name,
            "versionCodes": [code],
            "status": "completed",
            "releaseNotes": [{"language": "en-US", "text": notes}],
        }],
    })
    print(f"play-upload: track {track} set to {name}")
    call("POST", f"{base}/{edit}:commit", auth)
    print(f"play-upload: committed; {name} is sent for review on {track}")


if __name__ == "__main__":
    main()
