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
- Run `lpcli login` first. Many commands, including common reads such as
  `bug show` and `bug search`, require authentication — do not assume any
  command works anonymously.
- If the user logged in with a **hardware-bound token** (`lpcli hardware-token
  issue` or `lpcli login --hardware-token`), each `lpcli` invocation needs a
  touch of their USB security key (and possibly its PIN). Tell the user before
  running commands. `lpcli status` shows which login is in use.

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
| Edit a bug description | `lpcli bug set-description --bug-id 123456 --description "new text"` or `--description-file ./desc.txt` (`-` reads stdin) |
| Set bug task status | `lpcli bug set-status --bug-id 123456 --target curl --series noble --status in-progress` |
| Set status on multiple series | `lpcli bug set-status --bug-id 123456 --target curl --many-series "noble, jammy" --status fix-released` |
| Set status on all series | `lpcli bug set-status --bug-id 123456 --target curl --all-series --status fix-released` |
| Set status on multiple targets | `lpcli bug set-status --bug-id 123456 --many-targets "rust-alacritty, rust-eza" --all-series --status fix-released` |
| Set status on all targets | `lpcli bug set-status --bug-id 123456 --all-targets --all-series --status fix-released` |
| Set status on series-less tasks | `lpcli bug set-status --bug-id 123456 --target curl --no-series --status fix-released` |
| Set importance | `lpcli bug set-importance --bug-id 123456 --target curl --series noble --importance high` |
| Set importance on all targets | `lpcli bug set-importance --bug-id 123456 --all-targets --all-series --importance critical` |
| Assign a bug task | `lpcli bug set-assignee --bug-id 123456 --target curl --series noble --name my-username` |
| Assign on all targets | `lpcli bug set-assignee --bug-id 123456 --all-targets --all-series --name my-username` |
| Subscribe a person | `lpcli bug subscribe --bug-id 123456 --name my-username` |
| Unsubscribe a person | `lpcli bug unsubscribe --bug-id 123456 --name my-username` |
| List subscribers | `lpcli bug subscriptions --bug-id 123456` |
| Add a bug task | `lpcli bug add-task --bug-id 123456 --target ubuntu --package curl --series noble --status new --importance undecided` |
| Add a bug task with remote bug link | `lpcli bug add-task --bug-id 123456 --target debian --package curl --bug-tracker debbugs --remote-bug 654321` |
| Link bug task to remote bug | `lpcli bug link-remote --bug-id 123456 --target debian --bug-tracker debbugs --remote-bug 654321` |
| Link bug task to a GitHub issue/PR | `lpcli bug link-remote --bug-id 123456 --target ubuntu --package curl --github "owner/repo#658"` |
| Unlink remote bug watch from task | `lpcli bug unlink-remote --bug-id 123456 --target debian` |
| List bug watches (Linked Tasks column shows task links) | `lpcli bug watches --bug-id 123456` |
| Find a remote bug tracker | `lpcli bug remote-bug-trackers --search "canonical/apport" --limit 20` |
| Find remote bug trackers as JSON | `lpcli bug remote-bug-trackers --search "github" --json` |
| Register a bug tracker | `lpcli bug tracker create --base-url https://github.com/OWNER/REPO/issues --type github` |
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
| Show a person or team | `lpcli person show --name my-username` |
| Search people | `lpcli person search --query "John Doe"` |
| List team members | `lpcli person members --team ubuntu-security` |
| List bugs for a person | `lpcli person bugs --name my-username` |
| List PPAs | `lpcli person ppas --name my-username` |
| List owned teams | `lpcli person owned-teams --name my-username` |

### Packages

| Action | Command |
|--------|---------|
| Show a distro series | `lpcli package series --series noble` |
| List all distro series | `lpcli package list-series` |
| Search published sources | `lpcli package search --series noble --name curl` |
| Search by pocket | `lpcli package search --series noble --pocket Security` |
| Show distribution info | `lpcli package distro` |
| Show a PPA | `lpcli package ppa --owner my-username --ppa my-ppa` |
| List PPA sources | `lpcli package ppa-sources --owner my-username --ppa my-ppa --name curl` |
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
| Show a repo | `lpcli git show --path "~my-username/launchpad/+git/myrepo"` |
| Show default repo | `lpcli git default --target launchpad` |
| List person repos | `lpcli git list-person-repos --name my-username` |
| List refs (branches/tags) | `lpcli git refs --path "~my-username/launchpad/+git/myrepo"` |
| List merge proposals | `lpcli git proposals --path "~my-username/launchpad/+git/myrepo"` |
| Filter merge proposals | `lpcli git proposals --path "~my-username/launchpad/+git/myrepo" --status "Needs review"` |
| List MP comments | `lpcli git comments --path "~my-username/launchpad/+git/myrepo" --id 12345` |
| List MP diffs | `lpcli git diffs --path "~my-username/launchpad/+git/myrepo" --id 12345` |

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
| Show a snap recipe | `lpcli snap show --owner my-username --name my-snap` |
| Find snap recipes | `lpcli snap find --owner my-username` |
| List builds | `lpcli snap builds --owner my-username --name my-snap` |
| Request builds | `lpcli snap request-builds --owner my-username --name my-snap` |

### Package Upload Queues

| Action | Command |
|--------|---------|
| Search queue items | `lpcli queue search --status "New"` |
| Search by name | `lpcli queue search --status "New" --name curl` |
| Search with pocket/series | `lpcli queue search --status "Unapproved" --series noble --pocket Security` |
| Exact match search | `lpcli queue search --status "New" --name curl --version 8.5.0-2ubuntu10 --exact-match` |
| Search a PPA queue | `lpcli queue search --status done --ppa my-username/my-ppa --series tenacious` |
| Download queue files | `lpcli queue download --name curl --status "New" --arch source` |
| Download PPA queue files | `lpcli queue download --name mutter --status done --arch source --ppa my-username/my-ppa --series tenacious` |
| Show binary details | `lpcli queue info --name curl --status "New" --arch amd64` |
| Show PPA queue details | `lpcli queue info --name mutter --status done --arch source --ppa my-username/my-ppa --series tenacious` |
| Accept a package | `lpcli queue accept --name curl --status "New" --arch source` |
| Accept from a PPA | `lpcli queue accept --name mutter --version 51.0-1ubuntu4 --status unapproved --arch source --ppa my-username/my-ppa --series tenacious` |
| Reject a package | `lpcli queue reject --name curl --status "New" --arch source --comment "Reason"` |
| Reject from a PPA | `lpcli queue reject --name mutter --version 51.0-1ubuntu4 --status unapproved --arch source --ppa my-username/my-ppa --series tenacious --comment "Reason"` |

### Access Tokens

| Action | Command |
|--------|---------|
| Issue token for project | `lpcli access-token issue --project launchpad --description "CI" --scopes "repository:push"` |
| Issue token for Git repo | `lpcli access-token issue-git --repo "~my-username/launchpad/+git/myrepo" --description "Deploy" --scopes "repository:push"` |
| List project tokens | `lpcli access-token list --project launchpad` |
| List Git repo tokens | `lpcli access-token list-git --repo "~my-username/launchpad/+git/myrepo"` |
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
    --series noble --status triaged
lpcli bug set-importance --bug-id 123456 --target curl \
    --series noble --importance high
lpcli bug comment --bug-id 123456 --message "Triaged as High for Noble."
```

### Forward a bug to Debian or upstream

```bash
# Add a Debian task and link it to the Debian BTS bug in one step
lpcli bug add-task --bug-id 123456 --target debian --package curl \
    --bug-tracker debbugs --remote-bug 654321

# Or link an existing task to the remote bug
lpcli bug link-remote --bug-id 123456 --target debian --package curl \
    --bug-tracker debbugs --remote-bug 654321

# Check bug watches on the bug
lpcli bug watches --bug-id 123456
```

### Link a Launchpad bug to a GitHub issue or PR

lpcli manages the repository tracker and bug watch automatically. It
validates the configuration and resulting watch URL, reuses an existing
matching watch instead of duplicating one, and prints the final watch URL.

```bash
# Documentation example (not executed):
lpcli bug link-remote -b 2174521 -t ubuntu -p apport --github "canonical/apport#658"
lpcli bug watches -b 2174521
```

- Name the task: `-t` (plus `-p` for a source-package task in a distribution)
  or `-u/--task-url`. Without a selector, linking succeeds only when the bug
  has exactly one task. When `-t`/`-p` match several tasks and `-s` is
  omitted, a single series-less (distribution-level or upstream) task is
  preferred; pass `-s` or `-u` to link a release task. Run
  `lpcli bug tasks -b 2174521` first to check which task will change.
  `--github "OWNER/REPO#N"` takes a plain numeric N (issues and PRs share one
  numbering sequence) and avoids the legacy mis-typed `github-issues` tracker.
- Expected watch URL: https://github.com/canonical/apport/issues/658
  (GitHub redirects /issues/N to /pull/N for PRs). Verify the expected URL
  AND the intended task in Linked Tasks; listing a watch is not proof of
  reachability — optionally probe anonymously:
  `curl -IL https://github.com/canonical/apport/issues/658`
- Wrong watch? `bug unlink-remote` detaches the selected task link only;
  watch deletion is web-UI-only:
  `https://bugs.launchpad.net/bugs/<bug>/+watch/<watch-id>`.

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
lpcli git proposals --path "~my-username/launchpad/+git/myrepo" --status "Needs review"
```

### Review the upload queue

```bash
lpcli queue search --status "New" --name curl
lpcli queue info --name curl --status "New" --arch source
lpcli queue download --name curl --status "New" --arch source
lpcli queue accept --name curl --status "New" --arch source
```

### Review a PPA upload queue

Use `--ppa OWNER/NAME` (or `--archive ppa:OWNER/NAME`, or the full
`~OWNER/+archive/ubuntu/NAME` reference) to target a PPA; `--ppa` and
`--archive` are mutually exclusive, and PPAs are Ubuntu-only. PPA uploads are
normally auto-accepted, so search with `--status done` or `--status accepted`
(a newly built binary can be accepted while it waits for publication). The
search output names the queried archive: a PPA label such as
`(~my-username/my-ppa)`, or the distribution name for the primary archive,
such as `(ubuntu)`. PPA reads are authenticated by default; `--public` opts
into an anonymous read of a public PPA and may omit private data.

```bash
lpcli queue search --status done --ppa my-username/my-ppa --series tenacious
lpcli queue info --name mutter --status done --arch source --ppa my-username/my-ppa --series tenacious
lpcli queue download --name mutter --status done --arch source --ppa my-username/my-ppa --series tenacious
lpcli queue accept --name mutter --version 51.0-1ubuntu4 --status unapproved --arch source --ppa my-username/my-ppa --series tenacious
lpcli queue reject --name mutter --version 51.0-1ubuntu4 --status unapproved --arch source --ppa my-username/my-ppa --series tenacious --comment "Reason"
```

The accept/reject examples apply only when an upload is genuinely queued
(`new` or `unapproved`); use the status reported by `queue search`. Accept and
reject default to `--status New`.

### Download source packages

```bash
lpcli package download --name curl --series noble
lpcli package download --name curl --series noble --version 8.5.0-2ubuntu10 --output ~/src
```

### Check a person's activity

```bash
lpcli person show --name my-username
lpcli person bugs --name my-username
lpcli person ppas --name my-username
```

---

## Tips for Agents

1. **Run in terminal** — nearly all Launchpad operations use `lpcli`
   commands; no browser or web scraping needed. The one known exception is
   deleting a bug watch, which is web-UI-only (see the GitHub linking
   workflow above).
2. **Log in first** — run `lpcli login` before using `lpcli`; many commands
   (bug searches, package lookups, etc.) require authentication.
3. **Use `--help`** — every subcommand supports `--help` for full option details
   (e.g. `lpcli bug search --help`).
4. **Counting results** — `lpcli` outputs human-readable tables. To count bugs,
   pipe through `grep -c` or `wc -l` on the relevant output lines.
5. **Source package names in Launchpad** may differ from upstream project names
   (e.g. `rustc-1.97` not `rust`). Use the exact source package name the user
   provides.
6. **Self-contained documentation** — `lpcli skills` (alias `lpcli skill`)
   prints this entire document straight from the binary, byte for byte, with
   no authentication or network access; redirect it into a skill directory if
   your agent tooling expects one.
