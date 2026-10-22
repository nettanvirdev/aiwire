# Security policy

## Reporting a vulnerability

Please report vulnerabilities privately through
[GitHub security advisories](https://github.com/nettanvirdev/aiwire/security/advisories/new),
not in a public issue. Include what is affected, how to reproduce it, and the impact you see.

You'll get a reply within 7 days. Once a fix is released, the advisory is published with
credit to you unless you'd rather stay anonymous.

## Supported versions

Only the latest released version gets security fixes.

## Scope

aiwire sends API keys to the hosts you configure. Things we treat as vulnerabilities include
keys leaking into logs, `Debug` output or error messages; keys sent to the wrong host; and
crashes or unbounded memory use caused by a malicious server response.
