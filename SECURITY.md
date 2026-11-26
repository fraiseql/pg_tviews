# Security Policy

## Supported versions

pg_tviews is in beta. Only the latest `0.1.0-beta.N` release receives security fixes;
a fix ships in the next beta, never as a backport.

## Reporting a vulnerability

Do not open a public issue, pull request or discussion for a vulnerability.

Report it privately through GitHub: on
[fraiseql/pg_tviews → Security → Advisories](https://github.com/fraiseql/pg_tviews/security/advisories/new),
choose **Report a vulnerability**. Only the maintainers can read the report.

Include what you can of:

- the affected version(s) and PostgreSQL version;
- what a role can do that it should not, and which privileges that role needs;
- the steps or SQL that reproduce it;
- any workaround you found.

## What happens next

- We aim to acknowledge a report within 48 hours and to confirm or reject it within
  7 days.
- The fix is written privately and tested with the full suite. Until the release that
  fixes it, the defect is named only by its advisory ID: in commits, the CHANGELOG and
  public issues.
- The advisory is published with that release, crediting the reporter unless they ask
  not to be named.

pg_tviews offers no bounty.

## The security model

What pg_tviews trusts, what runs as whom, and what each threat is answered by:
[docs/security/architecture.md](docs/security/architecture.md). How to check a
release's signature and provenance:
[docs/security/verify-release.md](docs/security/verify-release.md).
