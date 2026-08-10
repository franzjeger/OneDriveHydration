# systemd packaging

Release artifacts now contain the unprivileged daemon, control CLI and revision-matched
`hydrationd` helper in one payload. The helper is built directly from the exact HydrationAPI
commit pinned by `Cargo.toml`; its privileged source is not copied or allowed to drift here.

Systemd units are intentionally not included in the first binary payload. A unit must bind a
specific user's mount, runtime socket and numeric uid while keeping credentials out of the system
manager and retaining helper recovery after a user-manager restart. Those installation-time facts
need a package installer with validation; a generic unit with shell expansion would weaken the
boundary. The tarball is therefore an auditable build artifact, not yet an unattended installer.

Do not copy HydrationAPI's example units verbatim. The final product units must retain its
measured safety properties: a separate real mount, `RequiresMountsFor=` recovery, an explicit
`--peer-uid`, only `CAP_SYS_ADMIN` on the helper, `PrivateNetwork=yes`, and no credential access
from the privileged service.
