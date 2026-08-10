# systemd packaging

The user and privileged units belong here after the product package owns both installed
binaries. The unprivileged daemon and control CLI are stable enough to package, but `hydrationd`
is still built by HydrationAPI and is not yet an artifact of this repository. Shipping units now
would create an installation that references a helper binary the package does not provide.

Do not copy HydrationAPI's example units verbatim. The final product units must retain its
measured safety properties: a separate real mount, `RequiresMountsFor=` recovery, an explicit
`--peer-uid`, only `CAP_SYS_ADMIN` on the helper, `PrivateNetwork=yes`, and no credential access
from the privileged service.
