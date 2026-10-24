# Competitive intelligence

## OSS gap analysis — 2026-10-24

Scope: Compared PyTagShield's implemented server-side consent gate, minimization, routing and delivery pipeline against the actively maintained OSS consent managers and consent-native collection layers users pair with it. Assessed where enforcement happens, what is persisted, destination coverage, standards support and packaging; the repo has no `AUDIT_FINDINGS.md` or `docs/ARCHITECTURE.md`, so the README and source were the primary evidence.

### Comparable OSS projects
| Project | GitHub | License | Last activity | Why it is the comparison |
| --- | --- | --- | --- | --- |
| CookieConsent v3 | https://github.com/orestbida/cookieconsent | MIT | Last commit 2026-05-01 | Widely deployed vanilla-JS consent manager; the client-side alternative teams use to gate analytics tags |
| tarteaucitron.js | https://github.com/AmauriC/tarteaucitron.js | MIT | Last commit 2026-10-15 | OSS CMP with service-level controls and Google Consent Mode v2; closest functional substitute for consent collection |
| walkerOS | https://github.com/elbwalker/walkerOS | MIT | Pushed 2026-10-23; 4,238 commits | Consent-native collection layer that queues events until consent and routes to destinations — the closest OSS analogue to the whole pipeline, not just the banner |

Klaro (https://github.com/kiprotect/klaro, BSD-3-Clause) was reviewed as a hint but its last commit is 2025-03-27, so it is not treated as actively maintained here. Osano and OneTrust are commercial and were not assessed as OSS.

### Capability comparison
| Capability | Ours | CookieConsent | tarteaucitron | walkerOS | Gap |
| --- | --- | --- | --- | --- | --- |
| Consent preference UI | No | Yes | Yes | Partial | We have no banner |
| Purpose/service-level choices | Yes | Yes | Yes | Yes | Parity |
| Enforcement before vendor egress | Yes | Partial | Partial | Yes | Ours is server-side |
| Consent persistence | Partial | Yes | Yes | Partial | Ours is in-memory only |
| Durable consent audit | Partial | Partial | Partial | Partial | Transport log is bounded |
| PII minimization / pseudonymization | Yes | No | No | No | Ours only |
| Deterministic transforms | Yes | No | No | Yes | Ours and walkerOS |
| Destination routing | Yes | No | Partial | Yes | Ours is server-side |
| Destination breadth | Partial | No | Partial | Yes | 5 real destination types |
| Schema validation | Yes | Partial | Partial | Yes | Ours validates events and strict config |
| Bounded queues + retry/backoff | Yes | No | No | Partial | Ours only |
| HTTP ingest server | Yes | No | Partial | Partial | Ours is runnable |
| Python API | Yes | No | No | No | Ours only |
| Consent Mode v2 / TCF / GPP | No | Partial | Yes | Partial | We lack standard signals |
| Self-hosted | Yes | Yes | Yes | Yes | Parity |

"Partial" enforcement for the CMPs is important: CookieConsent and tarteaucitron block configured scripts in the browser, but tarteaucitron's own README warns that initialising the banner does not block scripts already loaded elsewhere. PyTagShield's gate sits at the server boundary, so a directly loaded vendor tag cannot bypass it. "Partial" persistence for walkerOS reflects that its consent queue/state is not a durable consent ledger in the sense a regulated deployment needs; that was not verified in depth.

### Gaps that matter (ranked)
1. **The consent store is in-memory and bounded** — `crates/pytagshield-core/src/consent.rs` implements a `ConsentStore` with capacity and TTL eviction, and the README roadmap lists "pluggable persistent consent store (Redis/Postgres)" as open. A restart loses every stored decision, which makes the server unsuitable for production consent state. Both mature CMPs persist to browser storage; a server-side authority must persist too. Effort M.
2. **No consent capture surface or standard signal support** — PyTagShield expects the caller to supply `consent` in the event or via `set_consent`; it has no banner/preference centre and no Google Consent Mode v2, TCF v2 or GPP translation. tarteaucitron advertises Consent Mode v2 (Google, Bing, Piano); CookieConsent is widely integrated with CMP flows. Teams must run a second tool and bridge its state manually. Effort M, but the right answer is integration, not a new banner.
3. **Destination breadth is thin for an activation data plane** — only PostHog, GA4, Meta CAPI, sGTM, webhook and log exist. There is no warehouse, Braze, Klaviyo, TikTok, LinkedIn or generic ad-platform CAPI, so users still need another pipeline for most destinations. walkerOS ships a broader destination set. Effort M.
4. **No durable audit trail for consent decisions** — the delivery log is bounded and the consent store is ephemeral; there is no append-only ledger of grant/revoke events with timestamps and provenance beyond the current snapshot. Regulated users need to prove what was granted when. This is a "weaker version" of a feature the design already gestures at with `consent_source`. Effort M.
5. **Alpha packaging limits adoption** — PyPI wheels are macOS arm64 only for CPython 3.10+, other platforms build from source, and container images are still an unchecked roadmap item. The JavaScript comparators deploy anywhere, and walkerOS ships npm packages. Effort S-M.
6. **No documented performance envelope** — the README lists benchmarks and a performance envelope as open. The project claims bounded queues and backpressure but gives no throughput/latency numbers, which makes capacity planning for a server-side gate hard. Effort S.

### Where we are differentiated
- Enforcement happens before egress, server-side, with a default-deny policy and explicit precedence `event → stored → configured default`; the CMP comparators are browser-side and bypassable if a tag loads directly.
- Minimization is part of the data plane: PII key dropping, property/depth/string caps, IP clearing and salted SHA-256 pseudonymization happen before routing. None of the three comparators does this.
- Per-destination purpose gates with explicit skip reasons, transform/minimization counts and Prometheus metrics make the decision auditable at the event level.
- The versioned `pytagshield.consent.v1` event contract carries granted purposes, the full decision map and provenance, which is more explicit than a binary consent cookie.
- One Rust core with both a Python API and an HTTP server, Apache-2.0, fully self-hosted.

### Disposition
- **Build:** a persistent consent store with an append-only decision ledger (Redis/Postgres), Consent Mode v2/TCF translation, and 5-10 more verified destinations.
- **Integrate/reuse:** pair with CookieConsent, tarteaucitron or Klaro as the capture UI and consume their state; do not build a banner or a legal-compliance suite.
- **Defer/reject:** full CMP feature parity, a visual preference centre, mobile SDKs and a broad ad-tech catalog remain out of scope until persistence and packaging are production-grade.

### Sources
- https://github.com/orestbida/cookieconsent (MIT, commit feed 2026-05-01, accessed 2026-10-24)
- https://github.com/AmauriC/tarteaucitron.js (MIT, Consent Mode v2, commit feed 2026-10-15, accessed 2026-10-24)
- https://github.com/elbwalker/walkerOS (MIT, consent-native collector, pushed 2026-10-23, accessed 2026-10-24)
- https://github.com/kiprotect/klaro (BSD-3-Clause, last commit 2025-03-27, accessed 2026-10-24)
- Repo files inspected: `README.md`, `Cargo.toml`, `docs/squid-consent-contract.md`, `crates/pytagshield-core/src/consent.rs`, `crates/pytagshield-core/src/minimize.rs`
