# lpcli

A command-line client for [Launchpad.net](https://launchpad.net), written in async Rust.

`lpcli` lets you interact with Launchpad from the terminal — query bugs, packages,
projects, people, Git repositories, specifications, questions, webhooks, snap recipes,
and more — without opening a browser. It also ships as a Rust library crate so other
applications can build on the same type-safe API wrappers.

## Features

- **Full Launchpad API coverage** — bugs, packages, projects, people, CVEs, Git,
  specs (blueprints), questions, webhooks, translations, snap recipes, and access tokens.
- **OAuth 1.0a authentication** — securely log in once; credentials are stored in
  `~/.config/lpcli/credentials.toml`.
- **Rich terminal output** — coloured text and formatted tables via
  [`colored`](https://crates.io/crates/colored) and
  [`comfy-table`](https://crates.io/crates/comfy-table).
- **Library crate** — all public API operations are exposed as an async Rust library
  that other crates can depend on.

## Requirements

- Rust 1.85+ (edition 2024)
- Ubuntu Linux (or any Linux distribution with OpenSSL available)

## Installation

### From source

```bash
git clone https://github.com/canonical/lpcli
cd lpcli
cargo build --release
# The binary is at target/release/lpcli
```

You can install it into your `$PATH`:

```bash
cargo install --path .
```

## Authentication

Most read operations work anonymously. To perform write operations (file bugs,
add comments, set statuses, etc.) you must first log in:

```bash
lpcli login
```

This starts the OAuth flow. Your browser will be opened to the Launchpad authorisation
page. After granting access, your credentials are saved to
`~/.config/lpcli/credentials.toml` for future use.

To verify authentication and check connectivity:

```bash
lpcli status
```

To remove your stored credentials:

```bash
lpcli logout
```

## Usage

```
lpcli <COMMAND> [SUBCOMMAND] [OPTIONS]
```

Run `lpcli --help` or `lpcli <COMMAND> --help` for the full list of options.

---

### Bugs

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

# Add a comment to a bug
lpcli bug comment --bug-id 123456 --message "Reproduced on noble."

# List all comments on a bug
lpcli bug comments --bug-id 123456

# File a new bug
lpcli bug create --target ubuntu --package curl --title "curl crashes on redirect" \
    --description "Steps to reproduce: ..."

# Change the status of bug tasks
lpcli bug set-status --bug-id 123456 --target ubuntu --package curl \
    --series noble --status "In Progress"

# Change status across multiple series at once
lpcli bug set-status --bug-id 123456 --target ubuntu --package curl \
    --many-series "noble, jammy" --status "Fix Released"

# Change status on every series the bug currently has a task for
lpcli bug set-status --bug-id 123456 --target ubuntu --package curl \
    --all-series --status "Fix Released"

# Change the importance of a bug task
lpcli bug set-importance --bug-id 123456 --target ubuntu --package curl \
    --series noble --importance "High"

# Assign a bug task to a user
lpcli bug set-assignee --bug-id 123456 --target ubuntu --package curl \
    --series noble --name jdoe

# Subscribe / unsubscribe a person
lpcli bug subscribe   --bug-id 123456 --name jdoe
lpcli bug unsubscribe --bug-id 123456 --name jdoe

# List subscribers
lpcli bug subscriptions --bug-id 123456

# Add a bug task (targeting a new project or series)
lpcli bug add-task --bug-id 123456 --target ubuntu --package curl \
    --series noble --status "New" --importance "Undecided"

# Delete a bug task
lpcli bug delete-task --bug-id 123456 --target ubuntu --package curl --series noble
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

# List bugs filed by or assigned to a person
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

# Search CVEs (optionally filtered by distribution)
lpcli cve search --distro ubuntu --limit 10

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
```

---

### Specifications (Blueprints)

```bash
# Show a specification
lpcli spec show --target launchpad --name feature-x

# List all specifications for a project
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
lpcli webhook ping --webhook-url "https://api.launchpad.net/devel/..."

# List recent deliveries for a webhook
lpcli webhook deliveries --webhook-url "https://api.launchpad.net/devel/..."

# Delete a webhook
lpcli webhook delete --webhook-url "https://api.launchpad.net/devel/..."
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

# List builds for a snap recipe
lpcli snap builds --owner jdoe --name my-snap

# Request new builds for a snap recipe
lpcli snap request-builds --owner jdoe --name my-snap
```

---

### Personal Access Tokens

```bash
# Manage access tokens for projects and Git repositories via the access-token subcommands
lpcli access-token --help
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
use lpcli::{auth, client::LaunchpadClient, bugs, packages};

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
    for pkg in &results.entries {
        println!("{} {}", pkg.source_package_name.as_deref().unwrap_or("?"),
                          pkg.source_package_version.as_deref().unwrap_or("?"));
    }
    Ok(())
}
```

### Error handling

All library functions return `lpcli::error::Result<T>`, which is an alias for
`std::result::Result<T, lpcli::error::LpError>`. Meaningful variants include:

| Variant | When raised |
|---------|-------------|
| `LpError::NotAuthenticated` | Credentials file is missing; run `lpcli login` |
| `LpError::NotFound` | The requested resource does not exist on Launchpad |
| `LpError::Api` | Launchpad returned a non-success HTTP status |
| `LpError::RateLimit` | Launchpad throttled the request (HTTP 429) |
| `LpError::Timeout` | The request timed out |

---

## Project Structure

```
src/
  lib.rs            — Public library crate root; re-exports all modules
  auth.rs           — OAuth 1.0a login flow and credential persistence
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
  access_tokens.rs  — Personal access tokens
  error.rs          — LpError type

  bin/
    lpcli.rs        — CLI binary (clap argument parsing and dispatch)
```

---

## Contributing

1. Fork the repository and create your branch from `main`.
2. Write idiomatic Rust following the conventions in
   [`.github/instructions/rust.instructions.md`](.github/instructions/rust.instructions.md).
3. Run `cargo fmt` and `cargo clippy -- -D warnings` before submitting.
4. Add or update tests in `tests/` for any new behaviour.
5. Open a pull request against `main`.

---

## License

Licensed under the [GNU General Public License v3.0 or later](LICENSE).

## AI / LLM Disclosure 

This project was generated, revised, and checked using large language model (LLM) tools.  Claude generated most of the code, Gemini checked the implementation against the Launchpad web API and documentation, and ChatGPT provided a code review.  The code has been partially human-reviewed and is currently being tested to ensure correct operation and refine the user experience and workflows.
