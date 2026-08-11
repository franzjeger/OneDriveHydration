#!/usr/bin/env python3
"""Browser enrollment for OneDriveHydration, until the daemon grows its own.

The daemon only knows the device code flow (`Grant` has exactly `DeviceCode`,
`DeviceToken` and `Refresh`), and a Conditional Access policy can block that flow
specifically while still permitting an ordinary interactive browser sign-in. This
does the browser half — authorization code with PKCE against a loopback redirect
— and leaves the result where the daemon already looks for it.

It deliberately lives outside `hydration-graph`. Adding an authorization-code
grant to that crate is on the roadmap behind a threat-model review, and this is
enrollment tooling to unblock testing, not a down payment on that review.

The handoff is `<state-dir>/refresh-token`, which `migrate_legacy_credential`
reads once and moves into Secret Service, deleting the file only after the secure
write succeeds. That path requires a regular file, not a symlink, with no group
or other permission bits — so this writes it 0600 by construction.

The token is written with **no trailing newline**: `FileCredentialStore::load`
does `read_to_string` and hands the result straight to `RefreshToken::new`, so a
newline would silently become part of the credential.

No third-party packages, on purpose — the standard library covers all of it, and
an enrollment tool that needs a virtualenv is one more thing to get wrong.
"""

import argparse
import base64
import hashlib
import http.server
import json
import os
import secrets
import socket
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request
from typing import NoReturn

# Matched to `AuthConfig::public_client` and `auth_config` in the daemon. If
# these drift the refresh token still redeems, but for a different audience than
# the daemon asks for, which surfaces much later and much less clearly.
AUTHORITY_HOST = "login.microsoftonline.com"
TENANT = "common"
SCOPES = ["offline_access", "Files.ReadWrite.All", "User.Read"]


def b64url(raw: bytes) -> str:
    """base64url with the padding stripped, as PKCE requires."""
    return base64.urlsafe_b64encode(raw).rstrip(b"=").decode("ascii")


class Redirect(http.server.BaseHTTPRequestHandler):
    """One request, captured whole.

    The query is stored rather than interpreted here: `state` has not been
    checked yet at this point, and a handler that acted on an unverified
    parameter would be the bug this flow exists to avoid.
    """

    captured = None

    def do_GET(self):  # noqa: N802 - name fixed by BaseHTTPRequestHandler
        Redirect.captured = urllib.parse.parse_qs(
            urllib.parse.urlparse(self.path).query
        )
        body = (
            b"<html><body style='font-family:sans-serif'>"
            b"<h2>Innlogging mottatt</h2>"
            b"<p>Du kan lukke denne fanen og g&aring; tilbake til terminalen.</p>"
            b"</body></html>"
        )
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args):
        """Silence the default stderr access log; it would print the code."""


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def post_form(url: str, fields: dict) -> dict:
    """POST a form and return the parsed body, error responses included.

    The identity platform puts the useful part of a failure in the body of a 400,
    so a bare `HTTPError` would throw away the only thing worth reading.
    """
    data = urllib.parse.urlencode(fields).encode("ascii")
    request = urllib.request.Request(
        url,
        data=data,
        headers={"Content-Type": "application/x-www-form-urlencoded"},
    )
    try:
        with urllib.request.urlopen(request, timeout=60) as response:
            return json.loads(response.read())
    except urllib.error.HTTPError as e:
        raw = e.read()
        try:
            return json.loads(raw)
        except json.JSONDecodeError:
            return {
                "error": f"http_{e.code}",
                "error_description": raw.decode("utf-8", "replace").strip()
                or f"HTTP {e.code} with an empty body",
            }


def fail(message: str) -> NoReturn:
    print(f"pkce-enroll: {message}", file=sys.stderr)
    sys.exit(1)


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Browser (PKCE) enrollment for OneDriveHydration."
    )
    parser.add_argument("--client-id", required=True)
    parser.add_argument(
        "--state-dir",
        required=True,
        help="the daemon's --state-dir; refresh-token is written inside it",
    )
    parser.add_argument(
        "--tenant",
        default=TENANT,
        help=f"authority tenant (default {TENANT}, matching the daemon)",
    )
    parser.add_argument(
        "--no-browser",
        action="store_true",
        help="print the URL instead of opening it",
    )
    args = parser.parse_args()

    state_dir = os.path.abspath(os.path.expanduser(args.state_dir))
    if not os.path.isdir(state_dir):
        fail(f"{state_dir} is not a directory; create it first")

    verifier = b64url(os.urandom(32))
    challenge = b64url(hashlib.sha256(verifier.encode("ascii")).digest())
    state = secrets.token_urlsafe(16)

    port = free_port()
    redirect_uri = f"http://localhost:{port}"

    authorize = f"https://{AUTHORITY_HOST}/{args.tenant}/oauth2/v2.0/authorize?" + (
        urllib.parse.urlencode(
            {
                "client_id": args.client_id,
                "response_type": "code",
                "redirect_uri": redirect_uri,
                "response_mode": "query",
                "scope": " ".join(SCOPES),
                "state": state,
                "code_challenge": challenge,
                "code_challenge_method": "S256",
                # Force the account chooser. Without it a live SSO session
                # silently enrolls whichever account the browser already holds,
                # which is exactly the mistake worth making impossible when a
                # test account and a production account both exist.
                "prompt": "select_account",
            }
        )
    )

    print(f"Redirect-URI (må være registrert på appen): {redirect_uri}")
    print()
    if args.no_browser:
        print("Åpne denne i nettleseren:")
        print(authorize)
    else:
        print("Åpner nettleseren. Logg inn med kontoen du vil enrolle.")
        print("Hvis ingenting skjer, åpne denne manuelt:")
        print(authorize)
        try:
            subprocess.run(
                ["xdg-open", authorize],
                check=False,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
        except FileNotFoundError:
            print("(xdg-open finnes ikke; bruk URL-en over)")
    print()
    print(f"Venter på redirect på {redirect_uri} ...")

    server = http.server.HTTPServer(("127.0.0.1", port), Redirect)
    server.timeout = 300
    server.handle_request()
    server.server_close()

    got = Redirect.captured
    if got is None:
        fail("ingen redirect kom inn innen 300 sekunder")

    if "error" in got:
        description = got.get("error_description", ["(ingen beskrivelse)"])[0]
        fail(f"{got['error'][0]}: {description}")

    # Checked before the code is spent: a mismatched state means the response
    # did not come from the request this process made.
    if got.get("state", [None])[0] != state:
        fail("state stemmer ikke — svaret hører ikke til denne forespørselen")

    code = got.get("code", [None])[0]
    if not code:
        fail(f"redirect hadde verken code eller error: {sorted(got)}")

    token = post_form(
        f"https://{AUTHORITY_HOST}/{args.tenant}/oauth2/v2.0/token",
        {
            "client_id": args.client_id,
            "grant_type": "authorization_code",
            "code": code,
            "redirect_uri": redirect_uri,
            "code_verifier": verifier,
            "scope": " ".join(SCOPES),
        },
    )

    if "error" in token:
        fail(
            f"{token['error']}: "
            f"{token.get('error_description', '(ingen beskrivelse)')}"
        )

    refresh = token.get("refresh_token")
    if not refresh:
        # offline_access was requested, so its absence is a consent problem on
        # the app registration rather than anything this script can retry.
        fail(
            "svaret hadde ingen refresh_token — mangler appen offline_access? "
            f"scopes som ble gitt: {token.get('scope', '(ukjent)')}"
        )

    path = os.path.join(state_dir, "refresh-token")
    # Opened 0600 rather than written and chmod-ed after: between those two calls
    # the token would exist at the umask's permissions, and the daemon's
    # migration refuses any file with group or other bits set anyway.
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as f:
        f.write(refresh)  # no trailing newline; see the module docstring

    print()
    print(f"Skrev refresh-token til {path} (mode 0600).")
    print(f"Scopes: {token.get('scope', '(ukjent)')}")
    print("Daemonen flytter den inn i Secret Service ved neste start,")
    print("og sletter fila først etter at den sikre skrivingen har lyktes.")


if __name__ == "__main__":
    main()
