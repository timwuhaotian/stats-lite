# Security policy

## Supported versions

Only the latest `main` gets security fixes.

## Reporting a vulnerability

Please **don't open a public issue.** Report it privately through GitHub:
[**Report a vulnerability**](https://github.com/timwuhaotian/stats-lite/security/advisories/new).

Include what an attacker could do, the steps to reproduce it, and the macOS version and
stats-lite build. You'll get a reply within a week. Once a fix is released, the advisory is
published with credit to you, unless you'd rather not be named.

## Scope

stats-lite reads CPU and disk counters and shows them in the menu bar. It has no network
access, no helpers and no elevated privileges; that is not a vulnerability. These are, for
example:

- Anything that makes stats-lite run commands, write files or send data anywhere.
- A distributed app whose signature doesn't hold.
