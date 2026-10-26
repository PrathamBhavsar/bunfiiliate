# Security Policy

bunfiiliate takes security and financial correctness seriously. This document outlines our vulnerability disclosure process, supported versions, and security boundaries.

---

## Supported Versions

bunfiiliate is currently under active pre-release development. Security patches are applied to the active development branches.

| Version | Supported | Notes |
| --- | --- | --- |
| `0.1.0-alpha` / pre-release | Yes | Active development on `staging` and `prod` |
| `< 0.1.0` | No | Experimental snapshots |

---

## Reporting a Vulnerability

**Please do not report security vulnerabilities via public GitHub issues, discussions, or pull requests.**

To report a vulnerability:

1. **GitHub Security Advisories (Preferred)**: Navigate to the **Security** tab of the bunfiiliate repository, select **Advisories**, and click **Report a vulnerability**. This creates a confidential advisory draft visible only to project maintainers.
2. **Private Disclosure**: If the GitHub advisory workflow is unavailable, contact the project maintainers via the private contact channels specified on the project repository.

### What to Include

To help us triage and resolve the issue quickly, please include:

- A clear description of the vulnerability and its potential impact.
- Step-by-step reproduction instructions or a minimal proof of concept (PoC).
- Affected components (e.g., `bf-spool`, `crates/api`, `crates/db`, `crates/tracker`).
- Any proposed mitigations or remediation patches.

### Response Timeline

- **Acknowledgment**: Within 48 hours of receipt.
- **Triage & Assessment**: Within 5 business days, including confirmation of severity and affected components.
- **Remediation & Advisory**: We will coordinate the release of a patch alongside a public advisory crediting the reporter (if desired).

---

## Scope & Threat Model

### In-Scope Vulnerabilities

We welcome reports regarding flaws that undermine the confidentiality, integrity, or availability of bunfiiliate:

1. **Financial Ledger & State Machines**:
   - Bypassing append-only constraints or deferred zero-sum triggers on `ledger_entries`.
   - Manipulation of affiliate balances, unauthorized debit/credit postings, or double spending.
   - Integer overflow, underflow, or rounding discrepancies in basis-point commission calculations (`CommissionPlan`).
   - Invalid state machine transitions permitting unauthorized payout approvals or conversion completions.

2. **Authentication & Authorization**:
   - Flaws in hand-rolled JWT creation, verification, or expiration (`bf_at`).
   - Refresh token rotation flaws or failure of family-wide token revocation upon token reuse (`bf_rt`).
   - Bypassing the `Origin` header check middleware on mutating requests (CSRF).
   - Privilege escalation between roles (`affiliate`, `admin`, `super_admin`).
   - Cross-tenant data leakage violating workspace isolation boundaries (`Principal::workspace_scope`).

3. **Tracker Hot Path & Ingestion**:
   - Ingestion payload poisoning, buffer manipulation, or memory safety bugs in `crates/tracker` or `bf-spool`.
   - Replay vulnerabilities or checkpoint corruption leading to unrecoverable WAL state.
   - Denial-of-service conditions causing unhandled panics or crash loops in the tracker service.
   - Bypassing HMAC-SHA256 signature verification on generic conversion webhooks, Stripe webhook endpoints, or Paddle webhook endpoints.

4. **Cryptographic & Traffic Hygiene Rules**:
   - Constant-time verification bypasses in signature validation routines.
   - Flaws in IP hashing or salt handling enabling visitor re-identification.
   - Bypassing fraud rules (`IpVelocity`, `BotUa`, `Dedupe`) through header spoofing.

### Out-of-Scope Vulnerabilities

The following areas are explicitly excluded from our threat model for v1 (see [`docs/PLAN.md`](docs/PLAN.md) and [`brain/decisions/0003-skip-list.md`](brain/decisions/0003-skip-list.md)):

- **Plaintext Database Secrets**: Storing webhook secrets and third-party integration keys in database columns without a dedicated Key Management Service (KMS). bunfiiliate v1 operates on a trusted, single-VPS database model.
- **Missing Enterprise Auth**: Lack of single sign-on (SSO), SAML, hardware tokens, or two-factor authentication (2FA) in v1.
- **Volumetric Denial of Service**: High-volume HTTP flood attacks against unauthenticated public endpoints (`/r/:code`, `/c`). Production instances are required to be deployed behind a reverse proxy (e.g., Caddy, Cloudflare) providing transport-level rate limiting and TLS termination.
- **Compromised Host Environment**: Attacks requiring direct root or administrative shell access to the host server, or direct read/write access to the PostgreSQL database cluster port (`5433`).
- **Insecure Development Flags**: Issues arising solely when `COOKIE_SECURE=0` or plain HTTP is deliberately configured by the operator.
