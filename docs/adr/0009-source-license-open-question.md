# ADR-0009: Source code license is an open question, not a resolved decision

## Status

Flagged for human/legal decision. The metadata inconsistency this ADR
exists to explain has been fixed, but the underlying business question
has not.

## Context

Early in this implementation, `Cargo.toml`'s workspace `license` field was
set to `"LicenseRef-CurseDelete-Commercial OR MIT"` -- an invented SPDX
expression with no corresponding license text anywhere in the repository.
The actual checked-in `LICENSE` file (present since the very first commit,
`fd1e77b`, before any CurseDelete-specific implementation work began) is
the Apache License 2.0, a generic bootstrap default rather than a
deliberate choice for this specific product.

This was a real inconsistency (`cargo metadata`/`cargo package` would
report a license with no basis in the actual repository content) and,
more importantly, a substantive business question that a Rust rewrite
should not silently paper over: **the product has a commercial licensing
model** (Community free / Education free / Business and Enterprise paid,
per `README-CurseDelete2.md` §§24-27 and `docs/LICENSING.md`). If the
*source code itself* is distributed under a permissive OSI license like
Apache-2.0 or MIT, that license's own grant of rights legally permits
anyone to compile the source and use it for any purpose, including
unrestricted commercial use, completely independent of `cursdel-license`'s
activation/entitlement system -- which would make the Business/Enterprise
"commercial use" restriction legally unenforceable against anyone willing
to build from source. `README-CurseDelete2.md`'s own guidance points at
exactly this tension: "Commercial restrictions can be legal/licensing
terms where technical enforcement would make the tool worse" -- i.e. the
intent is that commercial-use restriction is a *license terms* matter,
which requires the source license itself to actually carry that
restriction (a source-available license such as the Business Source
License, the Functional Source License, or a custom EULA -- the kind of
license used by comparable commercial-with-a-free-tier developer tools),
not a permissive OSI license that grants the opposite.

## Decision

1. `Cargo.toml`'s `license` field now reads `"Apache-2.0"`, matching what
   is *actually* checked into the repository as `LICENSE`, so the crate
   metadata is at least internally consistent and not asserting a license
   with no backing text.
2. This is recorded as a placeholder, not a considered decision. Choosing
   and drafting the actual license text a commercial product with a free
   community tier should ship under is a legal/business decision this
   implementation pass is not positioned to make unilaterally -- it has
   real consequences (what a competitor or reseller may legally do with
   the source, what "commercial use" actually excludes, jurisdiction and
   enforcement questions) that call for deliberate business/legal review,
   not an invented license string.

## What a human needs to decide

- Whether CurseDelete's source should ship under a source-available
  license with an explicit commercial-use carve-out (e.g. BSL 1.1, FSL,
  a custom EULA), a permissive OSI license with commercial restriction
  enforced purely through the separate `cursdel-license` activation
  system and terms-of-service (accepting that source availability makes
  the restriction only contractually, not technically or copyright-wise,
  enforceable), or a dual-license model (e.g. AGPL for community use with
  a paid commercial license buying out the copyleft obligation, a common
  pattern for exactly this kind of product).
- If a source-available/custom license is chosen, its actual legal text
  needs drafting or review by someone qualified to do so -- this is not
  something to generate from a product brief.

## Consequences

- Until this is resolved, treat the `Apache-2.0` `LICENSE` file as
  provisional infrastructure, not a statement that CurseDelete is
  permissively licensed open source software as a matter of product
  intent.
- `docs/LICENSING.md` (the *software* activation/entitlement licensing
  documentation) is unaffected by this ADR -- that system's design and
  implementation are unrelated to which *source code* license the
  repository itself carries, and nothing about `cursdel-license`,
  `cursdel-policy`, or the activation flow needs to change regardless of
  how this question is ultimately resolved.
