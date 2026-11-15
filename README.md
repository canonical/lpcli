# lpcli

A command-line client for [Launchpad.net](https://launchpad.net), written in async Rust.

**lpcli is beta software.  Please use it with caution and report any bugs you discover.**

`lpcli` lets you interact with Launchpad from the terminal — query bugs, packages,
projects, people, Git repositories, specifications, questions, webhooks, snap recipes,
and more — without opening a browser. It also ships as a Rust library crate so other
applications can build on the same type-safe API wrappers.

## Features

- **Broad Launchpad API coverage** — bugs, packages, projects, people, CVEs, Git,
  specs (blueprints), questions, webhooks, translations, snap recipes, package
  upload queues, and access tokens.
- **OAuth 1.0a authentication** — securely log in once; credentials are stored in
  the operating system keyring (Secret Service, Keychain, or Credential Manager),
  with a plaintext file fallback when no keyring is available.
- **Hardware-bound API tokens** — optionally bind your API access to a USB FIDO2
  security key (e.g. a YubiKey), so exchanging a stored grant requires the key.
- **Rich terminal output** — coloured text and formatted tables via
  [`colored`](https://crates.io/crates/colored) and
  [`comfy-table`](https://crates.io/crates/comfy-table).
- **Library crate** — all public API operations are exposed as an async Rust library
  that other crates can depend on.

## Requirements

- Ubuntu Linux (other Linux distributions may also work).
- For source builds: Rust 1.97 or newer and native build tools, including a C
  compiler, Make, CMake, and pkg-config. The provided development container uses
  Ubuntu 26.04 and Rust 1.97.
- HTTPS access to Launchpad; the HTTP client uses Rustls and system certificate
  verification.
- A Secret Service provider (e.g. GNOME Secret Service or KWallet) is
  recommended for secure credential storage. On Linux the credentials backend
  uses the D-Bus Secret Service API and vendors libdbus, so no system
  `libdbus-1-dev` package is required. Without a usable Secret Service, lpcli
  falls back to `~/.config/lpcli/credentials.toml`.

## Installation

### From source

```bash
git clone https://github.com/canonical/lpcli
cd lpcli
cargo build --locked --release
# The binary is at target/release/lpcli
```

You can install it into your `$PATH`:

```bash
cargo install --locked --path .
```

## Authentication

Reads that may include private data use authentication by default. To read
only public data without loading credentials or touching a security key, use
`--public`, for example `lpcli --public bug show --bug-id 123456`. Public-only
results may omit private objects, fields, or collection entries. Some public
distribution reads work anonymously by default. Use `--authenticated` to require
credentials for these reads too, for example:

```bash
lpcli --authenticated package search --series noble --name curl
```

`--public` and `--authenticated` are mutually exclusive. `--public` is available
for data reads, while mutations and identity/security-management commands require
authentication. To perform write operations (file bugs, add comments, set statuses,
etc.) you must log in:

```bash
lpcli login
```

This starts the OAuth flow. Open the printed URL in your browser, authorise access
on Launchpad, then press Enter in the terminal. Your credentials are saved to the
operating system keyring for future use. If no keyring is available, lpcli falls back to
`~/.config/lpcli/credentials.toml`.

To verify authentication and check connectivity:

```bash
lpcli status
```

To remove both your stored classic OAuth credentials and hardware-bound grant:

```bash
lpcli logout
```

Use `lpcli logout --classic` or `lpcli logout --hardware-token` to remove only the
selected type. Logout removes local credentials and clears the persisted hardware
access-token cache when removing the grant. Revoke authorisations on Launchpad to
invalidate credentials server-side; local logout does not revoke tokens or remove
access tokens already held by running processes.

### Hardware-bound API tokens

A classic login stores a long-lived OAuth access token: anything that can read
your keyring or files can use it. A **hardware-bound** login stores a
long-lived OAuth *refresh grant* instead, bound to a USB FIDO2 security key.
Exchanging the grant for a short-lived access token requires the key, so a
stolen grant is useless on its own. This works the same way as
[launchpadlib's hardware-bound tokens](https://documentation.ubuntu.com/launchpad/user/how-to/launchpadlib/use-hardware-bound-api-tokens/).

Requirements: a USB (HID) FIDO2 security key (NFC authenticators and platform
passkeys are not supported) and read/write access to its `/dev/hidraw*` device,
which most distributions grant through their FIDO udev rules. Root is not
required. Hardware-bound tokens are currently supported on Linux only.

1. Log in classically (the *bootstrap* login), then register your key. Touch
   the key when prompted:

   ```bash
   lpcli login
   lpcli hardware-token register "My YubiKey"
   lpcli hardware-token keys            # list registered keys and their IDs
   ```

   You can also register a key on the Launchpad website.

2. Issue a grant bound to the key. lpcli logs in with it straight away (touch
   the key again) and stores it:

   ```bash
   lpcli hardware-token issue --key "My YubiKey" --permission change-anything
   ```

   Grants default to, and are limited to, 365 days (`--expires` sets an earlier
   date) and cannot be broader than the bootstrap login's permission. Use
   `--print` to output the grant as JSON for launchpadlib's
   `login_with_hardware_token` instead.

3. Remove the classic bootstrap token, which does not need your key: revoke the
   `lpcli` authorisation at `https://launchpad.net/people/+me/+oauth-tokens`, then
   run `lpcli logout --classic`. Grants use the separate `lpcli-hardware`
   consumer, so this does not affect them.

From then on, lpcli uses the stored grant by default for authenticated commands.
A successful key check mints a short-lived access token for API requests.

#### Access-token cache

By default, lpcli caches the access token for up to five minutes across
invocations, bounded by the token's Launchpad expiry. The grace period starts
with the key check and does not extend when the token is used. Set
`authentication.hardware_token_cache_seconds` in `~/.config/lpcli/config.toml`
(or the platform-equivalent configuration directory) to an integer from 0 to 86400
seconds (one day):

```toml
[authentication]
hardware_token_cache_seconds = 300
```

Set it to `0` to disable cross-invocation caching. Access tokens are cached in
the OS keyring only, with no plaintext-file fallback. On Linux, cross-process
locking and a non-secret generation marker are kept in `~/.cache/lpcli`
(resolved from the account's system passwd home directory rather than
environment overrides, falling back to `/run/user/<uid>` if home is
unavailable). If the keyring or both coordination directories are unavailable,
each invocation requires a key touch; under `sudo`, access tokens are kept in
memory only. No security-key PINs or FIDO assertions are stored in the cache.

A cached access token is an ordinary short-lived OAuth token: anyone who obtains it
can use it without the security key until Launchpad expires it, even after the local
cache grace period ends. The local grace period limits lpcli's reuse, not the token's
server-side validity.

Clear the cached access token at any time, without deleting the hardware-bound
grant:

```bash
lpcli hardware-token clear-cache
```

Clearing does not revoke issued tokens on Launchpad or remove tokens already held
by running processes. The local deadline relies on the system clock.

#### Registering an additional key

Authentication and registration use **different keys**. With a hardware-bound
login, start with the key bound to your stored grant connected:

```bash
lpcli hardware-token register "Backup key"
```

Authenticate with the original key first. At the **Registration** prompt,
connect the new key and press Enter. You may unplug the original key at that
point, or leave both connected and select the new key. Device selection is
labelled separately for authentication and registration. If the short-lived
access token needs renewal later, reconnect the original key when prompted.
A new key cannot authenticate with a grant bound to the original key; selecting
it during authentication causes "This key has no matching credential".

Alternatively, use an existing classic OAuth session even when a hardware-bound
grant is stored:

```bash
lpcli hardware-token register "Backup key" --classic
```

This loads the OAuth session from the system keyring or the legacy
`~/.config/lpcli/credentials.toml` file. Only the new key is needed. If no classic
session remains, run `lpcli login` first. There is no automatic downgrade to
classic authentication when hardware authentication fails. Registration does
not replace your stored grant or issue a grant for the new key. After using a
new classic bootstrap session, revoke its authorization and run
`lpcli logout --classic` as above.

Other useful commands:

```bash
lpcli hardware-token show            # show the stored grant's configuration (no credentials)
lpcli hardware-token revoke          # revoke the stored grant and delete it
lpcli hardware-token revoke-key 42   # revoke a registered key
lpcli logout --hardware-token        # delete the stored grant only

# Import a grant issued elsewhere (the secret is prompted for, or read from
# LPCLI_HARDWARE_GRANT_SECRET):
lpcli login --hardware-token --account jdoe --grant-token "issued-oauth-token"
```

In the import example, replace `jdoe` with your Launchpad account and
`issued-oauth-token` with the token from your issued grant.

The grant is stored in the system keyring, falling back to
`~/.config/lpcli/hardware-token.json` (owner-only) when no keyring is
available. Set `LP_HARDWARE_CREDENTIALS_FILE` to use a specific file; the format
is the same as launchpadlib's, so both tools can share it. Under `sudo`, grants
are kept in memory only unless `LP_HARDWARE_CREDENTIALS_FILE` is set.

If a security key PIN is set, lpcli asks for it when Launchpad requests user
verification. Keys with built-in verification (e.g. fingerprint) use that
instead.

## Display width

To configure the output width, create `~/.config/lpcli/config.toml` (or the
platform-equivalent configuration directory):

```toml
[display]
width = 150
```

The configured width must be an integer between 1 and 65535 columns and takes
precedence over terminal detection, including when output is redirected. If no
width is configured, lpcli uses the terminal width when stdout is a terminal,
or 100 columns when output is redirected or the width cannot be detected.

Widths include table borders and padding. Long cell text wraps instead of being
truncated; extremely narrow outputs use a stacked `Field: value` layout. A
configured width is not capped to the physical terminal width.

## Usage

Run `lpcli --help` for the command list and `lpcli bug --help` for a command's
subcommands. For options on an individual operation:

```bash
lpcli bug show --help
```

The examples below use illustrative accounts, resource IDs, versions, and URLs.
Replace them with your own values and ensure referenced files exist. Reads that
require authentication assume you have logged in; use `--public` for public-only
reads. Mutation examples change Launchpad data and require appropriate permissions.

---

### Bugs

For bulk task mutations, `--all-series` includes **series-less tasks** (such as
upstream or distribution-level tasks) as well as release-specific tasks for the
matching targets. Inspect `lpcli bug tasks --bug-id 123456` before applying bulk
changes.

```bash
# Show a single bug
lpcli bug show --bug-id 123456

# List all tasks (project/package assignments) for a bug
lpcli bug tasks --bug-id 123456

# Search bugs on a project
lpcli bug search --target launchpad --status "New" --limit 10

# Search bugs for a specific source package in Ubuntu
lpcli bug search --target ubuntu --package firefox --status "Confirmed"

# Search bugs by keyword
lpcli bug search --target ubuntu --keyword "kernel panic" --limit 20

# Search bugs by tag
lpcli bug search --target ubuntu --tag "regression-update" --limit 10

# Search bugs by importance
lpcli bug search --target ubuntu --importance "Critical" --limit 10

# Add a comment to a bug
lpcli bug add-comment --bug-id 123456 --message "Reproduced on noble."

# List all comments on a bug
lpcli bug comments --bug-id 123456

# File a new bug
lpcli bug create --target ubuntu --package curl --title "curl crashes on redirect" \
    --description "Steps to reproduce: ..."

# Change the description of a bug (requires 'launchpad.Edit' permission on the bug)
lpcli bug set-description --bug-id 123456 \
    --description "Updated description with reproduction steps."

# Change the description of a bug from a file ('-' reads standard input)
lpcli bug set-description --bug-id 123456 --description-file ./description.txt

# Change the description of a bug from a pipe
cat description.txt | lpcli bug set-description --bug-id 123456 --description-file -

# Change the status of a bug task (--target is the name shown by 'lpcli bug tasks';
# status values are kebab-case: new, confirmed, triaged, in-progress,
# fix-committed, fix-released, invalid, wont-fix, opinion, expired)
lpcli bug set-status --bug-id 123456 --target curl \
    --series noble --status in-progress

# Change status across multiple series at once
lpcli bug set-status --bug-id 123456 --target curl \
    --many-series "noble, jammy" --status fix-released

# Change status on all matching tasks, including series-less tasks
lpcli bug set-status --bug-id 123456 --target curl \
    --all-series --status fix-released

# Change status across multiple targets, including their series-less tasks
lpcli bug set-status --bug-id 123456 --many-targets "rust-alacritty, rust-eza" \
    --all-series --status fix-released

# Change status on every current task (all targets, including series-less tasks)
lpcli bug set-status --bug-id 123456 --all-targets --all-series --status fix-released

# Change status only on series-less (distribution-level or upstream) tasks
lpcli bug set-status --bug-id 123456 --target curl \
    --no-series --status fix-released

# Change the importance of a bug task
lpcli bug set-importance --bug-id 123456 --target curl \
    --series noble --importance high

# Change importance on every current task, including series-less tasks
lpcli bug set-importance --bug-id 123456 --all-targets --all-series --importance critical

# Assign a bug task to a user
lpcli bug set-assignee --bug-id 123456 --target curl \
    --series noble --name jdoe

# Assign every current task, including series-less tasks
lpcli bug set-assignee --bug-id 123456 --all-targets --all-series --name jdoe

# Subscribe / unsubscribe a person
lpcli bug subscribe   --bug-id 123456 --name jdoe
lpcli bug unsubscribe --bug-id 123456 --name jdoe

# List subscribers
lpcli bug subscriptions --bug-id 123456

# Add a bug task (targeting a new project or series)
lpcli bug add-task --bug-id 123456 --target ubuntu --package curl \
    --series noble --status new --importance undecided

# Add a bug task and link it to a remote bug (e.g. forward to Debian)
lpcli bug add-task --bug-id 2120577 --target debian --package arctica-greeter \
    --bug-tracker debbugs --remote-bug 1150409

# List the Launchpad tracker names accepted by --bug-tracker
lpcli bug remote-bug-trackers
# Search tracker metadata and cap results; -s and -l are short forms.
lpcli bug remote-bug-trackers --search debian --limit 10

# Link an existing bug task to a remote bug tracker
lpcli bug link-remote --bug-id 2120577 --target debian --bug-tracker debbugs --remote-bug 1150409

# List bug watches (remote bug links) for a bug
lpcli bug watches --bug-id 2120577

# Unlink a bug task from its remote bug watch
lpcli bug unlink-remote --bug-id 2120577 --target debian

# Delete a bug task
lpcli bug delete-task --bug-id 123456 --target ubuntu --package curl --series noble

# Add a tag to a bug
lpcli bug tag --bug-id 123456 --add-tag regression

# Add multiple tags
lpcli bug tag --bug-id 123456 --add-many-tags "regression, oem, sru"

# Remove a tag from a bug
lpcli bug tag --bug-id 123456 --remove-tag regression

# Remove multiple tags
lpcli bug tag --bug-id 123456 --remove-many-tags "regression, oem"

# Remove all tags from a bug
lpcli bug tag --bug-id 123456 --remove-all-tags

# Replace all tags (clear + add in one invocation)
lpcli bug tag --bug-id 123456 --remove-all-tags --add-many-tags "new-tag-1, new-tag-2"
```

---

### People

```bash
# Show a Launchpad person or team
lpcli person show --name jdoe

# Search by name
lpcli person search --query "John Doe"

# List members of a team
lpcli person members --team ubuntu-security

# List bug tasks for bugs filed by a person
lpcli person bugs --name jdoe

# List PPAs owned by a person
lpcli person ppas --name jdoe

# List teams owned by a person
lpcli person owned-teams --name jdoe
```

---

### Packages

```bash
# Show information about an Ubuntu distro series
lpcli package series --series noble

# List all Ubuntu distro series
lpcli package list-series

# Search published source packages in a series
lpcli package search --series noble --name curl
lpcli package search --series noble --pocket Security

# Show information about a distribution
lpcli package distro

# Show info about a PPA
lpcli package ppa --owner jdoe --ppa my-ppa

# List source packages in a PPA
lpcli package ppa-sources --owner jdoe --ppa my-ppa --name curl

# Download source files for a published source package
lpcli package download --name curl --series noble
lpcli package download --name curl --series noble --version 8.5.0-2ubuntu10
lpcli package download --name curl --series noble --output ~/Downloads
```

---

### Projects

```bash
# Show a project
lpcli project show --name launchpad

# Search projects
lpcli project search --query "ubuntu desktop"

# List milestones for a project
lpcli project milestones --project launchpad
lpcli project milestones --project launchpad --active   # active only

# Show a specific milestone
lpcli project show-milestone --project launchpad --name 1.0

# List all series for a project
lpcli project list-series --project launchpad

# Show a project series
lpcli project series-show --project launchpad --series trunk

# List releases in a project series
lpcli project series-releases --project launchpad --series trunk
```

---

### CVEs

```bash
# Show a CVE
lpcli cve show --sequence 2024-1234

# Search CVEs in Ubuntu (--distro defaults to ubuntu)
lpcli cve search --distro ubuntu --limit 10

# Search CVEs in Ubuntu by keyword
lpcli cve search --keyword "buffer overflow" --limit 20

# List CVEs linked to a bug
lpcli cve bug-cves --bug-id 123456
```

---

### Git Repositories

```bash
# Show a Git repository
lpcli git show --path "~jdoe/launchpad/+git/myrepo"

# Show the default repository for a project
lpcli git default --target launchpad

# List repositories owned by a person
lpcli git list-person-repos --name jdoe

# List branches and tags in a repository
lpcli git refs --path "~jdoe/launchpad/+git/myrepo"

# List merge proposals for a repository
lpcli git proposals --path "~jdoe/launchpad/+git/myrepo"
lpcli git proposals --path "~jdoe/launchpad/+git/myrepo" --status "Needs review"

# List code review comments on a merge proposal
lpcli git comments --path "~jdoe/launchpad/+git/myrepo" --id 12345

# Select a preview diff when listing inline review comments
lpcli git comments --path "~jdoe/launchpad/+git/myrepo" --id 12345 --diff 67890

# List code review diffs on a merge proposal
lpcli git diffs --path "~jdoe/launchpad/+git/myrepo" --id 12345
```

---

### Specifications (Blueprints)

```bash
# Show a specification
lpcli spec show --target launchpad --name feature-x

# List currently valid specifications for a project
lpcli spec list --target launchpad

# List all specifications (including non-current)
lpcli spec list --target launchpad --all
```

---

### Questions (Answers / Support)

```bash
# Show a question by ID
lpcli question show --question-id 42

# Search questions on a project
lpcli question search --target ubuntu --query "nvidia driver"
lpcli question search --target ubuntu --status "Open"

# Show messages on a question
lpcli question messages --target ubuntu --question-id 42
```

---

### Webhooks

For operations on an existing webhook, use its API `self_link` as `--webhook-url`.
The URL below illustrates that format; substitute your own webhook's link.

```bash
# List webhooks for a project or Git repository
lpcli webhook list --target launchpad
lpcli webhook list --target "~jdoe/launchpad/+git/myrepo"

# Create a webhook
lpcli webhook create --target launchpad \
    --delivery-url https://example.com/hook \
    --event-types "git:push:0.1,merge-proposal:0.1"

# Create an inactive webhook with a shared secret
lpcli webhook create --target launchpad \
    --delivery-url https://example.com/hook \
    --event-types "git:push:0.1" \
    --inactive \
    --secret "mysecret"

# Send a test ping to a webhook
lpcli webhook ping --webhook-url "https://api.launchpad.net/devel/launchpad/+webhook/1"

# List recent deliveries for a webhook
lpcli webhook deliveries --webhook-url "https://api.launchpad.net/devel/launchpad/+webhook/1"

# Delete a webhook
lpcli webhook delete --webhook-url "https://api.launchpad.net/devel/launchpad/+webhook/1"
```

---

### Translations

```bash
# List translation import queue entries for a distro series
lpcli translation queue --series noble

# List translation templates for a distro series
lpcli translation templates --series noble
```

---

### Snap Recipes

```bash
# Show a snap recipe
lpcli snap show --owner jdoe --name my-snap

# Find snap recipes owned by a person
lpcli snap find --owner jdoe

# List pending builds for a snap recipe
lpcli snap builds --owner jdoe --name my-snap

# Request new builds for a snap recipe
lpcli snap request-builds --owner jdoe --name my-snap
```

---

### Package Upload Queues

When `--series` is omitted, queue commands use the distribution's current
development series. Specify it explicitly to work on a stable release. Use
`--version` to disambiguate matching uploads; accepting multiple matching versions
without it fails rather than choosing one. The mutation examples below select a
specific series and version.

```bash
# Search packages in the upload queue for a distro series
lpcli queue search --status "New"
lpcli queue search --status "New" --name curl
lpcli queue search --status "Unapproved" --series noble --pocket Security

# Search with exact match on name/version
lpcli queue search --status "New" --name curl --version 8.5.0-2ubuntu10 --exact-match

# Download files from a package in the upload queue
lpcli queue download --name curl --status "New" --arch source
lpcli queue download --name curl --status "New" --arch amd64 --output ~/Downloads

# Display binary package details for a queue item
lpcli queue info --name curl --status "New" --arch amd64
lpcli queue info --name curl --version 8.5.0-2ubuntu10 --status "New" --arch source

# Accept a package from the queue (requires queue-admin permissions)
lpcli queue accept --name curl --version 8.5.0-2ubuntu10 --status "New" \
    --arch source --series noble

# Reject a package from the queue (requires queue-admin permissions)
lpcli queue reject --name curl --version 8.5.0-2ubuntu10 --status "New" \
    --arch source --series noble --comment "Needs a bug fix."
```

---

### Personal Access Tokens

For revocation, use the token's API `self_link` as `--token-url`; substitute your
own token's link for the illustrative URL below.

```bash
# Issue a personal access token for a project
lpcli access-token issue --project launchpad --description "CI token" \
    --scopes "repository:push,repository:build_status"

# Issue a personal access token for a Git repository
lpcli access-token issue-git --repo "~jdoe/launchpad/+git/myrepo" \
    --description "Deploy key" --scopes "repository:push"

# List access tokens for a project
lpcli access-token list --project launchpad

# List access tokens for a Git repository
lpcli access-token list-git --repo "~jdoe/launchpad/+git/myrepo"

# Revoke an access token
lpcli access-token revoke --token-url "https://api.launchpad.net/devel/launchpad/+access-token/1"
```

---

## Using lpcli as a Rust Library

Add it to your `Cargo.toml`:

```toml
[dependencies]
lpcli = { git = "https://github.com/canonical/lpcli" }
tokio = { version = "1", features = ["full"] }
```

### Unauthenticated usage

```rust
use lpcli::{client::LaunchpadClient, bugs};

#[tokio::main]
async fn main() -> lpcli::error::Result<()> {
    let lp = LaunchpadClient::new(None);
    let bug = bugs::get_bug(&lp, 123456).await?;
    println!("Bug #{}: {}", bug.id, bug.title);
    Ok(())
}
```

### Authenticated usage

```rust
use lpcli::{auth, client::LaunchpadClient, packages};

#[tokio::main]
async fn main() -> lpcli::error::Result<()> {
    // Load credentials stored by `lpcli login`
    let creds = auth::load_credentials()?;
    let lp = LaunchpadClient::new(Some(creds));

    // Search for source packages in Ubuntu Noble
    let params = packages::SourceSearchParams {
        source_name: Some("curl"),
        ..Default::default()
    };
    let results = packages::search_published_sources(&lp, "ubuntu", "noble", &params).await?;
    for pkg in &results {
        println!("{} {}", pkg.source_package_name.as_deref().unwrap_or("?"),
                          pkg.source_package_version.as_deref().unwrap_or("?"));
    }
    Ok(())
}
```

### Error handling

Fallible API operations use `lpcli::error::Result<T>`, which is an alias for
`std::result::Result<T, lpcli::error::LpError>`. Meaningful variants include:

| Variant | When raised |
|---------|-------------|
| `LpError::NotAuthenticated` | No usable authentication credentials are available; run `lpcli login` |
| `LpError::NotFound` | The requested resource does not exist on Launchpad |
| `LpError::Api` | Launchpad returned a non-success HTTP status |
| `LpError::RateLimit` | Launchpad throttled the request (HTTP 429) |
| `LpError::Timeout` | The request timed out |
| `LpError::HardwareKey` | A security-key operation failed (no key, permissions, wrong PIN) |
| `LpError::HardwareTokenGrantRejected` | Launchpad rejected a hardware-bound grant |

### Hardware-bound usage

```rust
use std::sync::Arc;

use lpcli::{client::LaunchpadClient, hardware_key::UsbAuthenticator, hardware_tokens};

#[tokio::main]
async fn main() -> lpcli::error::Result<()> {
    // Load the grant stored by `lpcli hardware-token issue`.
    let store = hardware_tokens::GrantStore::from_env();
    let session = hardware_tokens::load_session(&store, Arc::new(UsbAuthenticator::default()))?
        .ok_or(lpcli::error::LpError::NotAuthenticated)?;
    // If no reusable cached access token is available, the first request mints one.
    // Touch the key when prompted.
    let lp = LaunchpadClient::with_hardware_token(Arc::new(session));
    let bug = lpcli::bugs::get_bug(&lp, 1).await?;
    println!("Bug #{}: {}", bug.id, bug.title);
    Ok(())
}
```

---

## Project Structure

```
src/
  lib.rs            — Public library crate root; exposes the API modules
  auth.rs           — OAuth 1.0a login flow and credential persistence
  hardware_tokens.rs — Hardware-bound OAuth grants, token minting and renewal
  hardware_key.rs   — USB FIDO2 security-key interface
  hardware_key/     — CTAP2, PIN/UV protocols, CBOR, and Linux hidraw transport
  client.rs         — Low-level HTTP client for the Launchpad REST API
  bugs.rs           — Bug tracking
  packages.rs       — Source packages, distro series, PPAs
  projects.rs       — Projects and milestones
  people.rs         — People and teams
  cves.rs           — CVE lookup
  git.rs            — Git repositories and merge proposals
  specifications.rs — Blueprints / specs
  questions.rs      — Answers / support questions
  webhooks.rs       — Webhook management
  translations.rs   — Translation queues and templates
  snaps.rs          — Snap recipes and builds
  queue.rs          — Package upload queue management
  download.rs       — File download utilities
  access_tokens.rs  — Personal access tokens
  status.rs         — Server status and connectivity checks
  error.rs          — LpError type

  bin/
    lpcli.rs        — CLI binary (clap argument parsing, authentication policy, dispatch)
    lpcli/
      config.rs     — Configuration loading and validation
      display.rs    — Terminal output formatting
      tests.rs      — CLI argument, documentation-example, and handler-helper tests

tests/              — Mocked HTTP integration tests
```

---

## Contributing

1. Fork the repository and create your branch from `main`.
2. Write idiomatic Rust following [the project rules](AGENTS.md) and
   [Rust conventions](rust-instructions.md).
3. Add or update inline library tests, CLI tests in `src/bin/lpcli/tests.rs`, or
   integration tests in `tests/` for changed behaviour. Use mocked HTTP responses
   and isolated filesystem/subprocess fixtures for routine validation.
4. Run the CI-equivalent checks below and the documentation tests.
5. Open a pull request against `main`.

```bash
cargo fmt -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
RUSTDOCFLAGS="-D warnings" cargo doc --locked --no-deps --all-features
cargo build --locked --all-targets --all-features
cargo test --locked --all-targets --all-features
cargo test --locked --doc
```

CI also checks tracked filenames for ASCII-only names and audits dependencies
against the RustSec advisory database. CLI tests parse the `lpcli` examples in this
README and `AGENTS.md`, including argument validation, without executing commands
or making Launchpad requests.

---

## License

Licensed under the [GNU General Public License v3.0 or later](LICENSE).

## AI / LLM Disclosure

This project was generated, revised, and checked using large language model (LLM) tools.  Claude generated most of the code, Gemini checked the implementation against the Launchpad web API and documentation, and ChatGPT provided a code review.  The code has been partially human-reviewed and is currently being tested to ensure correct operation and refine the user experience and workflows.
