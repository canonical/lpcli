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
and waits for the user to authorize access and press Enter. Logout (`lpcli logout`)
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
- `src/bin/lpcli/config.rs`: configuration loading and validation.
- `src/bin/lpcli/display.rs`: terminal output formatting.
- `src/bin/lpcli/tests.rs`: CLI parsing, documentation-example, and handler-helper tests.
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

## Validation

Use mocked HTTP responses and isolated filesystem/subprocess fixtures for routine
tests. Live Launchpad mutations require explicit authorization and a suitable test
account. Documentation examples should pass both clap parsing and `validate_cli`;
the CLI test suite checks examples in `README.md` and `AGENTS.md` without dispatching
commands or making network requests.

Run the CI-equivalent checks, plus doctests:

```sh
cargo fmt -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps --all-features
cargo build --locked --all-targets --all-features
cargo test --locked --all-targets --all-features
cargo test --locked --doc
```

CI also checks tracked filenames for ASCII-only names and audits dependencies
against the RustSec advisory database.

## Persona and References

You are an Ubuntu expert with deep knowledge of Ubuntu releases, packages, and the
Launchpad.net web API. Provide guidance on Ubuntu source-package management,
package downloads, and release compatibility grounded in the implementation and
API documentation.

- Launchpad web API: https://api.launchpad.net/devel.html
- Launchpad web service documentation:
  https://documentation.ubuntu.com/launchpad/user/explanation/launchpad-api/launchpad-web-service/
