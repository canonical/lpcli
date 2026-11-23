# lpcli JSON output contract

**Contract version:** 1 (lpcli 0.5.0 and later)

`lpcli schema` reports the contract version and a link to this document:

```json
{"lpcli": {"json_contract_version": 1, "documentation": "https://github.com/canonical/lpcli/blob/main/docs/json-output.md"}}
```

## Stability

JSON output is a versioned public contract. It covers the shapes described
here, the lpcli-defined members of combined objects, the `lpcli` object, the
error object, and the meaning of the exit codes.

Within a contract version, lpcli may add commands, optional members of
lpcli-defined objects, and new error kinds. Renaming or removing an
lpcli-defined key, changing its meaning, or changing the meaning of an exit
code requires a new contract version. Only version 1 exists today.

Launchpad entries are passed through unchanged. lpcli does not rename,
reformat, or remove Launchpad fields itself, but those fields follow changes in
Launchpad's [`devel` web service](https://api.launchpad.net/devel.html).

## Selecting JSON

`--format json` is available for every command except `skills`, whose output
is the embedded `SKILL.md` document itself. JSON is written to stdout only;
prompts, progress, notices, and warnings go to stderr. JSON is indented when
stdout is a terminal and compact otherwise. Keys are sorted, so compare JSON as
values rather than as text.

`LPCLI_FORMAT=json` or `display.format = "json"` in `config.toml` set a
default. A command without JSON output (`skills`) then prints a warning on
stderr and uses its normal output. An explicit `--format json` on such a
command is a usage error (exit code 2).

`--format tsv` is available for list commands, `status`, and
`hardware-token keys`. It prints a header row, then exactly one tab-separated
record per line.

## Launchpad entries and collections

Commands that read one Launchpad object print that object exactly as Launchpad
returned it, including `self_link`, `web_link`, `resource_type_link`,
`http_etag`, `null` values, links, timestamps, and enum display strings.

Lists use Launchpad's collection shape:

```json
{"total_size": 2, "start": 0, "entries": [{"name": "example"}]}
```

- When lpcli has fetched every page, `next_collection_link` is omitted.
- When `--limit` cuts results short, `next_collection_link` is kept.
- When Launchpad gives `total_size_link` instead of `total_size`, lpcli passes
  it through rather than inventing a count.
- When lpcli filters a collection locally (`bug remote-bug-trackers --search`,
  `cve search --keyword`), it includes `total_size` only after checking every
  entry and never includes an unfiltered continuation link.

## lpcli-defined objects

Commands that combine several requests print an lpcli-defined object. Members
containing Launchpad data are unmodified Launchpad entries or collections,
named after Launchpad's link names where possible. Values computed by lpcli
are placed under `lpcli` and are never added to Launchpad entries.

| Command | Top-level members |
|---|---|
| `bug show` | `bug`, `bug_tasks`, `attachments`, `lpcli.attachment_download_urls` |
| `bug comments` | `messages`, `attachments`, `lpcli.attachment_download_urls` |
| `bug attachments` | `attachments`, `lpcli.attachment_download_urls` |
| `bug watches` | `bug_watches`, `bug_tasks`, `lpcli.linked_bug_tasks` |
| `bug link-remote` | `bug_task`, `bug_watch` |
| `git comments` | `all_comments`, `inline_comments`, `lpcli.inline_comment_locations` |
| `queue info` | `package_upload`, `binary_properties`, `logs` (rejected uploads only), `lpcli.archive`, `lpcli.series` |
| `queue download` | `package_upload`, `source_file_urls` or `binary_file_urls`, `lpcli` (archive, series, output directory, per-file `downloads`, `downloaded`, `failed`) |
| `package download` | `source_package_publishing_history`, `lpcli` (`source_file_urls`, output directory, per-file `downloads`, `downloaded`, `failed`) |
| `package changelog` | `source_package_publishing_history`, `lpcli` (`archive`, `series`, `changelog_url`, `changelog`, `total_entries`, `truncated`, `entries`) |
| `package builds` | `source_publications`, `builds`, `lpcli` (`archive`, `series`, `complete`, `sources`) |
| `package build logs` | `builds`, `lpcli` (`archive`, `series`, `complete`, `output_directory`, `downloads`, `downloaded`, `skipped_no_log`, `failed`) |
| `package build retry` | `builds`, `lpcli` (`archive`, `series`, `dry_run`, `cancelled`, `eligible`, `retried`, `skipped`, `saved_logs`, `failures`) |

`package builds` prints `builds.entries` from the `getBuilds` entries of the
listed source publications, in source and architecture order. `--arch`,
`--state`, and `--exclude-state` filter those entries locally, so `builds` omits
`start` and `total_size`. `lpcli.sources` is keyed by the source publication's
`self_link` and records the build `self_link`s listed for that source plus whether
Launchpad reports active binary publications; an empty `builds` list means the
source has no builds yet. `lpcli.complete` is false when `--limit` cut the
source publications short; `source_publications.next_collection_link` then
continues the unfiltered source list.

`package build show` prints Launchpad's `build` entry unchanged.
`package build logs` downloads build logs into `output_directory` without
sending Launchpad credentials and prints matching `builds.entries` alongside
per-file outcomes under `lpcli.downloads`.
`package build retry` prints the retried build entries in `builds.entries` (or
eligible preview entries during `--dry-run` / cancelled runs) alongside
`lpcli.retried`, `lpcli.skipped`, `lpcli.saved_logs`, and `lpcli.failures`.

`status` prints:

```json
{"lpcli": {"authentication": {"credentials": "oauth", "account": "someone", "verified": true}, "server": {"reachable": true}}}
```

Commands without a corresponding Launchpad JSON resource place their data
under `lpcli`.

### Identity commands

Identity output never contains OAuth token keys or secrets, grant secrets,
PINs, or FIDO assertions.

| Command | JSON |
|---|---|
| `login` | `{"lpcli": {"credentials": "oauth", "hardware_token_grant_takes_precedence": false}}` |
| `login --hardware-token` | `{"lpcli": {"credentials": "hardware_token", "account": "...", "grant_storage": "keyring", "path": null}}`; `grant_storage` is `keyring`, `file`, `memory`, or `unchanged` |
| `logout` | `{"lpcli": {"classic_credentials": "removed", "hardware_token_grant": "removed"}}`; each is `removed`, `skipped`, or `failed` |
| `hardware-token keys` | Launchpad's collection of registered security keys |
| `hardware-token register` | The registered key's Launchpad entry, read back after registration |
| `hardware-token issue` | `{"lpcli": {"issued": true, "security_key": {"id", "name", "self_link"}, "grant": {"consumer", "permission", "context", "date_expires"}, "login": {...} or null, "remove_classic_bootstrap_token": true}}`; `login` has the `login --hardware-token` members, and is `null` when logging in with the new grant failed |
| `hardware-token issue --print` | The grant in launchpadlib's format, in every output mode and even with `--quiet`, because the grant is the requested result |
| `hardware-token show` | `{"lpcli": {"hardware_token_grant": {"consumer", "credential_id", "service_root", "storage", "path"}}}`, or `{"lpcli": {"hardware_token_grant": null}}` |
| `hardware-token revoke-key` | `{"lpcli": {"revoked": true, "security_key": {"id", "name", "self_link"}}}` |
| `hardware-token revoke` | `{"lpcli": {"revoked": true, "stored_copy": "removed"}}`; `stored_copy` is `removed`, `failed`, `unchanged`, or `none` |
| `hardware-token clear-cache` | `{"lpcli": {"access_token_cache": "cleared", "hardware_token_grant": "unchanged"}}` |

## Field selection

`--fields name,title,web_link` keeps only the named Launchpad attributes in
each Launchpad entry, that is, each object with a `self_link` or
`resource_type_link`. Collection keys such as `total_size`, `start`, and
`next_collection_link` are retained, and the `lpcli` member is left unchanged.
Selected values are unchanged. Unknown fields are omitted rather than created
as `null`.

Field names must be Launchpad attribute names (ASCII letters, digits, and
underscores). `--fields` requires JSON output; combining it with
`--format rich`, `plain`, or `tsv` is a usage error (exit code 2).

## Mutations and `--quiet`

Mutation commands print the Launchpad entry returned by the operation, or the
entry read back from Launchpad afterwards. Commands whose Launchpad operation
returns no entry print an lpcli-defined result under `lpcli`, for example
`{"lpcli": {"deleted": true, "webhook_link": "..."}}`.

`bug mark-duplicate` and `bug unmark-duplicate` print the updated bug entry:
`duplicate_of_link` is the master bug's API link after marking, and `null`
after unmarking. Bug operations that change several tasks print
`{"bug_tasks": {...}, "lpcli": {"failures": [...]}}` (`bug add-task` may also
include `bug_watches`). `bug delete-task` prints
`{"lpcli": {"bug_id", "deleted_task_links", "failures"}}`. Each failure is:

```json
{"operation": "set_status", "target": "noble", "task_link": "...", "error": {"kind": "permission_denied", "message": "...", "http_status": 403}}
```

`--quiet` is accepted only by mutations; on other commands it is a usage
error (exit code 2). With `--quiet`, a mutation prints only the `self_link` of
each resulting entry, one per line, in any output mode, and prints nothing
when the result has no entry. Two results are printed even with `--quiet`
because they cannot be read again: the secret from `access-token issue` and
`access-token issue-git`, and the grant from `hardware-token issue --print`.

## Errors and exit codes

In JSON mode, runtime errors are written to stderr as one JSON object:

```json
{"error": {"kind": "not_found", "message": "Not found: ...", "hint": null, "http_status": 404}}
```

`hint` is a short recovery suggestion or `null`. `http_status` is the HTTP
status when known, otherwise `null`. Error output never includes credentials,
grant secrets, PINs, assertions, or OAuth headers.

Command-line parsing and validation errors are reported by clap as text on
stderr, with exit code 2, before configuration is loaded, credentials are
read, or any request is made. They are not JSON.

| Code | `error.kind` | Meaning |
|---:|---|---|
| 0 | | Success |
| 1 | `general` | General error, including security-key failures |
| 2 | `usage` | Usage or validation error |
| 3 | `not_authenticated` | Not authenticated, HTTP 401, or a rejected hardware-bound grant |
| 4 | `not_found` | Not found (HTTP 404 or 410) |
| 5 | `permission_denied` | Permission denied (HTTP 403) |
| 6 | `network` | Network failure, timeout, rate limiting, or HTTP 408, 429, or 5xx |
| 7 | `partial` | Partial success |

### Partial success

Exit code 7 means Launchpad or the local system may already reflect part of
the operation. The command prints what succeeded and what failed on stdout
first (in JSON, under `lpcli`), then the error on stderr. Check the output
before retrying. Examples:

- `package download` or `queue download` where some files downloaded and
  others failed. When every file fails, nothing was written and the exit code
  is 1.
- Bug operations on several tasks or series where only some succeed, or a bug
  created by `bug create` whose attachment then fails.
- Mutations that Launchpad applied, but whose result could not be read back.
- `logout` that removed one kind of credentials but not the other,
  `hardware-token revoke` that could not remove the stored copy, and
  `hardware-token issue` where logging in with the new grant failed.

## Non-interactive use

`--no-input` makes prompts fail immediately. It is enabled automatically when
stdin is not a terminal.

- `login` (classic) and `hardware-token register` need interactive input, so
  they fail with exit code 2 before contacting Launchpad.
- `login --hardware-token --grant-token` reads the grant secret from
  `LPCLI_HARDWARE_GRANT_SECRET` instead of prompting.
- Security-key PIN prompts and choosing one of several connected keys fail
  instead of waiting for input.
- Hardware-key touches cannot be skipped; lpcli reports on stderr when it is
  waiting for one.
