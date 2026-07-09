# lpcli Skill

> Interact with [Launchpad.net](https://launchpad.net) from the command line
> or as an async Rust library.

## What is lpcli?

`lpcli` is a command-line client and Rust library for the
[Launchpad.net](https://launchpad.net) web API. It covers bugs, packages,
projects, people/teams, CVEs, Git repositories, specifications (blueprints),
questions, webhooks, translations, snap recipes, package upload queues, and
personal access tokens.

Repository: <https://github.com/canonical/lpcli>

---

## Installation

```bash
# Clone and build from source (requires Rust 1.88+)
git clone https://github.com/canonical/lpcli
cd lpcli
cargo build --release
# Binary: target/release/lpcli

# Or install into $PATH
cargo install --path .
```

---

## Authentication

Most **read** operations work anonymously. **Write** operations require OAuth
login first.

```bash
# Log in (opens browser for OAuth authorisation)
lpcli login

# Check authentication and connectivity
lpcli status

# Log out and remove stored credentials
lpcli logout
```

Credentials are stored in `~/.config/lpcli/credentials.toml`.

---

## CLI Reference

General syntax:

```
lpcli <COMMAND> [SUBCOMMAND] [OPTIONS]
```

Run `lpcli --help` or `lpcli <COMMAND> --help` for full option details.

### Bugs

| Action | Command |
|--------|---------|
| Show a bug | `lpcli bug show --bug-id 123456` |
| List bug tasks | `lpcli bug tasks --bug-id 123456` |
| Search bugs on a project | `lpcli bug search --target launchpad --status "New" --limit 10` |
| Search bugs for a package | `lpcli bug search --target ubuntu --package firefox --status "Confirmed"` |
| Search bugs by keyword | `lpcli bug search --target ubuntu --keyword "kernel panic" --limit 20` |
| Search bugs by tag | `lpcli bug search --target ubuntu --tag "regression-update" --limit 10` |
| Search bugs by importance | `lpcli bug search --target ubuntu --importance "Critical" --limit 10` |
| Add a comment | `lpcli bug comment --bug-id 123456 --message "Reproduced on noble."` |
| List comments | `lpcli bug comments --bug-id 123456` |
| File a new bug | `lpcli bug create --target ubuntu --package curl --title "title" --description "desc"` |
| Set bug task status | `lpcli bug set-status --bug-id 123456 --target curl --series noble --status "In Progress"` |
| Set status on multiple series | `lpcli bug set-status --bug-id 123456 --target curl --many-series "noble, jammy" --status "Fix Released"` |
| Set status on all series | `lpcli bug set-status --bug-id 123456 --target curl --all-series --status "Fix Released"` |
| Set status on multiple targets | `lpcli bug set-status --bug-id 123456 --many-targets "rust-alacritty, rust-eza" --all-series --status "Fix Released"` |
| Set status on all targets | `lpcli bug set-status --bug-id 123456 --all-targets --all-series --status "Fix Released"` |
| Set status on series-less tasks | `lpcli bug set-status --bug-id 123456 --target curl --no-series --status "Fix Released"` |
| Set importance | `lpcli bug set-importance --bug-id 123456 --target curl --series noble --importance "High"` |
| Set importance on all targets | `lpcli bug set-importance --bug-id 123456 --all-targets --all-series --importance "Critical"` |
| Assign a bug task | `lpcli bug set-assignee --bug-id 123456 --target curl --series noble --name jdoe` |
| Assign on all targets | `lpcli bug set-assignee --bug-id 123456 --all-targets --all-series --name jdoe` |
| Subscribe a person | `lpcli bug subscribe --bug-id 123456 --name jdoe` |
| Unsubscribe a person | `lpcli bug unsubscribe --bug-id 123456 --name jdoe` |
| List subscribers | `lpcli bug subscriptions --bug-id 123456` |
| Add a bug task | `lpcli bug add-task --bug-id 123456 --target ubuntu --package curl --series noble --status "New" --importance "Undecided"` |
| Delete a bug task | `lpcli bug delete-task --bug-id 123456 --target ubuntu --package curl --series noble` |
| Add a tag | `lpcli bug tag --bug-id 123456 --add-tag regression` |
| Add multiple tags | `lpcli bug tag --bug-id 123456 --add-many-tags "regression, oem, sru"` |
| Remove a tag | `lpcli bug tag --bug-id 123456 --remove-tag regression` |
| Remove multiple tags | `lpcli bug tag --bug-id 123456 --remove-many-tags "regression, oem"` |
| Remove all tags | `lpcli bug tag --bug-id 123456 --remove-all-tags` |
| Replace all tags | `lpcli bug tag --bug-id 123456 --remove-all-tags --add-many-tags "new-tag-1, new-tag-2"` |

### People & Teams

| Action | Command |
|--------|---------|
| Show a person or team | `lpcli person show --name jdoe` |
| Search people | `lpcli person search --query "John Doe"` |
| List team members | `lpcli person members --team ubuntu-security` |
| List bugs for a person | `lpcli person bugs --name jdoe` |
| List PPAs | `lpcli person ppas --name jdoe` |
| List owned teams | `lpcli person owned-teams --name jdoe` |

### Packages

| Action | Command |
|--------|---------|
| Show a distro series | `lpcli package series --series noble` |
| List all distro series | `lpcli package list-series` |
| Search published sources | `lpcli package search --series noble --name curl` |
| Search by pocket | `lpcli package search --series noble --pocket Security` |
| Show distribution info | `lpcli package distro` |
| Show a PPA | `lpcli package ppa --owner jdoe --ppa my-ppa` |
| List PPA sources | `lpcli package ppa-sources --owner jdoe --ppa my-ppa --name curl` |
| Download source files | `lpcli package download --name curl --series noble` |
| Download specific version | `lpcli package download --name curl --series noble --version 8.5.0-2ubuntu10` |

### Projects

| Action | Command |
|--------|---------|
| Show a project | `lpcli project show --name launchpad` |
| Search projects | `lpcli project search --query "ubuntu desktop"` |
| List milestones | `lpcli project milestones --project launchpad` |
| List active milestones | `lpcli project milestones --project launchpad --active` |
| Show a milestone | `lpcli project show-milestone --project launchpad --name 1.0` |
| List project series | `lpcli project list-series --project launchpad` |
| Show a project series | `lpcli project series-show --project launchpad --series trunk` |
| List series releases | `lpcli project series-releases --project launchpad --series trunk` |

### CVEs

| Action | Command |
|--------|---------|
| Show a CVE | `lpcli cve show --sequence 2024-1234` |
| Search CVEs | `lpcli cve search --distro ubuntu --limit 10` |
| Search CVEs by keyword | `lpcli cve search --keyword "buffer overflow" --limit 20` |
| List CVEs for a bug | `lpcli cve bug-cves --bug-id 123456` |

### Git Repositories

| Action | Command |
|--------|---------|
| Show a repo | `lpcli git show --path "~jdoe/launchpad/+git/myrepo"` |
| Show default repo | `lpcli git default --target launchpad` |
| List person repos | `lpcli git list-person-repos --name jdoe` |
| List refs (branches/tags) | `lpcli git refs --path "~jdoe/launchpad/+git/myrepo"` |
| List merge proposals | `lpcli git proposals --path "~jdoe/launchpad/+git/myrepo"` |
| Filter merge proposals | `lpcli git proposals --path "~jdoe/launchpad/+git/myrepo" --status "Needs review"` |
| List MP comments | `lpcli git comments --path "~jdoe/launchpad/+git/myrepo" --id 12345` |
| List MP diffs | `lpcli git diffs --path "~jdoe/launchpad/+git/myrepo" --id 12345` |

### Specifications (Blueprints)

| Action | Command |
|--------|---------|
| Show a spec | `lpcli spec show --target launchpad --name feature-x` |
| List specs | `lpcli spec list --target launchpad` |
| List all specs (incl. non-current) | `lpcli spec list --target launchpad --all` |

### Questions (Support)

| Action | Command |
|--------|---------|
| Show a question | `lpcli question show --question-id 42` |
| Search questions | `lpcli question search --target ubuntu --query "nvidia driver"` |
| Search by status | `lpcli question search --target ubuntu --status "Open"` |
| Show question messages | `lpcli question messages --target ubuntu --question-id 42` |

### Webhooks

| Action | Command |
|--------|---------|
| List webhooks | `lpcli webhook list --target launchpad` |
| Create a webhook | `lpcli webhook create --target launchpad --delivery-url https://example.com/hook --event-types "git:push:0.1,merge-proposal:0.1"` |
| Ping a webhook | `lpcli webhook ping --webhook-url "<URL>"` |
| List deliveries | `lpcli webhook deliveries --webhook-url "<URL>"` |
| Delete a webhook | `lpcli webhook delete --webhook-url "<URL>"` |

### Translations

| Action | Command |
|--------|---------|
| List import queue | `lpcli translation queue --series noble` |
| List templates | `lpcli translation templates --series noble` |

### Snap Recipes

| Action | Command |
|--------|---------|
| Show a snap recipe | `lpcli snap show --owner jdoe --name my-snap` |
| Find snap recipes | `lpcli snap find --owner jdoe` |
| List builds | `lpcli snap builds --owner jdoe --name my-snap` |
| Request builds | `lpcli snap request-builds --owner jdoe --name my-snap` |

### Package Upload Queues

| Action | Command |
|--------|---------|
| Search queue items | `lpcli queue search --status "New"` |
| Search by name | `lpcli queue search --status "New" --name curl` |
| Search with pocket/series | `lpcli queue search --status "Unapproved" --series noble --pocket Security` |
| Exact match search | `lpcli queue search --status "New" --name curl --version 8.5.0-2ubuntu10 --exact-match` |
| Download queue files | `lpcli queue download --name curl --status "New" --arch source` |
| Show binary details | `lpcli queue info --name curl --status "New" --arch amd64` |
| Accept a package | `lpcli queue accept --name curl --status "New" --arch source` |
| Reject a package | `lpcli queue reject --name curl --status "New" --arch source --comment "Reason"` |

### Access Tokens

| Action | Command |
|--------|---------|
| Issue token for project | `lpcli access-token issue --project launchpad --description "CI" --scopes "repository:push"` |
| Issue token for Git repo | `lpcli access-token issue-git --repo "~jdoe/launchpad/+git/myrepo" --description "Deploy" --scopes "repository:push"` |
| List project tokens | `lpcli access-token list --project launchpad` |
| List Git repo tokens | `lpcli access-token list-git --repo "~jdoe/launchpad/+git/myrepo"` |
| Revoke a token | `lpcli access-token revoke --token-url "<URL>"` |

---

## Using lpcli as a Rust Library

Add to `Cargo.toml`:

```toml
[dependencies]
lpcli = { git = "https://github.com/canonical/lpcli" }
tokio = { version = "1", features = ["full"] }
```

### Unauthenticated example

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

### Authenticated example

```rust
use lpcli::{auth, client::LaunchpadClient, packages};

#[tokio::main]
async fn main() -> lpcli::error::Result<()> {
    let creds = auth::load_credentials()?;
    let lp = LaunchpadClient::new(Some(creds));

    let params = packages::SourceSearchParams {
        source_name: Some("curl"),
        ..Default::default()
    };
    let results = packages::search_published_sources(&lp, "ubuntu", "noble", &params).await?;
    for pkg in &results {
        println!("{} {}",
            pkg.source_package_name.as_deref().unwrap_or("?"),
            pkg.source_package_version.as_deref().unwrap_or("?"));
    }
    Ok(())
}
```

### Error handling

All library functions return `lpcli::error::Result<T>` (alias for
`std::result::Result<T, lpcli::error::LpError>`).

| Variant | Meaning |
|---------|---------|
| `LpError::NotAuthenticated` | No credentials; run `lpcli login` |
| `LpError::NotFound` | Resource does not exist on Launchpad |
| `LpError::Api` | Launchpad returned a non-success HTTP status |
| `LpError::RateLimit` | Launchpad throttled the request (HTTP 429) |
| `LpError::Timeout` | Request timed out |

### Library modules

| Module | Purpose |
|--------|---------|
| `lpcli::bugs` | Bug tracking (show, search, create, comment, tasks, status, tags) |
| `lpcli::packages` | Source packages, distro series, PPAs, downloads |
| `lpcli::projects` | Projects, milestones, series |
| `lpcli::people` | People and teams |
| `lpcli::cves` | CVE lookup |
| `lpcli::git` | Git repositories, refs, merge proposals |
| `lpcli::specifications` | Blueprints / specs |
| `lpcli::questions` | Answers / support questions |
| `lpcli::webhooks` | Webhook management |
| `lpcli::translations` | Translation queues and templates |
| `lpcli::snaps` | Snap recipes and builds |
| `lpcli::queue` | Package upload queue management |
| `lpcli::download` | File download utilities |
| `lpcli::access_tokens` | Personal access tokens |
| `lpcli::auth` | OAuth login/logout and credential management |
| `lpcli::client` | `LaunchpadClient` — low-level HTTP client |
| `lpcli::status` | Server status and connectivity checks |
| `lpcli::error` | `LpError` error type |

---

## Common Workflows

### Triage a bug

```bash
lpcli bug show --bug-id 123456
lpcli bug tasks --bug-id 123456
lpcli bug set-status --bug-id 123456 --target curl \
    --series noble --status "Triaged"
lpcli bug set-importance --bug-id 123456 --target curl \
    --series noble --importance "High"
lpcli bug comment --bug-id 123456 --message "Triaged as High for Noble."
```

### Find packages in a series

```bash
lpcli package search --series noble --name curl
lpcli package search --series noble --pocket Security
```

### Investigate a CVE

```bash
lpcli cve show --sequence 2024-1234
lpcli cve bug-cves --bug-id 123456
```

### Review merge proposals

```bash
lpcli git proposals --path "~jdoe/launchpad/+git/myrepo" --status "Needs review"
```

### Review the upload queue

```bash
lpcli queue search --status "New" --name curl
lpcli queue info --name curl --status "New" --arch source
lpcli queue download --name curl --status "New" --arch source
lpcli queue accept --name curl --status "New" --arch source
```

### Download source packages

```bash
lpcli package download --name curl --series noble
lpcli package download --name curl --series noble --version 8.5.0-2ubuntu10 --output ~/src
```

### Check a person's activity

```bash
lpcli person show --name jdoe
lpcli person bugs --name jdoe
lpcli person ppas --name jdoe
```

---

## Tips for Agents

1. **No browser needed** — all Launchpad operations run in the terminal via
   `lpcli`, making it ideal for automated and scripted workflows.
2. **Read operations are anonymous** — you can query bugs, packages, projects,
   people, CVEs, and more without authentication.
3. **Write operations require login** — run `lpcli login` first, which stores
   OAuth credentials in `~/.config/lpcli/credentials.toml`.
4. **Use `--help`** — every command and subcommand supports `--help` for full
   option details (e.g. `lpcli bug search --help`).
5. **Parse output** — `lpcli` outputs human-readable coloured text and tables.
   When scripting, pipe through standard text-processing tools.
6. **Launchpad API docs** — the underlying web API is documented at
   <https://api.launchpad.net/devel.html> and
   <https://documentation.ubuntu.com/launchpad/user/explanation/launchpad-api/launchpad-web-service/>.
7. **Library usage** — for deeper integration, use `lpcli` as an async Rust
   library crate (see the Rust library section above).
