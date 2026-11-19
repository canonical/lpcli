# Decision 0001: Preserving Launchpad's JSON for `--format json`

- **Date:** 2026-11-16
- **Status:** Accepted (Phase 0 of
  [`improve_user_interface_2026-11-16.md`](completed/improve_user_interface_2026-11-16.md))
- **Applies to:** the library's read functions and the `json` output mode

## Context

D2 makes `--format json` a versioned public contract that keeps Launchpad's
own representation: field names, `null`s, `*_link` fields, and values are
never reformatted, and lists keep Launchpad's collection shape.

The library's typed structs, such as `BugTask` and `BugTracker`, keep only the
fields lpcli models. Serializing them again would drop fields such as
`http_etag` and `resource_type_link`, and could change values; for example,
`chrono` writes `date_created` in a different format from Launchpad's. JSON
output must therefore come from the JSON Launchpad sent.

## Options considered

1. **`Raw<T>` wrapper (chosen).** The library returns the typed value together
   with the unmodified JSON, from new functions alongside the existing ones.
2. **`#[serde(flatten)] extra` map on every type.** Simpler to call, but it
   changes every public library type, still re-encodes the modelled fields
   (so dates could change), and loses Launchpad's key order for those fields.
3. **Return only `serde_json::Value` and parse in the binary.** Keeps fidelity
   but moves typed decoding, which belongs in the library, into the CLI.

## Decision

- `lpcli::client::Raw<T> { value: T, raw: serde_json::Value }` decodes `value`
  from `raw` with `Raw::from_value`, so both always describe the same response.
- `LaunchpadClient::get_url_raw` and `Collection::fetch_page_raw` return raw
  responses. A raw page keeps `total_size` or `total_size_link`, `start`, and
  `next_collection_link` exactly as Launchpad sent them.
- Read functions gain `_raw` counterparts; the existing functions keep their
  signatures, so this is an additive library change. Phase 0 adds
  `bugs::search_bugs_raw` and `bugs::search_bug_trackers_raw`.
- The typed `value` drives the `rich`, `plain`, and `tsv` renderers; `json`
  prints `raw`, or a collection built only from raw entries.
- **Client-side filtering** (`search_bug_trackers_raw`) returns
  `BugTrackerSearch { trackers, complete }`. `complete` is `true` only when
  every page was scanned, so the CLI includes `total_size` only when it is the
  true number of matches, and never includes `next_collection_link`, which
  would point to unfiltered results.
- **Commands without a Launchpad entry**, such as `status`, print an
  lpcli-defined object under a top-level `lpcli` member:

  ```json
  {"lpcli": {
    "authentication": {"credentials": "oauth", "account": "someone", "verified": true},
    "server": {"reachable": true, "http_status": 200,
               "resource_type_link": "https://api.launchpad.net/devel/#service-root",
               "error": null}
  }}
  ```

  `credentials` is `oauth`, `hardware_token`, or `none`. `account` is the
  Launchpad name without `~`. The object never contains credentials, tokens,
  grant secrets, or OAuth headers; `status::AuthStatus` does not carry them.
- **Key order.** `serde_json` is used without its `preserve_order` feature, so
  object keys are written in sorted order. Key order carries no meaning in
  JSON and Launchpad does not document one; enabling the feature would also
  change the exact bytes of JSON that `hardware_tokens` signs and compares.
  Tests compare JSON as values.

## Consequences

- Each newly ported read command needs a `_raw` library function.
- Holding both the typed value and the raw JSON roughly doubles memory for a
  page; pages are bounded by `--limit` or Launchpad's page size.
- Fields come from Launchpad's `devel` API and follow its changes; lpcli does
  not rename or remove them itself.
