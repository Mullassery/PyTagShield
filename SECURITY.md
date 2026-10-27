# Security Policy

## Supported versions

Only the latest release and the `main` branch receive security fixes. There is
no backport policy for older versions yet.

## Reporting a vulnerability

Please do not open a public issue for a security vulnerability. Use GitHub's
[private vulnerability reporting](../PyTagShield/security/advisories/new)
for this repository. If private reporting is unavailable, open an issue titled
"Security contact request" with no vulnerability details so a private channel
can be arranged.

This is a solo-maintained project. There is no guaranteed response time, and no
SLA is promised.

## Security model and known limitations

- **Consent must be supplied by the caller.** PyTagShield enforces the configured
  policy, but it does not collect consent on your behalf. A missing or malformed
  consent signal fails closed.
- **Pseudonymization is salted hashing, not anonymization.** Treat pseudonymized
  identifiers as personal data and protect the salt.
- **Destination credentials are sensitive.** API keys are read from local
  configuration; never commit them or log resolved configuration.
- **HTTP destinations are caller-controlled.** The server performs no destination
  allowlisting beyond URL validation. Run it behind a trusted gateway with
  authentication, request limits, and egress policy when exposed.
- **Delivery is at-least-once unless your destination deduplicates.** Retries can
  produce duplicate events after a timeout or crash.
- **Dependency scanning runs in CI.** No audit proves the absence of
  vulnerabilities; review advisories before each release.

## Scope

The Rust core, Python bindings, and HTTP server in this repository are in scope.
Third-party services (GA4, Meta, PostHog, sGTM, webhooks) and your deployment
environment are out of scope.
