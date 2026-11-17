# Changelog

All notable changes to lpcli are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and lpcli follows [Semantic Versioning](https://semver.org/spec/v2.0.0.html) as
applied by [Cargo](https://doc.rust-lang.org/cargo/reference/semver.html). While
lpcli is below 1.0, Cargo treats a change in the minor version (for example,
0.4 to 0.5) as an incompatible release, which may contain breaking changes.

## [Unreleased]

These changes are for **0.5.0**, a SemVer-incompatible release, and follow
[`plans/improve_user_interface_2026-11-16.md`](plans/improve_user_interface_2026-11-16.md).
Entries move to a `0.5.0` section when that release is tagged. Entries marked
*(planned)* are not implemented yet.

### Added

- A global `--format` option with four output modes: `rich` (the default),
  `plain` (accessible labeled records with ASCII punctuation, no color, and
  no box drawing), `json`, and `tsv` (one record per line).
- The `LPCLI_FORMAT` environment variable and the `display.format` and
  `display.accessible` settings in `config.toml`. `display.accessible = true`,
  or `TERM=dumb`, makes `plain` the default.
- A global `--color auto|always|never` option. `auto` follows `CLICOLOR_FORCE`,
  `NO_COLOR`, and `TERM=dumb`, and decides separately for stdout and stderr.
- JSON and TSV output for `status`, `bug search`, and
  `bug remote-bug-trackers`. JSON follows the Launchpad web API's own
  representation, and lists use Launchpad's collection shape: `total_size`,
  `start`, and `entries`. Other commands reject `--format json|tsv` with a
  usage error for now.
- Library: `client::Raw`, `LaunchpadClient::get_url_raw`,
  `Collection::fetch_page_raw`, `bugs::search_bugs_raw`, and
  `bugs::search_bug_trackers_raw`, which keep the exact JSON Launchpad
  returned.
- *(planned)* JSON output for every read command, a versioned and documented
  JSON output contract, and a `lpcli schema` command that reports its
  version.
- *(planned)* Structured JSON errors on stderr in JSON mode.
- *(planned)* `--fields`, `--quiet`, and `--no-input` options for scripts and
  agents.

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
- **Breaking** *(planned)*: commands exit with a code that identifies the kind of failure,
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
  exit code instead, or handle the specific codes above.
- **Breaking** *(planned)*: operations that only partly succeed exit with code 7
  instead of 0. This includes downloads where some files fail, and changes to
  several targets or series where only some succeed.

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
  0.5.0; see "Unreleased".
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

[Unreleased]: https://github.com/canonical/lpcli/compare/v0.4.0...HEAD
[0.4.0]: https://github.com/canonical/lpcli/compare/v0.3.2...v0.4.0
[0.3.2]: https://github.com/canonical/lpcli/compare/v0.3.1...v0.3.2
[0.3.1]: https://github.com/canonical/lpcli/compare/v0.3.0...v0.3.1
[0.3.0]: https://github.com/canonical/lpcli/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/canonical/lpcli/releases/tag/v0.2.0
