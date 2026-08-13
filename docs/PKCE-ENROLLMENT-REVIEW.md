# Browser (authorization-code + PKCE) enrollment: threat-model review

> **STATUS: ACCEPTED by the repository owner, 2026-08-13.**
> This is the review that `docs/ROADMAP.md` M1 listed as unchecked
> ("PKCE/browser enrollment threat-model review"). It is a security decision
> about whether authorization-code/PKCE enrollment belongs *in the product*,
> and it was not an agent's to ratify: the recommendation in §7 was put to the
> owner as a question and accepted as written, without amendments. The
> conditions in §7 bind the implementation, and §6 lists what would re-open
> this document.

## Why this is on the critical path, and why now

The shipped daemon knows exactly one way to sign in: the OAuth 2.0 device code
flow (RFC 8628). `hydration-graph`'s `Grant` enum has three variants —
`DeviceCode`, `DeviceToken`, `Refresh` — and no fourth
(`crates/.../hydration-graph/src/auth.rs`). That was a deliberate scope line: a
daemon has no browser and no redirect URI to be called back on, so device code
was the whole of what `auth.rs` needed to implement.

On this deployment's tenant that line has become a wall. Conditional Access
blocks the device code flow *specifically* while still permitting an ordinary
interactive browser sign-in. Measured 2026-08-10: a device-code sign-in against
the account returned that it *"does not meet the criteria to access this
resource"*, and browser sign-in for the same account succeeded. This is not a
transient outage and not a misconfiguration on our side — it is the tenant
enforcing Microsoft's own current guidance. Microsoft recommends blocking device
code flow wherever it is not needed, and ships an "Authentication flows"
Conditional Access condition whose first purpose is to target device code
([concept-authentication-flows]). The reason is concrete: the Storm-2372
device-code phishing campaign (Microsoft Security, 2025-02-13, [storm-2372])
turned the flow's own shape — read a code from one channel, type it into a login
page reached by another — into a credential-harvesting technique at scale. A
tenant that blocks device code is doing the thing Microsoft told it to do.

So for this deployment the device code flow is not a fallback that is merely
slower or uglier. It is unusable. Enrollment happens today through
`tools/pkce-enroll.py` — a standalone, stdlib-only script that performs the
browser half (authorization code with PKCE against a loopback redirect) and
leaves a refresh token where the daemon's migration already looks for it. That
script is prior art for this review, not a product component, and it says so in
its own docstring: it "deliberately lives outside `hydration-graph`" and is "not
a down payment on that review." This document is that review.

Two roadmap items wait behind it. M3's "Re-authentication UX" needs *some*
in-product way to re-enroll when a credential dies. An installer that can enroll
needs the same. And anyone else adopting this product on a device-code-blocking
tenant hits the wall we hit. The question this review has to answer is narrow and
load-bearing: **does authorization-code/PKCE enrollment belong in the product,
and under what constraints?**

---

## 1. The redirect

### What the flow does

`pkce-enroll.py` picks a free TCP port, starts a one-shot HTTP server on the
loopback interface, and hands the browser an `/authorize` URL whose
`redirect_uri` points back at that port. The browser completes sign-in against
`login.microsoftonline.com` and is redirected to the loopback listener, which
captures the `code` and `state` from the query string. The code is then redeemed
at the token endpoint together with the PKCE `code_verifier`.

### Microsoft's loopback rules, as fact

These are the documented rules for loopback redirect URIs on public clients
([reply-url], "Localhost exceptions"), stated so the design rests on fact rather
than assumption:

- **`http` is permitted for loopback**, and only for loopback, "because the
  redirect never leaves the device." Both `http://localhost/...` and
  `https://localhost/...` are acceptable; `http` to any non-loopback host is
  not.
- **The port is ignored when matching a loopback redirect URI.** All of
  `http://127.0.0.1:1234/app`, `:5000`, `:8080` match one registered
  `http://127.0.0.1/app`. This is the rule that makes an *ephemeral* port legal:
  you register one path-less loopback URI and every ephemeral port matches it.
  Port is ignored *only* for loopback; everywhere else it is significant.
- **The IPv6 loopback `[::1]` is not currently supported** as a registered
  redirect URI.
- **Microsoft says to prefer the literal `127.0.0.1` over `localhost`**, to
  avoid breakage "due to misconfigured firewalls or renamed network interfaces."
- **An `http` loopback redirect URI cannot be added through the Azure portal's
  Redirect URIs textbox**; it must be written into the app registration
  manifest's `replyUrlsWithType`. This is an operational fact for whoever
  registers the app, not a code concern.

### What binds the port, and what else could

Measured on this machine with a short probe (loopback `bind()` calls and
`getaddrinfo` only — nothing authenticated, no packet sent; reproduce by binding
a listener on `127.0.0.1:0`, then attempting the binds below):

```
holder LISTENs on 127.0.0.1:P
second bind of 127.0.0.1:P (plain):        refused (EADDRINUSE)
second bind of 127.0.0.1:P (SO_REUSEADDR): refused (EADDRINUSE)
second bind of 127.0.0.1:P (SO_REUSEPORT): refused (EADDRINUSE)
bind of [::1]:P while 127.0.0.1:P is held: SUCCEEDED
free_port() window: after close, another socket took 127.0.0.1:P: SUCCEEDED
getaddrinfo('localhost') order on this machine: ['::1', '127.0.0.1']
```

Three findings, in order of how much they should change the design:

**(a) The `localhost`/`127.0.0.1` split is real here and it is the script's one
security-relevant bug.** The script sets `redirect_uri = f"http://localhost:{port}"`
(so that string is what the browser is told and what must be registered) but
binds its listener to `127.0.0.1` only. On this machine `getaddrinfo("localhost")`
returns `::1` *first*. And `[::1]:P` is a *separate, independently bindable*
socket while `127.0.0.1:P` is held — the kernel does not treat them as the same
endpoint. So the browser, told to reach `localhost`, tries `::1` first, where the
script is not listening. In the common case the browser gets a connection refusal
on `::1` and falls back to `127.0.0.1`, and enrollment works "by fallback." In
the adversarial case a local process that has bound `[::1]:P` receives the
redirect — and with it the authorization code — instead of the script. This is
exactly the class of failure Microsoft's "prefer `127.0.0.1`" guidance names, and
`[::1]` being unregisterable is the same fact from the registration side. The fix
is to use the literal `127.0.0.1` in *both* the `redirect_uri` and the bind, so
the browser resolves nothing and there is no IPv6 twin to race. **Change this.**

**(b) A listening loopback socket is exclusive on its address.** Once the script
is `LISTEN`ing on `127.0.0.1:P`, no second process can bind `127.0.0.1:P` — not
plain, not with `SO_REUSEADDR`, not with `SO_REUSEPORT` (the holder did not opt
into port sharing, and `SO_REUSEPORT` on the second binder alone does not
override that). Python's `HTTPServer` sets `allow_reuse_address = True`, but as
measured that only helps against a socket in `TIME_WAIT`, never against a live
listener. So on the correct address, the code is delivered to us and not to a
squatter. This is what makes loopback delivery sound *once the address is
corrected per (a)*.

**(c) The `free_port()` pattern has a TOCTOU window, and it is a reliability
bug, not a theft one.** The script binds port 0, reads the assigned port, closes
the socket, and re-binds it later inside `HTTPServer`. Between the close and the
re-bind, another local socket can take the port (measured: `SUCCEEDED`). Because
a live listener cannot be displaced (finding b), the consequence is that the
script's own bind then *fails* — a crash before the browser is ever opened, not a
silent handoff of the code. It is still worth removing: pass the already-bound
listening socket into the server (`HTTPServer` can adopt a socket, or bind port 0
directly and read back the port) so there is no window. **Change this**, lower
priority than (a).

### What the code is worth to something that steals it, and why PKCE is the answer

The authorization code is short-lived — Microsoft documents "about 1 minute" and
single use ([v2-oauth2-auth-code-flow]) — and, critically, **it cannot be
redeemed without the PKCE `code_verifier`.** Microsoft requires `code_verifier`
at redemption whenever the authorization request carried a `code_challenge`, and
answers a missing or wrong verifier with `invalid_grant`. The verifier is
32 bytes of `os.urandom` held only in the enrolling process's memory; it is never
sent to the browser and never leaves the machine except in the final
back-channel POST to the token endpoint. So the threat model for a captured code
is:

- A local process that manages to *receive* the redirect (via the `::1` split of
  finding (a), or by winning the `free_port` race of finding (c)) gets the code
  but not the verifier. It cannot exchange the code. The worst it achieves is
  **denial of enrollment** — the user's sign-in fails — not credential theft.
  PKCE is precisely what downgrades "someone caught the code" from compromise to
  nuisance. This is why loopback + PKCE for a *public client* (no client secret
  to steal, none shippable — see §2) is an accepted pattern and not a compromise
  we are talking ourselves into.
- The code also lands in the browser's history and, depending on the handler, in
  process arguments, as `http://127.0.0.1:P/?code=...&state=...`. Same analysis:
  short-lived, single-use, useless without the verifier. The script already
  silences its *own* HTTP access log (`log_message` is overridden to a no-op)
  precisely so the code does not reach stderr — **keep that.**

The residual real risk is not the code at all; it is the process that holds the
*verifier and the resulting refresh token* — the enrolling process itself, and
the plaintext file it writes. That is §2's subject.

### `state`

The script generates `state = secrets.token_urlsafe(16)` and checks it *before*
spending the code, rejecting a response whose `state` does not match. This is the
CSRF/mix-up defense RFC 6749 §10.12 calls for, done correctly — the handler
stores the query without acting on it, and the main path validates `state`,
then `error`, then extracts `code`. **Keep this exactly.**

---

## 2. What the flow can reach — the credential and privilege boundary

The product's central invariant is that the **unprivileged daemon holds the
credential and the privileged helper (`hydrationd`) never does** — it has no
token and opens no network connection (`docs/ARCHITECTURE.md`, `docs/SECURITY.md`
first line). The review has to confirm a browser flow does not move that
boundary. It does not, and here is the reasoning rather than the assertion:

- **The credential a browser flow yields is the same object as a device-code
  flow yields**: a refresh token for the *same public client id* and the *same
  scopes*. `pkce-enroll.py` requests `offline_access Files.ReadWrite.All
  User.Read`; the daemon's `auth_config` builds
  `AuthConfig::public_client(client_id).with_scopes(["Files.ReadWrite.All",
  "User.Read"])`, and `with_scopes` re-adds `offline_access`. Same audience, same
  scope set. The token is redeemed and rotated by the same `TokenCache`
  regardless of how it was first obtained. So nothing downstream of enrollment —
  storage, rotation, the single-flight refresh, the privileged split — is aware
  of, or changed by, which flow minted the first refresh token.
- **Storage is unchanged.** The daemon derives its Secret Service entry from the
  client id — `keyring::Entry::new("io.github.franzjeger.OneDriveHydration",
  "refresh-token:{client_id}")` — and the `TokenCache`'s `CredentialStore` writes
  every rotation back through Secret Service. A browser flow feeds the same
  store.
- **The privileged helper is out of the credential path by construction, not by
  enrollment choice.** `hydrationd` never receives a token from device code
  today and would never receive one from a browser flow either; the boundary is a
  property of HydrationAPI's architecture, upstream of this decision.

So the *runtime* boundary is untouched. There is exactly one place a browser flow
as *currently implemented by the external script* differs from in-product device
code, and it is worth stating plainly because it is the strongest argument for
building the flow **inside** the product rather than blessing the script:

> **The script writes the refresh token to a plaintext file as an intermediate.**
> `pkce-enroll.py` writes `<state-dir>/refresh-token` at mode `0600` (opened with
> `O_CREAT|O_TRUNC` at `0600` rather than written-then-chmod'd, so the token never
> exists at the umask's permissions — correct), with no trailing newline (correct:
> `FileCredentialStore::load` does `read_to_string` and hands the result straight
> to `RefreshToken::new`, so a newline would become part of the credential). The
> daemon's `migrate_legacy_credential` then moves it into Secret Service and
> unlinks it — but only after validating it is a non-symlink regular file with no
> group/other bits (`mode & 0o077 == 0`), and only removing the plaintext *after*
> the secure write succeeds, then `fsync`ing the parent directory. That migration
> is careful and correct.

But the in-product device-code path has **no such file at all**: `complete_device_code`
installs straight into the `TokenCache` and persists via the Secret Service
`CredentialStore`. The plaintext-on-disk moment is a property of enrollment being
an *external* process that cannot reach the daemon's in-memory cache — not a
property of PKCE. **If browser enrollment is built into the product, it should
install directly into the shared `TokenCache` (`sign_in_with` + the normal
persist path), eliminating the file and its migration window entirely.** That is
the single biggest "keep the behavior, change the mechanism" conclusion of this
review, and it only becomes available by building the flow in-product.

---

## 3. Who may start it

Enrollment writes a credential the daemon then acts on across the *entire*
OneDrive account. That is a larger blast radius than any operation currently
guarded in this codebase, and the guard reasoning should exceed, not merely
match, what already exists.

Today enrollment is a same-uid, same-session act: either `onedrive-hydration-daemon
auth` (interactive, in the user's own process) or `pkce-enroll.py` run by the
user. The gate is Unix: you must be the user, able to write the state directory
and unlock the user's Secret Service collection. There is **no** IPC enrollment
surface. That is a fine posture and the review does not propose widening it
speculatively.

The relevant precedent is the D-Bus `Evict` method, which re-checks the caller's
uid against the daemon's own euid through `GetConnectionCredentials` and fails
closed when the sender cannot be attributed — *even though* session-bus `EXTERNAL`
auth already restricts callers to the bus owner. The stated reason is that
`Evict` "destroys local content on request" and "a proxy or a future bus policy
is not a thing to inherit assumptions from." The reasoning transfers to
enrollment with more force, not less:

- **Enrollment installs a credential; eviction destroys a file.** Eviction is
  locally reversible — the bytes re-download. A credential swap is not: it can
  point the daemon at a *different account*, or replace a live credential the
  user still wanted. The consequence is larger and not recoverable by re-running
  anything.
- Therefore, **if re-authentication is ever exposed on the D-Bus surface** (which
  M3's "Re-authentication UX" will pressure toward, since a tray "Sign in again"
  button is the obvious shape), it must carry *at least* the `Evict` uid
  re-check, and the review's position is that it should carry more: the request
  should be an owner-initiated interactive action, never a silent headless call,
  because a silent enrollment trigger is an account-swap primitive.
- There is also a practical reason enrollment resists being a daemon method:
  the browser flow must *launch a browser as the user in the user's session*.
  A daemon method invoked over the bus is the wrong place to be spawning
  `xdg-open`. This is an argument for keeping enrollment in a user-session-scoped,
  user-initiated tool (a CLI subcommand, or a small helper the tray launches in
  the session) rather than a `ControlSurface` method — the re-auth UX can *prompt*
  via D-Bus state (credential health, once M3 adds it) while the act of enrolling
  stays a user-launched, session-scoped process.

---

## 4. The browser itself

`xdg-open` hands the URL to whatever the desktop has registered for
`x-scheme-handler/https` (measured here: `brave-browser.desktop` for both
`http` and `https`). What that does and does not imply:

- **It implies the *authorize* URL is passed to the registered browser** — as a
  command-line argument or via the desktop portal — and lands in that browser's
  history and possibly its `/proc/<pid>/cmdline` (readable same-uid). That URL
  contains `client_id`, `scope`, `state`, and `code_challenge`. **None of these
  is a credential.** The `code_challenge` is the *public* half of PKCE by design;
  the verifier is not in the URL. So a handler that logs or leaks the authorize
  URL leaks nothing that helps an attacker.
- **It does not imply any trust that the browser keeps a secret.** The browser is
  already in the user's trusted computing base — it is *how the human types their
  password*. The code is never delivered *through* `xdg-open`; it comes back on
  the loopback socket. So even a malicious registered handler cannot obtain the
  code via `xdg-open` — it can at most refuse to open the URL (a denial) or open
  it in an attacker-chosen context, which is the same risk any browser-based sign
  in already carries.
- **The script's fallbacks are correct — keep them.** `xdg-open` is invoked with
  `check=False` and a `FileNotFoundError` guard, and the authorize URL is
  *printed* before the browser is launched, with a `--no-browser` mode that only
  prints. So a missing or broken `xdg-open` degrades to "open this URL yourself,"
  which is the right behavior on a minimal desktop. The one wart is that a
  *silent* `xdg-open` failure (exit non-zero, stderr to `/dev/null`) is not
  reported — but because the URL was already printed, the user can still recover.
  If built in-product, prefer surfacing the launch result over discarding it (in
  the spirit of the sibling repo's "never invent a diagnostic; print what
  actually happened").

---

## 5. What device code buys, and what it costs

The review must not treat PKCE as a strict upgrade. The two flows are for
different machines:

- **Device code is the right flow when there is no local browser and no loopback
  listener** — a headless server, a box reached only over SSH, an appliance.
  That is the capability it was designed for (RFC 8628) and the capability the
  browser/PKCE flow *cannot* provide: PKCE needs a browser on the enrolling
  machine and a socket the browser can reach. For a headless OneDrive host,
  device code is not a fallback, it is the correct answer — *where the tenant
  permits it.*
- **Its cost is phishability.** The device code flow trains the user to carry a
  code between channels, which is the Storm-2372 tradecraft and the reason
  Microsoft recommends blocking it and tenants (including this one) do. The
  browser/PKCE flow has no equivalent primitive: there is no code to relay, and a
  captured redirect is verifier-bound.

So the honest framing is: **browser/PKCE for machines that have a browser;
device code for machines that do not, on tenants that still allow it.** A
complete product wants *both*, selected by environment. Building browser
enrollment must not delete the device-code path — it must sit beside it. On this
deployment's tenant device code is blocked, which decides *this* machine's
default, but it does not retire the flow for the machines device code exists to
serve.

---

## 6. What would make this review's conclusion wrong later

A review that cannot expire is one nobody revisits. The recommendation in §7 is
contingent on all of the following remaining true; if any flips, re-open this
document:

1. **Microsoft's loopback policy holds.** The design leans on three current
   facts: `http` is allowed for loopback, the port is ignored when matching a
   loopback redirect, and one path-less `http://127.0.0.1` registration
   therefore covers every ephemeral port. If Microsoft stops ignoring the port,
   stops allowing `http` loopback, or begins requiring `[::1]` (today
   unsupported), the ephemeral-port design breaks and enrollment must change.
2. **The desktop's default browser can reach a host loopback socket.** This is
   the nearest real risk, not a hypothetical: a **sandboxed default browser**
   (Flatpak, Snap) may be confined such that it cannot connect to a listener on
   the host's `127.0.0.1`. On such a desktop the redirect never arrives and the
   loopback flow silently hangs until timeout. Before shipping, this must be
   tested against a Flatpak browser, and the failure must be diagnosable (the
   300s timeout should say *why*, not just time out).
3. **`xdg-open` and the registered handler are trustworthy.** On a desktop where
   the `https` scheme handler is hostile or absent, the flow degrades to
   copy-paste (acceptable) — but a desktop where the handler is actively
   malicious is out of scope for any browser-based sign-in and should be stated
   as such rather than defended against here.
4. **There is a browser at all.** A headless install has no browser and the
   PKCE flow is simply inapplicable; device code (if the tenant allows) or a
   `--no-browser` copy-paste onto another machine is the path. The conclusion to
   build browser enrollment is *not* a conclusion to make it mandatory.
5. **The tenant's Conditional Access posture.** Today it blocks device code and
   allows browser sign-in, which is what makes browser enrollment necessary. If
   the block on device code is lifted, device code returns as an option (the
   phishing cost stands). If browser sign-in is *also* later gated (compliant/
   managed-device requirements), even the PKCE flow could be refused and the
   answer changes again — at which point enrollment is a device-registration
   problem, not a flow-choice problem.

---

## 7. Recommendation (accepted 2026-08-13)

**Build it in the product, under the conditions below — do not bless the external
script as the permanent answer, and do not remove device code.**

The credential and privilege boundary is unchanged by a browser flow (§2); the
loopback + PKCE public-client pattern is legitimate under Microsoft's documented
rules (§1); and the one place the *script* is weaker than in-product device code —
a plaintext token file — is an artifact of being external and disappears when the
flow installs straight into the shared `TokenCache` (§2). Meanwhile device code
is measured-unusable on this tenant, and both the M3 re-auth UX and an enrolling
installer are blocked waiting on this decision.

Conditions on building it:

1. **Add an authorization-code + PKCE `Grant` alongside device code in
   `hydration-graph`, not replacing it.** Keep device code for headless hosts and
   for tenants that permit it (§5).
2. **Install directly into the shared `TokenCache` and Secret Service; no
   plaintext file.** Eliminate the `<state-dir>/refresh-token` handoff that only
   exists because the current tool is external (§2).
3. **Bind and register `127.0.0.1` literally, in both the `redirect_uri` and the
   listener** — never `localhost` (§1a). Register one path-less
   `http://127.0.0.1` redirect via the app-registration manifest
   (`replyUrlsWithType`), since the portal textbox refuses `http` loopback (§1).
4. **Keep, unchanged, the script's correct choices:** PKCE S256 (verified against
   RFC 7636 Appendix B: verifier `dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk`
   yields challenge `E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM`, matching the
   RFC's own vector), `state` validated before the code is spent,
   `prompt=select_account` to prevent silently enrolling a live SSO session's
   account, the `0600`-by-construction file mode (until the file is removed
   entirely per condition 2), the silenced HTTP access log, and the printed-URL
   `--no-browser` fallback.
5. **Close the `free_port()` TOCTOU** by handing the already-bound listening
   socket to the server rather than closing and re-binding (§1c).
6. **Keep enrollment a user-initiated, session-scoped act.** If re-auth is
   surfaced on D-Bus, it must carry at least the `Evict` uid re-check, treat the
   trigger as a prompt rather than a silent action, and launch the browser in the
   user's session — not from a bus method (§3).
7. **Before shipping, test against a sandboxed (Flatpak/Snap) default browser**
   and make the loopback-timeout path diagnosable (§6.2).

Rejected alternatives, recorded:

- **Keep enrollment permanently outside the product (bless the script).**
  Rejected: it leaves a plaintext-token step in-tree that the product otherwise
  never has (§2), gives the M3 re-auth UX nothing to build on, and ships a Python
  dependency and an out-of-band tool as the only way onto a common tenant
  configuration.
- **Replace device code with PKCE.** Rejected: device code is the only flow that
  works headlessly, and blocking it is the tenant's choice, not a property of the
  flow (§5).
- **Expose enrollment as a D-Bus method for the tray.** Rejected as the *act* of
  enrolling (§3); the tray may prompt, but the browser launch and credential write
  belong in a session-scoped, user-launched process.

---

*`docs/ROADMAP.md` M1's "PKCE/browser enrollment threat-model review" is
ticked on the basis of the owner's acceptance above — the decision, not the
draft's existence. The implementation the decision permits is tracked by the
still-unticked M3 item, and lands only under §7's conditions.*

<!-- References (Microsoft Learn, retrieved 2026-08-12):
[reply-url]: https://learn.microsoft.com/en-us/entra/identity-platform/reply-url
[v2-oauth2-auth-code-flow]: https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-auth-code-flow
[concept-authentication-flows]: https://learn.microsoft.com/en-us/entra/identity/conditional-access/concept-authentication-flows
[storm-2372]: https://www.microsoft.com/en-us/security/blog/2025/02/13/storm-2372-conducts-device-code-phishing-campaign/
-->
