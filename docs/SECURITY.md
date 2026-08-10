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

## Reporting

Do not open a public issue containing secrets, tokens, private file names or upload-session
URLs. Until a private security contact is configured, report only that a security issue exists
to the repository owner through GitHub.
