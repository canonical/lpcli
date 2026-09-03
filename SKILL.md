---
name: lpcli
description: 'Query and manage Launchpad.net via the lpcli CLI. USE FOR: counting or listing bugs against Ubuntu packages or projects, searching Launchpad bugs by status/tag/importance/keyword, looking up CVEs, finding source packages, checking people/teams, reviewing merge proposals, managing upload queues, and any question about Launchpad data. DO NOT USE FOR: GitHub issues, Jira, or non-Launchpad bug trackers.'
---

# lpcli Skill

Use the `lpcli` command-line tool to answer questions about
[Launchpad.net](https://launchpad.net) — Ubuntu's bug tracker, package archive,
and project hosting platform.

## When to Use

Use this skill whenever the user asks about **Launchpad** data, including:

- "How many bugs are opened against **rustc-1.97** in Launchpad?"
- "List Critical bugs for **curl** in Ubuntu"
- "Show me the bug tasks for LP #123456"
- "What packages are in the **noble** Security pocket?"
- "Find merge proposals needing review for a Launchpad Git repo"
- "What CVEs are linked to bug #123456?"
- Any question mentioning **Launchpad**, **LP**, Ubuntu **bugs**, Ubuntu **packages**,
  or Ubuntu **source packages** by name.

### Mapping natural-language questions to commands

| User intent | lpcli command |
|-------------|---------------|
| Count/list bugs for a **source package** | `lpcli bug search --target ubuntu --package <name>` |
| Count/list bugs for a **project** | `lpcli bug search --target <project>` |
| Filter bugs by status | Add `--status "New"` (or Confirmed, Triaged, In Progress, Fix Committed, Fix Released, etc.) |
| Filter bugs by importance | Add `--importance "Critical"` (or High, Medium, Low, Wishlist, Undecided) |
| Filter bugs by tag | Add `--tag <tag>` |
| Search bugs by keyword | Add `--keyword "<text>"` |
| Limit results | Add `--limit <n>` |

**Counting bugs**: `lpcli bug search` returns matching bugs. To count them,
pipe through `wc -l` or count the output entries. When the user asks "how many",
run the search and report the count.

## Prerequisites

- `lpcli` must be on `$PATH` (install via `cargo install --path .` from the
  [lpcli repo](https://github.com/canonical/lpcli)).
- Most **read** operations work **anonymously** (no login needed).
- **Write** operations require `lpcli login` first.

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

## Common Workflows

### Count open bugs for a source package

```bash
# "How many bugs are opened against rustc-1.97 in Launchpad?"
lpcli bug search --target ubuntu --package rustc-1.97
# To count: pipe output through wc -l or count the returned entries
```

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

1. **Run in terminal** — all Launchpad operations use `lpcli` commands; no
   browser or web scraping needed.
2. **Read operations are anonymous** — bug searches, package lookups, etc. work
   without `lpcli login`.
3. **Use `--help`** — every subcommand supports `--help` for full option details
   (e.g. `lpcli bug search --help`).
4. **Counting results** — `lpcli` outputs human-readable tables. To count bugs,
   pipe through `grep -c` or `wc -l` on the relevant output lines.
5. **Source package names in Launchpad** may differ from upstream project names
   (e.g. `rustc-1.97` not `rust`). Use the exact source package name the user
   provides.
