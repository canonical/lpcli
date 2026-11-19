# Changelog

All notable changes to lpcli are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and lpcli follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html) as
applied by [Cargo](https://doc.rust-lang.org/cargo/reference/semver.html). While
lpcli is below 1.0, Cargo treats a change in the minor version (for example,
0.4 to 0.5) as an incompatible release, which may contain breaking changes.

## [Unreleased]

### Added

- `git list --target` to enumerate the Git repositories of a project,
  distribution source package, or `~person`/`~team`, including personal
  forks.

### Fixed

- `git list-person-repos` queried `~person/+git`, which is a web-UI page
  that does not exist on the API host, and failed for every name. It now
  uses the root `/+git` collection's `getRepositories` operation with a
  `~owner` target, so person and team listings work.
- Complete snapshot coverage across `schema`, structured errors, remaining `git`,
  `queue`, and domain TSV list outputs, representative `plain` and `rich`
  reports across all command domains, and `bug`/`queue`/`package` partial
  failures, verified by an automated snapshot accessibility and redaction audit.
- Accessibility guarantees and manual Orca/BRLTTY release checklist alongside a
  `0.4.x` migration guide in `README.md`.
- A global `--no-pager` option and automatic `$PAGER` support for `rich` and
  `plain` output when stdout is a non-dumb terminal, `$PAGER` is non-empty and
  not `cat` (and not running under `sudo`), and the rendered output exceeds the
  terminal height (`$LINES`, detected terminal rows, or `24` lines by default).
- A global `--progress auto|plain|rich|none` option and non-redrawing download
  progress reporting on stderr (`plain` mode emits a start line, occasional
  milestones, per-file results, and a summary; `--progress none` disables
  progress output).

### Changed

- Expanded `plain` sanitization (`clean_plain`, `download` progress messages,
  and `assert_plain_invariants`) to also strip bullet (`●`), info (`ℹ`), and
  dingbat/symbol ranges while normalizing `–` and `—` to `-`.
- Documented `--no-pager` and `--progress none` in `SKILL.md`.
- Ported every remaining command (all `bug` subcommands, `queue`, `webhook`,
  `snap`, `access-token`, `hardware-token issue --print`, and `skills`) through
  the output layer (`Output`), removing direct `println!` calls from command
  handlers and replacing `colored` overrides with `anstyle`.
- Applied a consistent four-color ANSI palette in `rich` mode for status,
  importance/priority, tag, and person/team values when color is enabled, and
  normalized Launchpad usernames and timestamps across comment views (`bug
  comments`, `git comments`, `question messages`).
- Accessible `plain` output across all `bug` commands (`show`, `tasks`,
  `comments`, `subscriptions`, `attachments`, `watches`, and mutations), with
  leading result counts, labeled `Field: value` records, explicit `Series:`
  fields instead of indentation, `Attachment:` labels instead of emoji,
  `OK:`/`Warning:`/`Error:` word prefixes instead of symbols, and ASCII
  sanitization for external text in `plain` mode.
- Clearer assistive-technology wording in the security-key selection prompt
  (`Choose a ... (enter a number from 1 to N): `) and grant-secret prompt
  (`Enter grant secret (oauth_token_secret): `).

## [0.5.0] - 2026-11-17

This is a SemVer-incompatible release that implements Phases 0 and 1 of
[`plans/improve_user_interface_2026-11-16.md`](plans/improve_user_interface_2026-11-16.md).
The JSON output contract is documented in
[`docs/json-output.md`](docs/json-output.md).

### Added

- A global `--format` option with four output modes: `rich` (the default),
  `plain` (accessible labeled records with ASCII punctuation, no color, and
  no box drawing), `json`, and `tsv` (one record per line).
- The `LPCLI_FORMAT` environment variable and the `display.format` and
  `display.accessible` settings in `config.toml`. `display.accessible = true`,
  or `TERM=dumb`, makes `plain` the default.
- A global `--color auto|always|never` option. `auto` follows `CLICOLOR_FORCE`,
  `NO_COLOR`, and `TERM=dumb`, and decides separately for stdout and stderr.
- `--format json` for every read and mutation command, and for `status`,
  `schema`, `login`, `logout`, and the `hardware-token` commands. `skills` is
  the only command without JSON output. JSON follows the Launchpad web API's
  own representation: entries are passed through unchanged, and lists use
  Launchpad's collection shape, `total_size`, `start`, and `entries`.
  Commands that combine several requests print an lpcli-defined object whose
  members are unmodified Launchpad entries or collections, with values
  computed by lpcli under an `lpcli` member.
- `--format tsv` for list commands, `status`, and `hardware-token keys`.
- A versioned JSON output contract, documented in `docs/json-output.md`, and
  `lpcli schema`, which reports the contract version (currently 1).
- Structured JSON errors on stderr in JSON mode:
  `{"error": {"kind", "message", "hint", "http_status"}}`. Error output never
  contains credentials, grant secrets, PINs, or assertions.
- `--fields NAME,...`, which keeps only the named Launchpad attributes in each
  JSON entry. It requires JSON output.
- `--quiet`, which makes a mutation print only the `self_link` of each
  resulting entry. It is a usage error on commands that are not mutations.
- `--no-input`, which makes prompts fail instead of waiting for input. It is
  enabled automatically when stdin is not a terminal. Security-key touch
  notices are still written to stderr.
- `hardware-token register` prints the registered key's Launchpad entry in
  JSON, and `hardware-token issue` prints an lpcli-defined summary of the
  issued grant and the resulting login, without any secret.
- Library: `client::Raw`, which pairs a typed value with the exact JSON
  Launchpad returned, and `_raw` counterparts of the read and mutation
  functions in `bugs`, `people`, `packages`, `projects`, `cves`, `git`,
  `specifications`, `questions`, `translations`, `webhooks`, `snaps`,
  `queue`, `access_tokens`, and `hardware_tokens` (for example
  `bugs::get_bug_raw`, `bugs::set_bug_status_raw`, and
  `hardware_tokens::register_hardware_key_raw`). `LaunchpadClient` gains
  `get_raw`, `get_url_raw`, and `get_created_raw`, and `Collection` gains
  `fetch_all_raw` and `fetch_page_raw`.
- Library: `error::ErrorKind`, and `LpError::kind`, `LpError::exit_code`,
  `LpError::http_status`, and `LpError::hint`, which classify errors for exit
  codes and structured errors.
- Library: the `LpError::Usage`, `LpError::Partial`, and
  `LpError::LocalNotFound` variants. Raw mutations that Launchpad has already
  applied, but whose result cannot be confirmed, return `LpError::Partial`
  with recovery guidance. `LocalNotFound` reports a missing match without
  claiming that Launchpad returned HTTP 404.
- Library: `hardware_key::NonInteractiveConsole`, which fails instead of
  prompting but still reports security-key touches on stderr.

### Changed

- **Breaking (library):** date and time fields now use `jiff` instead of the
  unmaintained `chrono` crate. Launchpad timestamps are `jiff::Timestamp`,
  milestone target dates are `jiff::civil::Date`, and
  `IssueGrantRequest::date_expires` is a `jiff::Timestamp`. CLI date output
  and accepted `--expires` formats are unchanged. `lpcli::time` provides
  strict RFC 3339 timestamp parsing that preserves the previous accepted
  grammar.
- `status`, `bug search`, and `bug remote-bug-trackers` count results in
  words, such as `2 bugs found.`, instead of `2 bug(s) found.`.
- Colored output on stdout now also turns off when stdout is not a terminal,
  `NO_COLOR` is set, or `TERM=dumb`, for every command.
- **Breaking:** commands exit with a code that identifies the kind of failure,
  instead of always exiting with 1:

  | Code | Meaning |
  |---:|---|
  | 0 | Success |
  | 1 | General error |
  | 2 | Usage or validation error |
  | 3 | Not authenticated |
  | 4 | Not found |
  | 5 | Permission denied |
  | 6 | Network failure, timeout, or rate limiting |
  | 7 | Partial success |

  Scripts that check for exit code 1 specifically should check for a non-zero
  exit code instead, or handle the specific codes above. The mapping is listed
  in `docs/json-output.md`.
- **Breaking:** operations that only partly succeed exit with code 7 instead
  of 0. This includes downloads where some files fail, bug changes to several
  targets or series where only some succeed, mutations that Launchpad applied
  but whose result could not be read back, and `logout` or
  `hardware-token revoke` that could not remove every local copy. The output
  lists what succeeded and what failed before the error.
- **Breaking:** when stdin is not a terminal, prompting is disabled as if
  `--no-input` were given. Classic `login` and `hardware-token register` then
  fail with exit code 2 before contacting Launchpad, and security-key PIN
  prompts fail instead of reading from stdin. Run them from a terminal, and
  pass a grant secret to `login --hardware-token --grant-token` through
  `LPCLI_HARDWARE_GRANT_SECRET`.
- **Behavior change:** the classic `login` authorization URL and
  instructions are written to stderr instead of stdout, so stdout carries only
  the result.
- **Breaking (library):** `auth::login` writes its prompt to stderr and
  returns `LpError::Usage` when stdin reaches end-of-file; `auth::login` and
  `auth::logout` no longer print a success message, so callers report the
  result themselves.
- **Breaking (library):** some typed functions now return the new error
  variants: `bugs::delete_bug_comment` reports a partially applied deletion as
  `LpError::Partial`, and a zero `bugs::search_bug_trackers` limit is an
  `LpError::Usage`. Successful mutations with a missing, malformed, or
  untrusted `Location` now return `LpError::Partial`. Local misses, such as a
  named bug tracker lookup returning `null`, return `LpError::LocalNotFound`.
  Exhaustive `match` expressions on `LpError` must handle `Usage`, `Partial`,
  and `LocalNotFound`.

### Removed

- **Breaking:** the `--json` option of `bug remote-bug-trackers` has been
  removed. Use `--format json` instead. Passing `--json` now fails with an
  "unexpected argument" usage error.

  The JSON output has also changed from a bare array, sorted by tracker name,
  to Launchpad's collection shape, in the order Launchpad returns the entries:

  | | Before (0.4.x) | After (0.5.0) |
  |---|---|---|
  | Option of `bug remote-bug-trackers` | `--json` | `--format json` |
  | Output | `[{"name": "debbugs", ...}]` | `{"total_size": 1, "start": 0, "entries": [{"name": "debbugs", ...}]}` |
  | Reading names with `jq` | `jq '.[].name'` | `jq '.entries[].name'` |

  Entries now carry every field Launchpad returns, not only the seven fields
  the old output selected. When `--limit` stops a search early, `total_size`
  is omitted because the true number of matches is unknown. To keep sorting
  by name, use `jq '.entries | sort_by(.name)'`.

## [0.4.0] - 2026-11-16

### Added

- `bug link-remote --github "OWNER/REPO#N"`, which links a bug task to a
  GitHub issue or pull request. lpcli finds or registers the repository's bug
  tracker, reuses a matching bug watch, and checks the resulting watch URL
  before it links the task.
- `bug tracker create`, which registers a remote bug tracker.
- `--json` output for `bug remote-bug-trackers`. This option is removed in
  0.5.0; see "0.5.0".
- A "Linked Tasks" column in `bug watches`.
- `bug set-description`, which edits a bug's description from
  `--description` or `--description-file` (`-` reads standard input).
- PPA targeting for the upload-queue commands, using `--ppa OWNER/NAME` or
  `--archive ppa:OWNER/NAME`.
- `--public`, which reads public data only, without loading credentials or
  prompting for a security key, and `--authenticated`, which requires
  credentials.
- A configurable output width, set with `[display] width` in `config.toml`.
- A cache for hardware-bound access tokens shared between invocations, set
  with `[authentication] hardware_token_cache_seconds`, and
  `hardware-token clear-cache`.
- `lpcli skills` (alias `lpcli skill`), which prints the agent skill
  documentation embedded in the binary.

### Changed

- **Breaking:** the minimum supported Rust version is now 1.97.
- **Behavior change:** reads that may include private data use
  authentication by default. Some public distribution reads, such as Ubuntu
  package searches, remain anonymous by default.
- **Behavior change:** `bug link-remote` and `bug unlink-remote` no longer
  choose a task automatically when no `--target`, `--package`, `--series`, or
  `--task-url` is given and the bug has more than one task.
- Upload-queue status values are now accepted in any letter case.
- `Retry-After` headers are honored in both the delay-in-seconds and
  HTTP-date forms.
- Long table cells wrap instead of being truncated. Very narrow output uses a
  stacked `Field: value` layout.
- Cache coordination now locates the user's home directory through a safe
  system lookup.

### Security

- Created-resource `Location` headers are fetched only when they are on the
  configured Launchpad API origin, so credentials are never sent to another
  host.
- Bug task URLs and explicit bug tracker URLs used for remote linking must be
  on the configured API origin.
- Failures after a bug watch or bug tracker was created report what may
  already exist on Launchpad, and how to check it.

## [0.3.2] - 2026-11-09

### Added

- `hardware-token register --classic`, which registers a new security key
  using the existing classic OAuth session.

### Changed

- The prompts shown when registering an additional security key separate
  authentication and registration more clearly.

### Security

- Security-key prompt responses are limited in length.
- Registering a hardware key requires an interactive terminal; piped or
  redirected input is rejected before any prompt.

## [0.3.1] - 2026-10-30

### Changed

- Updated dependencies.

## [0.3.0] - 2026-10-28

### Added

- Hardware-bound API tokens backed by USB FIDO2 security keys, through the
  `hardware-token` commands (`keys`, `register`, `revoke-key`, `issue`,
  `show`, and `revoke`) and `login --hardware-token`.
- `logout --classic` and `logout --hardware-token`, which remove only the
  selected type of credentials.

## [0.2.0] - 2026-10-25

First tagged release.

[Unreleased]: https://github.com/canonical/lpcli/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/canonical/lpcli/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/canonical/lpcli/compare/v0.3.2...v0.4.0
[0.3.2]: https://github.com/canonical/lpcli/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/canonical/lpcli/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/canonical/lpcli/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/canonical/lpcli/releases/tag/v0.2.0
