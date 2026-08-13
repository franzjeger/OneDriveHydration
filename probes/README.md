# Probes

Small programs that answer a question about the desktop or the kernel that
would otherwise be an assumption. They are never dependencies of the shipped
product — nothing in the Cargo workspace links Qt or KF6 — and they are not
built by CI. They exist so that a claim in a comment, a README or a commit
message can be a measurement instead of an argument.

The same idea, and the same standard, as HydrationAPI's `probes/`: most of the
hard questions in this stack were settled by a fifty-line program, and the
answer is worth keeping next to the code it justifies.

| | |
|---|---|
| `servicemenu-match.cpp` | Does a KIO servicemenu declaring `MimeType=all/allfiles` actually reach a file's context menu — and does it stay off directories? |

## Building

Each file's header comment carries its own build line and how to run it. They
need development headers the product does not: `servicemenu-match.cpp` wants
Qt6 and KF6 KIO.

Run them against a scratch `XDG_DATA_HOME` and `XDG_CACHE_HOME` so a
measurement neither depends on nor disturbs the session's own configuration:

```
XDG_DATA_HOME=/tmp/d XDG_CACHE_HOME=/tmp/c QT_QPA_PLATFORM=offscreen ./servicemenu-match /tmp/d/subject.bin
```

Always run the control. `servicemenu-match` printing an action means nothing
until the same invocation with no servicemenu installed prints none — a probe
that cannot report zero is not measuring anything.
