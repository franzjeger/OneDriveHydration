# Security model

## Non-negotiable invariants

- OAuth tokens exist only in the unprivileged user process.
- Bearer tokens are attached only to validated Microsoft Graph origins.
- Pre-authorized upload URLs never receive the account bearer token.
- Refresh-token rotation is persisted before the rotated token is treated as durable.
- Refresh tokens are stored only in the user's Linux Secret Service collection; there is no
  plaintext credential-file fallback when the service is unavailable or locked.
- A malformed page, identity, tree or cursor stops progress instead of dropping an item.
- Remote deletion never silently wins over unsent local content.
- Tests and logs must never contain live credentials or pre-authorized upload URLs.

## Dependency policy

CI rejects known RustSec advisories, yanked packages, wildcard requirements, unknown registries,
and unapproved Git sources. The only permitted Git dependency is the full-revision-pinned
HydrationAPI repository. Dependency licenses are deny-by-default against the reviewed allowlist
in `deny.toml`; warnings about duplicate versions remain visible for deliberate cleanup.

Dependabot checks Cargo and GitHub Actions weekly. Updates still pass MSRV, stable Rust and the
supply-chain policy before merge.

## Reporting

Do not open a public issue containing secrets, tokens, private file names or upload-session
URLs. Until a private security contact is configured, report only that a security issue exists
to the repository owner through GitHub.
