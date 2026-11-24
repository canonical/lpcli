# Project Overview

lpcli is an async Rust command-line client and public library for common
Launchpad.net workflows, with Ubuntu Linux as its primary platform. It provides
idiomatic, type-safe wrappers for bugs, packages, projects, people, CVEs, Git,
blueprints, questions, webhooks, translations, snap recipes, upload queues, and
access tokens.

The CLI parses and validates arguments, calls the library API wrappers, and
formats results for terminal display. Reusable API operations belong in the
library; argument handling, CLI authentication policy, configuration, and
presentation belong in the binary.

Authentication uses OAuth 1.0a credentials stored in the OS keyring with a
plaintext-file fallback, or hardware-bound OAuth grants backed by USB FIDO2
security keys. Classic login (`lpcli login`) prints a browser authorization URL
on stderr and waits for the user to authorize access and press Enter, so it
needs an interactive terminal. Logout (`lpcli logout`)
removes locally stored classic credentials and hardware-bound grants; server-side
revocation is a separate operation. USB hardware-token support and persisted
access-token coordination are currently implemented on Linux.

## Folder Structure

- `src/lib.rs`: public library crate root and API module declarations.
- `src/`: reusable API wrappers, HTTP client, authentication, downloads, and errors.
- `src/hardware_tokens.rs`: hardware-bound grants, token minting, cache, and coordination.
- `src/hardware_key.rs` and `src/hardware_key/`: security-key interfaces, CTAP2,
  PIN/UV protocols, CBOR, and Linux hidraw transport.
- `src/bin/lpcli.rs`: CLI argument parsing, validation, authentication policy, and dispatch.
- `src/bin/lpcli/commands/`: command handlers and their `Report`s, by domain.
- `src/bin/lpcli/config.rs`: configuration loading and validation.
- `src/bin/lpcli/display.rs`: terminal table formatting.
- `src/bin/lpcli/output.rs`: output modes, renderers, `--fields`, and structured errors.
- `src/bin/lpcli/tests.rs`: CLI parsing, documentation-example, and handler-helper tests.
- `src/bin/lpcli/tests/`: per-domain command snapshot tests.
- `docs/json-output.md`: the versioned JSON output contract.
- `tests/`: mocked HTTP integration tests; library modules also contain inline unit tests.
- `.github/workflows/`: build, lint, test, documentation, audit, and publishing workflows.
- `.devcontainer/`: the Ubuntu 26.04 / Rust 1.97 development environment.

## Coding Standards

- Follow the idiomatic Rust practices in `rust-instructions.md`.
- The minimum supported Rust version is 1.97, as declared in `Cargo.toml`.
- Keep changes focused and preserve the library/CLI separation described above.
- Prefer mature safe OS wrappers over custom FFI. For example, use the `nix` user
  lookup to obtain an owned passwd home-directory path. Document the safety
  requirements of any necessary unsafe code.
- Keep public API documentation, CLI help, and README examples consistent with
  implemented behavior.

## Authentication and Security

- Preserve explicit public-read boundaries: `--public` reads use an anonymous
  client, without loading credentials, prompting for a security key, or retrying
  with authentication.
- Preserve hardware-grant precedence and propagate hardware-authentication errors
  rather than automatically downgrading to classic credentials.
- Keep credentials, grant secrets, PINs, and assertions out of debug output and logs.
- Preserve private file permissions and the keyring-only persisted access-token
  cache. Refresh-grant file fallback and access-token caching are separate mechanisms.
- Resolve cache coordination paths from the effective UID's system passwd home,
  independently of HOME/XDG/TMPDIR overrides, with the checked runtime directory
  as fallback. Preserve ownership checks, cross-process locking, and generation fencing.
- Follow the existing signing and redirect restrictions so credentials remain
  scoped to trusted Launchpad endpoints.
- Keep `--no-input` (implied when stdin is not a terminal) failing before any
  prompt reads input; security-key touch notices still go to stderr.

## Output and the JSON Contract

JSON output is a versioned public contract (`docs/json-output.md`, version 1,
reported by `lpcli schema`). Every command except `skills` supports
`--format json`; list commands, `status`, and `hardware-token keys` also
support `--format tsv`. Within a contract version, only add commands,
optional lpcli-defined members, or error kinds. Renaming or removing an
lpcli-defined key, or changing the meaning of an exit code, needs a new
contract version.

- Print Launchpad entries and collections unchanged; put computed values
  under an `lpcli` member, never inside Launchpad entries.
- Map errors to exit codes only through `LpError::kind()` (0 success,
  1 general, 2 usage, 3 not authenticated, 4 not found, 5 permission denied,
  6 network, 7 partial). Return `LpError::Partial` after printing what
  succeeded when Launchpad or the filesystem may already reflect part of an
  operation.
- `--quiet` is for mutations only and prints resulting `self_link`s.
  `--fields` requires JSON output.

## Upload Queues and PPAs

Queue commands default to `{distro}/+archive/primary`. Use `--ppa OWNER/NAME`
(or `--archive ppa:OWNER/NAME`) with `queue search`, `download`, `info`,
`accept`, and `reject` to target a PPA; it resolves to
`/~OWNER/+archive/ubuntu/NAME`. The resolved archive is shown in search and
result output.

Launchpad auto-accepts PPA uploads, so PPA queue entries normally appear as
`Done` or `Accepted`, not `New`/`Unapproved`.

```bash
lpcli queue search --status done --series tenacious --name mutter \
    --ppa my-username/my-ppa
lpcli queue info --name mutter --status accepted --arch amd64 \
    --ppa my-username/my-ppa --series tenacious
```

## Validation

Use mocked HTTP responses and isolated filesystem/subprocess fixtures for routine
tests. Live Launchpad mutations require explicit authorization and a suitable test
account. Documentation examples should pass both clap parsing and `validate_cli`;
the CLI test suite checks examples in `README.md`, `AGENTS.md`, and `SKILL.md`
without dispatching commands or making network requests.

Command output is snapshot-tested with [insta](https://insta.rs): the JSON of
every read command and of representative mutations and partial failures, TSV
for list commands, and `rich`/`plain` for representative commands. Commands on the output layer (`src/bin/lpcli/output.rs`) build a
`Report` rendered in all four modes (`rich`, `plain`, `json`, `tsv`).
Snapshots live in `snapshots/` next to the tests; CI sets `INSTA_UPDATE=no`, so new or changed snapshots fail. Review
changes with `cargo insta review` (or `INSTA_UPDATE=always cargo test` and
`git diff`). Redact timestamps and mock-server URLs, and never snapshot
credentials. JSON output must come from Launchpad's raw JSON (`client::Raw`);
see `plans/decision_0001_preserve_launchpad_json.md`.

Run the CI-equivalent checks, plus doctests:

```sh
cargo fmt -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps --all-features
cargo build --locked --all-targets --all-features
cargo test --locked --all-targets --all-features
cargo test --locked --doc
```

CI also checks tracked filenames for ASCII-only names, audits dependencies
against the RustSec advisory database, and verifies that pull request authors
have signed the
[Canonical Contributor License Agreement (CLA)](https://canonical.com/legal/contributors).

## Persona and References

You are an Ubuntu expert with deep knowledge of Ubuntu releases, packages, and the
Launchpad.net web API. Provide guidance on Ubuntu source-package management,
package downloads, and release compatibility grounded in the implementation and
API documentation.

- Launchpad web API: https://api.launchpad.net/devel.html
- Launchpad web service documentation:
  https://documentation.ubuntu.com/launchpad/user/explanation/launchpad-api/launchpad-web-service/
