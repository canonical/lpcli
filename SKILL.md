---
name: lpcli
description: 'Query and manage Launchpad.net via the lpcli CLI. USE FOR: counting or listing bugs against Ubuntu packages or projects, searching Launchpad bugs by status/tag/importance/keyword, marking bugs as duplicates, looking up CVEs, finding source packages, checking people/teams, reviewing merge proposals, managing upload queues, and any question about Launchpad data. DO NOT USE FOR: GitHub issues, Jira, or non-Launchpad bug trackers.'
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
| Count/list bugs for a **source package** | `lpcli bug search --target ubuntu --package curl` (use the package name) |
| Count/list bugs for a **project** | `lpcli bug search --target launchpad` (use the project name) |
| Filter bugs by status | Add `--status "New"` (or Confirmed, Triaged, In Progress, Fix Committed, Fix Released, etc.) |
| Filter bugs by importance | Add `--importance "Critical"` (or High, Medium, Low, Wishlist, Undecided) |
| Filter bugs by tag | Add `--tag <tag>` |
| Search bugs by keyword | Add `--keyword "<text>"` |
| Limit results | Add `--limit <n>` |

**Counting bugs**: `lpcli bug search` returns one page of matching bugs, up
to `--limit` (default 25). Never count output lines (for example with
`wc -l`); table borders, headings, and wrapped rows make that wrong. Use
`--format json` and read Launchpad's `total_size` (the count of all matches),
or count `entries` for the returned page. When the user asks "how many",
report `total_size`. If Launchpad sent `total_size_link` instead, the total is
not in the output; say so rather than reporting the page size as the total.

```bash
lpcli --format json --fields title bug search --target ubuntu --package rustc-1.97 --limit 1
```

## Prerequisites

- `lpcli` must be on `$PATH` (install via `cargo install --path .` from the
  [lpcli repo](https://github.com/canonical/lpcli)).
- The user must run `lpcli login` first, in their own terminal: it waits for
  them to authorize lpcli in a browser and press Enter, so it fails (exit
  code 2) when stdin is not a terminal. Many commands, including common reads
  such as `bug show` and `bug search`, require authentication — do not assume
  any command works anonymously. Exit code 3 means the user is not logged in.
- If the user logged in with a **hardware-bound token** (through the
  `hardware-token issue` command or `lpcli login --hardware-token`), each
  `lpcli` invocation needs a touch of their USB security key (and possibly its
  PIN). Tell the user before running commands. `lpcli status` shows which
  login is in use.

## CLI Reference

General syntax:

```
lpcli <COMMAND> [SUBCOMMAND] [OPTIONS]
```

Run `lpcli --help`, or `--help` after any command (for example
`lpcli bug --help`), for full option details.

### Output formats

The global options go before or after the command, for example
`lpcli --format json bug show --bug-id 123456`.

**Agents should use `--format json`.** It works with every command except
`skills`. stdout then contains only data; prompts, notices, and warnings go
to stderr. Set `LPCLI_FORMAT=json` to make it the default for a session.

- JSON is Launchpad's own representation. A single object is printed as the
  Launchpad entry. Lists use Launchpad's collection shape,
  `{"total_size", "start", "entries"}`, so read `.entries[]`.
- Commands that combine several requests print named members, for example
  `bug show` prints `{"bug", "bug_tasks", "attachments", "lpcli"}`. Values
  lpcli computes itself are under `lpcli`.
- **Save tokens with `--fields`**: `--fields name,title,web_link` keeps only
  those Launchpad attributes in each entry. It requires JSON output.
- `--format tsv` prints a header row and exactly one line per record. It
  works with list commands, `status`, and `hardware-token keys`.
- `--quiet` makes a mutation print only the resulting `self_link`, one per
  line. It is rejected on reads.
- `--no-input` makes prompts fail instead of waiting, and is automatic when
  stdin is not a terminal. `lpcli login` and `hardware-token register` need
  the user's terminal; ask the user to run them. Security-key touches still
  need the user.
- `--no-pager` disables `$PAGER` paging when running human-facing modes in a
  terminal.
- `--progress none` suppresses download progress output on stderr (`--format json`
  and `--format tsv` already suppress progress output).
- `lpcli --format json schema` reports the JSON contract version
  (currently 1).
- `--json` is **no longer accepted**; it fails with an "unexpected argument"
  error. Use `--format json`, and read `.entries[]` instead of `.[]`.

### Exit codes and errors

Check the exit code instead of parsing messages:

| Code | Meaning | What to do |
|---:|---|---|
| 0 | Success | |
| 1 | General error | Read the message |
| 2 | Usage or validation error | Fix the command; check `--help` |
| 3 | Not authenticated | Ask the user to run `lpcli login`, or use `--public` for public data |
| 4 | Not found | Check the name, ID, or series |
| 5 | Permission denied | The account lacks the required Launchpad permission |
| 6 | Network failure, timeout, or rate limiting | Retry later |
| 7 | Partial success | Part of the change was applied; read stdout before retrying |

In JSON mode, errors are one JSON object on stderr:
`{"error": {"kind": "not_found", "message": "...", "hint": null, "http_status": 404}}`.
Command-line usage errors (exit code 2) are plain text from the argument
parser. After exit code 7, stdout lists what succeeded, with failures under
`lpcli` (for example `lpcli.failures` for bug changes to several tasks), so
retry only the parts that failed.

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
| Add a comment | `lpcli bug add-comment --bug-id 123456 --message "Reproduced on noble."` or `--message-file ./reply.txt` (`-` reads stdin) |
| List comments | `lpcli bug comments --bug-id 123456` |
| Find a comment index | `lpcli --format json bug comments --bug-id 123456`; the JSON's `lpcli.comment_indexes` array lists them |
| Edit a comment in place | `lpcli bug edit-comment --bug-id 123456 --comment-index 3 --message-file ./reply.txt` (`-` reads stdin); the author-only edit keeps the old text as a revision |
| File a new bug | `lpcli bug create --target ubuntu --package curl --title "title" --description "desc"` |
| Edit a bug description | `lpcli bug set-description --bug-id 123456 --description "new text"` or `--description-file ./desc.txt` (`-` reads stdin) |
| Mark a bug as a duplicate | `lpcli bug mark-duplicate --bug-id 123456 --duplicate-of 654321` |
| Remove a duplicate marker | `lpcli bug unmark-duplicate --bug-id 123456` |
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
| Find remote bug trackers as JSON | `lpcli bug remote-bug-trackers --search "github" --format json --fields name,base_url` |
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
| Show PPA build results | `lpcli package builds --ppa my-username/my-ppa --name mutter --state failed --exclude-state dependency-wait` (supports `--owner`+`--ppa` or `--ppa OWNER/NAME`, repeatable `--arch`/`--state`/`--exclude-state`, `--version`, `--pocket`, and optional `--series`) |
| Show primary-archive build results | `lpcli package builds --series resolute --name mutter --version 50.1-0ubuntu2.5` (`--series` is required for the primary archive) |
| Show a single build record | `lpcli package build show --build-id 33824576 --ppa my-username/my-ppa` (or `--build-url <SELF_LINK>`, or omit `--ppa` for primary archive) |
| Download build logs | `lpcli package build logs --ppa my-username/my-ppa --series noble --state failed -O ./logs` (supports `--build-id`, `--build-url`, `--include-upload-logs`, and all build filters) |
| Dry-run bulk build retries | `lpcli package build retry --ppa my-username/my-ppa --series noble --arch amd64 --state failed --dry-run` (bulk retry requires explicit `--state`) |
| Retry failed builds and back up logs | `lpcli package build retry --ppa my-username/my-ppa --series noble --arch amd64 --state failed --save-logs ./logs --yes` (`--save-logs` saves each log before calling `retry`; omit `--yes` for interactive confirmation) |
| Download source files | `lpcli package download --name curl --series noble` |
| Download specific version | `lpcli package download --name curl --series noble --version 8.5.0-2ubuntu10` |
| Show package changelog | `lpcli package changelog --name unbound --series noble` (shows latest 5 entries; `--limit N`, `--full`, `--version`, `--pocket`, or `--ppa OWNER/NAME`) |

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
| Show default repo | `lpcli git default --target launchpad` (`--target`: project or `ubuntu/+source/pkg`, e.g. `ubuntu/+source/unbound`, not bare distros or `~person`; exits 4 if none configured) |
| List person or team repos | `lpcli git list-person-repos --name my-username` (`--name` accepts team names, with or without a leading `~`) |
| List repos for a target, incl. personal forks | `lpcli git list --target launchpad` (`--target`: project, `ubuntu/+source/pkg`, or `~person`/`~team`) |
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
# Report total_size from the JSON; entries holds at most --limit bugs.
lpcli --format json --fields title bug search --target ubuntu --package rustc-1.97 --limit 1
# To list the bugs themselves:
lpcli --format json --fields title,status,web_link bug search --target ubuntu --package rustc-1.97
```

### Triage a bug

```bash
lpcli --format json bug show --bug-id 123456
lpcli --format json --fields bug_target_name,status,importance,self_link bug tasks --bug-id 123456
lpcli --format json --quiet bug set-status --bug-id 123456 --target curl \
    --series noble --status triaged
lpcli bug set-importance --bug-id 123456 --target curl \
    --series noble --importance high
lpcli bug add-comment --bug-id 123456 --message "Triaged as High for Noble."
```

### Correct a comment in place

Editing needs the comment's index and the comment must be your own. Use JSON
output to get the index rather than counting comments by hand; the index stays
stable when other comments are deleted. Keep multi-line text in a file so shell
quoting cannot lose characters.

```bash
# Per-entry indices, in the same order as messages.entries
lpcli --format json bug comments --bug-id 123456

# Replace comment text in place; the previous text remains in its revisions
lpcli bug edit-comment --bug-id 123456 --comment-index 3 --message-file ./reply.txt
```

The JSON command's `lpcli.comment_indexes` member is the index array; pass the
wanted entry to `--comment-index`.

### Mark a bug as a duplicate

Marking requires `launchpad.Edit` on the bug (its reporter, the target's bug
supervisor, or a driver); otherwise lpcli exits 5. Marking a bug that already
has duplicates re-targets those duplicates to the new master bug.

```bash
# Check the current state first; the JSON includes Launchpad's duplicate links
lpcli --format json --fields duplicate_of_link,number_of_duplicates bug show --bug-id 123456

# Mark 123456 as a duplicate of 654321
lpcli --format json --quiet bug mark-duplicate --bug-id 123456 --duplicate-of 654321

# Verify the result (the link resolves to the canonical master)
lpcli --format json --fields duplicate_of_link bug show --bug-id 123456

# Remove the duplicate marker again
lpcli --format json --quiet bug unmark-duplicate --bug-id 123456
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

# Check whether an upload built, and get the log URL if it failed
lpcli package builds --series noble --name mutter
lpcli package builds --series noble --name mutter --state failed
lpcli package builds --owner my-username --ppa my-ppa --name mutter --version 50.1-0ubuntu2.8 --state failed
lpcli --format json --fields source_package_version,arch_tag,buildstate,datebuilt,duration,build_log_url package builds --series noble --name mutter
```

`package builds` prints one row per source version and architecture with
`Package`, `Version`, `Series`, `Arch`, `Publication` (Published, Superseded,
or Pending), `Build State`, `Binaries`, `Finished`, `Duration`, and `Log`. Rich
output puts the long log URLs in a separate "Build logs" table; `plain`, `tsv`,
and `json` include the full URL. Use `--series` for the primary archive (or as
a PPA filter), `--arch` to select one architecture, and `--state` with
`failed`, `success`, `building`, `needs-build`, or `dependency-wait`.
`--state` takes one state; `failed` means `Failed to build`, so name
`failed-to-upload` or `chroot-problem` explicitly when needed. Sources with no
builds yet show `No builds yet`.

In JSON, `builds.entries[]` holds Launchpad's build entries with `arch_tag`,
`buildstate`, `datebuilt`, `duration`, and `build_log_url`;
`source_publications` holds the matching source publications; and
`lpcli.sources` maps each source publication `self_link` to its `builds` links
and `binaries_published`. For example, a failed PPA build's log URL is at
`.builds.entries[].build_log_url`. `datebuilt` is an RFC 3339 timestamp (null
until the build finishes) and `duration` is Launchpad's `H:MM:SS.ffffff`
string. `lpcli package builds --help` lists every flag.

`--version` matches exactly. The default listing includes Published,
Superseded, and Pending source publications and checks up to `--limit` source
publications (default 30); `--format tsv` prints the same columns. A matching
source with no builds shows a `No builds yet` row; no matching source
publications prints `No source publications found ...`, and an unknown PPA
exits 4. Canonical build states are `Needs building`, `Successfully built`,
`Failed to build`, `Dependency wait`, `Chroot problem`,
`Build for superseded Source`, `Currently building`, `Failed to upload`,
`Uploading build`, `Cancelling build`, `Cancelled build`, and
`Gathering build output`. `--fields` selects Launchpad attributes in
`builds.entries` and `source_publications`; the build version is
`source_package_version`, the series is `distro_series_link`, and the `lpcli`
member is unchanged. The primary-archive listing includes all pockets; there
is no `--pocket` filter.
PPA builds take `--owner OWNER --ppa NAME` (no leading `~`), not
`--ppa OWNER/NAME` or `--archive`.
`--public` reads a public PPA anonymously; your own or a private PPA needs
authentication. `--name`, `--version`, and `--series` filter server-side
before `--limit`; `--arch` and `--state` filter the fetched builds locally
(raise `--limit` if `lpcli.complete` is false). `Binaries` in table rows is
`yes` on succeeded builds whose source publication has active binaries (and `no`
on failed/pending rows), while `lpcli.sources.<link>.binaries_published` is
source-level. Failed builds are data, so the command exits 0; use
`--state failed` to select them. `build_log_url` is present whenever Launchpad
has a log, including successful builds, and may be null while a build is still
running.

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
2. **Log in first** — ask the user to run `lpcli login` in their terminal;
   many commands (bug searches, package lookups, etc.) require
   authentication. `lpcli --format json status` shows whether they are
   logged in.
3. **Use `--help`** — every subcommand supports `--help` for full option details
   (e.g. `lpcli bug search --help`).
4. **Use `--format json`** — it works with every command except `skills`.
   Add `--fields` to keep only the attributes you need, and `--quiet` on
   mutations to get just the resulting `self_link`.
5. **Counting results** — read `total_size` (or count `entries`) from
   `--format json`. Never count output lines with `wc -l`.
6. **Check exit codes** — 3 means log in, 4 not found, 5 permission denied,
   6 retry later, and 7 means the change was only partly applied; read stdout
   before retrying.
7. **Missing command?** lpcli does not wrap every Launchpad operation. Do not
   fall back to `launchpadlib`: it uses a separate credential store
   (`~/.launchpadlib`) and any new authorization needs an interactive browser
   session, which fails in agent runs. Report the gap instead.
8. **Check the binary version** — run `lpcli --version`. If a command described
   here is missing, the installed binary predates this documentation: report the
   version mismatch and ask the user to upgrade or reinstall `lpcli`.
9. **Source package names in Launchpad** may differ from upstream project names
   (e.g. `rustc-1.97` not `rust`). Use the exact source package name the user
   provides.
10. **Self-contained documentation** — `lpcli skills` (alias `lpcli skill`)
   prints this entire document straight from the binary, byte for byte, with
   no authentication or network access; redirect it into a skill directory if
   your agent tooling expects one.
