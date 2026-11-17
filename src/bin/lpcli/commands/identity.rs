//! Login, logout, and hardware-bound token handlers.
//!
//! `hardware-token keys` prints Launchpad's collection unchanged in `json`
//! mode and one record per key in `tsv`, and `hardware-token register` prints
//! the registered key's entry as Launchpad returned it. The other commands
//! have no Launchpad entry to print, so they print an lpcli-defined object
//! under `lpcli`. No output ever contains credentials, token keys, grant
//! secrets, PINs, or assertions, except `hardware-token issue --print`, whose
//! requested result is the grant itself in launchpadlib's format, printed
//! unchanged in every mode.
//!
//! With `--no-input`, every prompt (the classic login confirmation, the grant
//! secret, the registration key swap, the security-key PIN, and choosing one
//! of several keys) fails before anything is read. Touch notices are still
//! written to stderr, because no software can skip a touch.

use std::ffi::OsString;
use std::sync::atomic::{AtomicBool, Ordering};

use lpcli::error::LpError;
use lpcli::hardware_key::{ConsoleInteraction, Interaction, NonInteractiveConsole};
use lpcli::hardware_tokens::HardwareTokenCredentials;

use crate::*;

/// Environment variable holding the secret of a grant imported with
/// `lpcli login --hardware-token --grant-token`.
pub(crate) const GRANT_SECRET_ENV: &str = "LPCLI_HARDWARE_GRANT_SECRET";

/// The logged-in user's "Authorized applications" page on Launchpad.
pub(crate) const OAUTH_TOKENS_URL: &str = "https://launchpad.net/people/+me/+oauth-tokens";

// ---------------------------------------------------------------------------
// Non-interactive security-key prompting
// ---------------------------------------------------------------------------

/// `--no-input` for security-key ceremonies started by any command, set once
/// by `main`. Commands reach the key through `authenticated_client`, which
/// has no access to the parsed arguments.
static PROMPTS_DISABLED: AtomicBool = AtomicBool::new(false);

/// Record whether prompting is disabled (`--no-input`, or stdin is not a
/// terminal).
pub(crate) fn set_prompts_disabled(disabled: bool) {
    PROMPTS_DISABLED.store(disabled, Ordering::Relaxed);
}

/// Whether prompting is disabled for security-key ceremonies.
pub(crate) fn prompts_disabled() -> bool {
    PROMPTS_DISABLED.load(Ordering::Relaxed)
}

/// How security-key ceremonies interact with the user. Without prompting,
/// PIN requests, key choices and the registration pause fail before reading
/// input; touch notices still go to stderr.
pub(crate) fn security_key_interaction(no_input: bool) -> Arc<dyn Interaction> {
    if no_input {
        Arc::new(NonInteractiveConsole)
    } else {
        Arc::new(ConsoleInteraction)
    }
}

fn no_input_error(action: &str) -> LpError {
    LpError::Usage(format!(
        "{action} needs interactive input, but prompting is disabled (--no-input, or stdin is \
         not a terminal); rerun from a terminal without --no-input."
    ))
}

// ---------------------------------------------------------------------------
// Results
// ---------------------------------------------------------------------------

/// A command's stdout report and its result. A partial failure has both: the
/// report lists what succeeded and what failed, and the error exits with 7.
#[derive(Debug)]
pub(crate) struct IdentityOutcome {
    pub(crate) report: Option<Report>,
    pub(crate) result: lpcli::error::Result<()>,
}

impl IdentityOutcome {
    /// Print the report (unless `quiet`: none of these results is a Launchpad
    /// entry with a `self_link`), then return the result.
    fn finish(self, output: &Output, quiet: bool) -> lpcli::error::Result<()> {
        if let Some(report) = &self.report
            && !quiet
        {
            output.print(report)?;
        }
        self.result
    }
}

/// The state of one local cleanup step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LocalCleanup {
    Removed,
    Skipped,
    Failed,
}

impl LocalCleanup {
    fn of(result: Option<&lpcli::error::Result<()>>) -> Self {
        match result {
            None => Self::Skipped,
            Some(Ok(())) => Self::Removed,
            Some(Err(_)) => Self::Failed,
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Removed => "removed",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
        }
    }
}

fn status(level: Level, text: impl Into<String>) -> Block {
    Block::Status(level, vec![Span::Text(text.into())])
}

fn note(text: impl Into<String>) -> Block {
    Block::Note(vec![Span::Text(text.into())])
}

/// A report whose JSON is an lpcli-defined object.
fn lpcli_report(blocks: Vec<Block>, lpcli: serde_json::Value) -> Report {
    Report {
        blocks,
        ..Report::json(serde_json::json!({ "lpcli": lpcli }))
    }
}

/// Print a mutation's report, or with `--quiet` only the resulting entry's
/// `self_link`, when it has one.
fn print_mutation(
    output: &Output,
    quiet: bool,
    report: &Report,
    self_link: Option<&str>,
) -> lpcli::error::Result<()> {
    if !quiet {
        return output.print(report);
    }
    match self_link {
        Some(link) => output.print_quiet(&serde_json::json!({ "self_link": link })),
        None => Ok(()),
    }
}

fn day(timestamp: Option<jiff::Timestamp>) -> Option<String> {
    timestamp.map(|d| d.strftime("%Y-%m-%d").to_string())
}

// ---------------------------------------------------------------------------
// Login and logout
// ---------------------------------------------------------------------------

/// `lpcli login`: the browser OAuth flow, or with `--hardware-token` a
/// hardware-bound grant.
pub(crate) async fn handle_login(
    args: LoginArgs,
    output: &Output,
    no_input: bool,
    cache_timeout: std::time::Duration,
) -> lpcli::error::Result<()> {
    if args.hardware_token {
        return handle_hardware_login(args, output, no_input, cache_timeout).await;
    }
    // Fail before requesting a token: the flow waits for the user to press
    // Enter after authorizing in the browser.
    if no_input {
        return Err(no_input_error("Classic login"));
    }
    eprintln!("Logging in to Launchpad...");
    auth::login().await?;
    let grant_stored = matches!(GrantStore::from_env().load(), Ok(Some(_)));
    output.print(&classic_login_report(grant_stored))
}

/// The report for a successful classic login. The new token is never shown.
pub(crate) fn classic_login_report(grant_stored: bool) -> Report {
    let mut blocks = vec![status(
        Level::Ok,
        "Logged in to Launchpad and stored the OAuth credentials.",
    )];
    if grant_stored {
        blocks.push(Block::Status(
            Level::Warning,
            vec![
                Span::Text(
                    "A hardware-bound grant is also stored and takes precedence; run ".into(),
                ),
                Span::Command("lpcli logout --hardware-token".into()),
                Span::Text(" to use these credentials instead.".into()),
            ],
        ));
    }
    lpcli_report(
        blocks,
        serde_json::json!({
            "credentials": "oauth",
            "hardware_token_grant_takes_precedence": grant_stored,
        }),
    )
}

/// `lpcli login --hardware-token`: import a grant or use the stored one.
pub(crate) async fn handle_hardware_login(
    args: LoginArgs,
    output: &Output,
    no_input: bool,
    cache_timeout: std::time::Duration,
) -> lpcli::error::Result<()> {
    let mut login = HardwareLogin::new().cache_timeout(cache_timeout);
    if let Some(token) = args.grant_token {
        let account = args.account.as_deref().ok_or_else(|| {
            LpError::HardwareToken("--account is required to import a grant.".to_string())
        })?;
        let secret = read_grant_secret(no_input)?;
        login = login.grant(HardwareTokenGrant::new(token, secret.as_str(), account)?);
    }
    if let Some(account) = args.account {
        login = login.account(account);
    }
    if let Some(consumer) = args.consumer {
        login = login.consumer(consumer);
    }
    if let Some(credential_id) = args.credential_id {
        login = login.credential_id(credential_id);
    }

    eprintln!("Logging in to Launchpad with a hardware-bound grant...");
    let outcome = hardware_tokens::login_with_hardware_token(login, usb_authenticator()).await?;
    output.print(&hardware_login_report(
        &outcome.account,
        outcome.stored_in.as_ref(),
    ))
}

/// Read the secret of a grant being imported, without echoing it.
pub(crate) fn read_grant_secret(
    no_input: bool,
) -> lpcli::error::Result<zeroize::Zeroizing<String>> {
    grant_secret_from(std::env::var_os(GRANT_SECRET_ENV), no_input, || {
        rpassword::prompt_password("Grant secret (oauth_token_secret): ")
    })
}

/// Use the secret from [`GRANT_SECRET_ENV`] when it is set, and otherwise
/// `prompt` for it, unless prompting is disabled.
pub(crate) fn grant_secret_from(
    env: Option<OsString>,
    no_input: bool,
    prompt: impl FnOnce() -> std::io::Result<String>,
) -> lpcli::error::Result<zeroize::Zeroizing<String>> {
    if let Some(secret) = env {
        return secret
            .into_string()
            .map(zeroize::Zeroizing::new)
            .map_err(|_| {
                LpError::HardwareToken(format!("{GRANT_SECRET_ENV} is not valid UTF-8."))
            });
    }
    if no_input {
        return Err(LpError::Usage(format!(
            "Importing a grant needs its secret, but prompting is disabled (--no-input, or stdin \
             is not a terminal); set {GRANT_SECRET_ENV} instead."
        )));
    }
    Ok(zeroize::Zeroizing::new(prompt()?))
}

/// The report for a successful hardware-bound login. It names the account,
/// as `status` does, but never the grant's token key or secret.
pub(crate) fn hardware_login_report(account: &str, stored_in: Option<&StoredIn>) -> Report {
    let account = account.trim_start_matches('~');
    let mut blocks = vec![Block::Status(
        Level::Ok,
        vec![
            Span::Text("Logged in as ".into()),
            Span::Name(format!("~{account}")),
            Span::Text(" with a hardware-bound grant.".into()),
        ],
    )];
    let (storage, path) = match stored_in {
        Some(StoredIn::Memory) => {
            blocks.push(status(
                Level::Warning,
                format!(
                    "Running under sudo, so the grant was kept in memory only. Set {} to store \
                     it in a file.",
                    hardware_tokens::CREDENTIALS_FILE_ENV
                ),
            ));
            ("memory", None)
        }
        Some(location) => {
            blocks.push(note(format!("Grant stored in {location}.")));
            match location {
                StoredIn::File(path) => ("file", Some(path.display().to_string())),
                _ => ("keyring", None),
            }
        }
        None => {
            blocks.push(note("Using the stored grant."));
            ("unchanged", None)
        }
    };
    lpcli_report(
        blocks,
        serde_json::json!({
            "credentials": "hardware_token",
            "account": account,
            "grant_storage": storage,
            "path": path,
        }),
    )
}

/// `lpcli logout`: remove the locally stored classic credentials and/or
/// hardware-bound grant. Nothing is revoked on Launchpad.
pub(crate) fn handle_logout(args: LogoutArgs, output: &Output) -> lpcli::error::Result<()> {
    logout_outcome(&args, auth::logout, || GrantStore::from_env().delete()).finish(output, false)
}

/// Run the requested cleanup steps, attempting each one even when another
/// fails. When some steps succeed and others fail, the error is
/// [`LpError::Partial`].
pub(crate) fn logout_outcome(
    args: &LogoutArgs,
    delete_classic: impl FnOnce() -> lpcli::error::Result<()>,
    delete_grant: impl FnOnce() -> lpcli::error::Result<()>,
) -> IdentityOutcome {
    let classic = (!args.hardware_token).then(delete_classic);
    let grant = (!args.classic).then(delete_grant);
    let classic_state = LocalCleanup::of(classic.as_ref());
    let grant_state = LocalCleanup::of(grant.as_ref());

    let mut blocks = Vec::new();
    match classic_state {
        LocalCleanup::Removed => blocks.push(status(
            Level::Ok,
            "Removed any stored classic OAuth credentials.",
        )),
        LocalCleanup::Failed => blocks.push(status(
            Level::Error,
            "Could not remove the stored classic OAuth credentials.",
        )),
        LocalCleanup::Skipped => {}
    }
    match grant_state {
        LocalCleanup::Removed => blocks.push(status(
            Level::Ok,
            "Removed any stored hardware-bound grant.",
        )),
        LocalCleanup::Failed => blocks.push(status(
            Level::Error,
            "Could not remove the stored hardware-bound grant.",
        )),
        LocalCleanup::Skipped => {}
    }
    let report = lpcli_report(
        blocks,
        serde_json::json!({
            "classic_credentials": classic_state.key(),
            "hardware_token_grant": grant_state.key(),
        }),
    );

    let mut removed = Vec::new();
    let mut failures = Vec::new();
    for (what, result) in [
        ("the classic OAuth credentials", classic),
        ("the hardware-bound grant", grant),
    ] {
        match result {
            Some(Ok(())) => removed.push(what),
            Some(Err(error)) => failures.push((what, error)),
            None => {}
        }
    }
    let describe = |failures: &[(&str, LpError)]| {
        failures
            .iter()
            .map(|(what, error)| format!("could not remove {what}: {error}"))
            .collect::<Vec<_>>()
            .join("; ")
    };
    let result = if failures.is_empty() {
        Ok(())
    } else if !removed.is_empty() {
        Err(LpError::Partial(format!(
            "Logout was incomplete: removed {}, but {}.",
            removed.join(" and "),
            describe(&failures)
        )))
    } else if failures.len() == 1 {
        Err(failures.remove(0).1)
    } else {
        Err(LpError::Other(format!(
            "Logout failed: {}.",
            describe(&failures)
        )))
    };
    IdentityOutcome {
        // Nothing to report when nothing changed: the error says why.
        report: (!removed.is_empty()).then_some(report),
        result,
    }
}

// ---------------------------------------------------------------------------
// Hardware-bound token handlers
// ---------------------------------------------------------------------------

/// Clear the local access-token cache. `run_command` prints
/// [`clear_cache_report`] afterwards.
pub(crate) fn handle_clear_cache() -> lpcli::error::Result<()> {
    hardware_tokens::clear_cached_access_token()
}

pub(crate) fn clear_cache_report() -> Report {
    lpcli_report(
        vec![
            status(Level::Ok, "Cleared the local hardware access-token cache."),
            note("The stored grant is unchanged; nothing was revoked on Launchpad."),
        ],
        serde_json::json!({
            "access_token_cache": "cleared",
            "hardware_token_grant": "unchanged",
        }),
    )
}

/// Find an active registered security key by its ID or name.
pub(crate) async fn find_registered_key(
    client: &LaunchpadClient,
    key: &str,
) -> lpcli::error::Result<RegisteredAuthenticator> {
    let mut matches: Vec<RegisteredAuthenticator> =
        hardware_tokens::list_registered_authenticators(client)
            .await?
            .into_iter()
            .filter(|a| a.is_active())
            .filter(|a| {
                a.id.is_some_and(|id| id.to_string() == key) || a.name.as_deref() == Some(key)
            })
            .collect();
    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 => Err(LpError::LocalNotFound(format!(
            "No active security key with ID or name '{key}'; see `lpcli hardware-token keys`."
        ))),
        _ => Err(LpError::Other(format!(
            "Several security keys are named '{key}'; use the key's ID instead."
        ))),
    }
}

/// The authenticated account's registered security keys, with Launchpad's
/// collection JSON. This is the same request as
/// [`hardware_tokens::list_registered_authenticators`].
pub(crate) async fn registered_keys_raw(
    client: &LaunchpadClient,
) -> lpcli::error::Result<Raw<Vec<RegisteredAuthenticator>>> {
    #[derive(serde::Deserialize)]
    struct ServiceRoot {
        me_link: Option<String>,
    }
    #[derive(serde::Deserialize)]
    struct Me {
        self_link: String,
    }

    let root: ServiceRoot = client.get("").await?;
    let me_link = root.me_link.ok_or(LpError::NotAuthenticated)?;
    let me: Me = client.get_url(&me_link).await?;
    Collection::fetch_all_raw(
        client,
        &format!("{}?ws.op=getRegisteredAuthenticators", me.self_link),
    )
    .await
}

/// The `hardware-token keys` report: Launchpad's collection in JSON.
pub(crate) fn hardware_keys_report(keys: &Raw<Vec<RegisteredAuthenticator>>) -> Report {
    let rows = keys
        .value
        .iter()
        .map(|key| {
            vec![
                key.id.map(|id| id.to_string()),
                key.name.clone(),
                day(key.date_created),
                day(key.date_last_used),
                Some(if key.is_active() { "active" } else { "revoked" }.to_string()),
            ]
        })
        .collect();
    let column = |label, key| Column { label, key };
    Report {
        blocks: vec![Block::List {
            noun: Noun {
                singular: "security key",
                plural: "security keys",
            },
            empty: "No security keys are registered. Register one with \
                    `lpcli hardware-token register <NAME>`."
                .into(),
            hint: None,
        }],
        json: keys.raw.clone(),
        records: Records {
            columns: vec![
                column("ID", "id"),
                column("Name", "name"),
                column("Registered", "date_created"),
                column("Last Used", "date_last_used"),
                column("Status", "status"),
            ],
            rows,
        },
    }
}

/// The `hardware-token show` report. Only configuration is shown: the
/// account name and token key identify the account or form part of the
/// credential, so they are never written to output, which may be captured in
/// logs. The secret is never shown either.
pub(crate) fn grant_show_report(
    credentials: Option<&HardwareTokenCredentials>,
    store: &GrantStore,
) -> Report {
    let Some(credentials) = credentials else {
        return lpcli_report(
            vec![note("No hardware-bound grant is stored.")],
            serde_json::json!({ "hardware_token_grant": null }),
        );
    };
    let credential_id = credentials.grant().credential_id();
    let (storage, path, stored_in) = match store {
        GrantStore::File(path) => (
            "file",
            Some(path.display().to_string()),
            path.display().to_string(),
        ),
        GrantStore::Memory => ("memory", None, StoredIn::Memory.to_string()),
        GrantStore::Keyring => (
            "keyring",
            None,
            "the system keyring (or ~/.config/lpcli/hardware-token.json)".to_string(),
        ),
    };
    lpcli_report(
        vec![
            Block::Heading("Hardware-bound grant".into()),
            Block::Field("Consumer", credentials.consumer().to_string()),
            Block::Field(
                "Credential ID",
                credential_id.unwrap_or("any registered key").to_string(),
            ),
            Block::Field("Service root", credentials.service_root().to_string()),
            Block::Field("Stored in", stored_in),
        ],
        serde_json::json!({
            "hardware_token_grant": {
                "consumer": credentials.consumer(),
                "credential_id": credential_id,
                "service_root": credentials.service_root(),
                "storage": storage,
                "path": path,
            }
        }),
    )
}

pub(crate) fn key_label(key: &RegisteredAuthenticator) -> String {
    let name = key.name.as_deref().unwrap_or("unnamed");
    match key.id {
        Some(id) => format!("'{name}' (ID {id})"),
        None => format!("'{name}'"),
    }
}

/// A security key's identifying fields, for lpcli-defined objects.
fn security_key_json(key: &RegisteredAuthenticator) -> serde_json::Value {
    serde_json::json!({
        "id": key.id,
        "name": key.name,
        "self_link": key.self_link,
    })
}

/// The `hardware-token revoke-key` report. Launchpad's `revoke` operation
/// returns no entry, so the JSON is lpcli-defined.
pub(crate) fn revoke_key_report(key: &RegisteredAuthenticator) -> Report {
    lpcli_report(
        vec![status(
            Level::Ok,
            format!("Revoked security key {}.", key_label(key)),
        )],
        serde_json::json!({
            "revoked": true,
            "security_key": security_key_json(key),
        }),
    )
}

/// The `hardware-token revoke` outcome after Launchpad revoked the grant.
///
/// `stored` says whether any grant is stored locally, and `deleted` is the
/// result of deleting it, when it was the revoked grant. Failing to delete
/// it is a partial success: Launchpad has already revoked the grant.
pub(crate) fn grant_revoke_outcome(
    stored: bool,
    deleted: Option<lpcli::error::Result<()>>,
) -> IdentityOutcome {
    let mut blocks = vec![status(
        Level::Ok,
        "Revoked the hardware-bound grant on Launchpad.",
    )];
    let (stored_copy, result) = match deleted {
        Some(Ok(())) => {
            blocks.push(note("Removed the stored copy."));
            ("removed", Ok(()))
        }
        Some(Err(error)) => {
            blocks.push(status(Level::Error, "Could not remove the stored copy."));
            (
                "failed",
                Err(LpError::Partial(format!(
                    "Revoked the hardware-bound grant on Launchpad, but could not remove the \
                     stored copy: {error}. Run `lpcli logout --hardware-token` to remove it."
                ))),
            )
        }
        None if stored => {
            blocks.push(note("The stored grant is a different one and was kept."));
            ("unchanged", Ok(()))
        }
        None => ("none", Ok(())),
    };
    IdentityOutcome {
        report: Some(lpcli_report(
            blocks,
            serde_json::json!({ "revoked": true, "stored_copy": stored_copy }),
        )),
        result,
    }
}

/// The `hardware-token register` report: the registered key's entry, read
/// back from Launchpad, in JSON.
pub(crate) fn register_report(key: &Raw<RegisteredAuthenticator>, name: &str) -> Report {
    let key_ref = key
        .value
        .id
        .map(|id| id.to_string())
        .unwrap_or_else(|| name.to_string());
    Report {
        blocks: vec![
            status(
                Level::Ok,
                format!("Registered security key {}.", key_label(&key.value)),
            ),
            Block::Note(vec![
                Span::Text("Issue a grant bound to it with ".into()),
                Span::Command(format!("lpcli hardware-token issue --key {key_ref}")),
                Span::Text(".".into()),
            ]),
        ],
        ..Report::json(key.raw.clone())
    }
}

/// The `self_link` of a Launchpad entry, for `--quiet`.
pub(crate) fn raw_self_link(entry: &serde_json::Value) -> Option<&str> {
    entry.get("self_link").and_then(serde_json::Value::as_str)
}

/// The `hardware-token issue` outcome, without `--print`, after Launchpad
/// issued a grant for `request` bound to `key`.
///
/// `login` is the account and storage reported by logging in with the new
/// grant, or the error when that failed. Launchpad has already issued the
/// grant by then, so a failed login is [`LpError::Partial`]. The JSON is
/// lpcli-defined: Launchpad returns only the grant's token key and secret,
/// which are never printed. `bootstrap` says whether a classic token issued
/// the grant and should now be removed.
pub(crate) fn issue_outcome(
    key: &RegisteredAuthenticator,
    request: &IssueGrantRequest,
    login: std::result::Result<(&str, Option<&StoredIn>), &LpError>,
    bootstrap: bool,
) -> IdentityOutcome {
    let grant = serde_json::json!({
        "consumer": request.consumer_key,
        "permission": request.permission.title(),
        "context": request.context,
        // `null` when Launchpad's default expiry was used.
        "date_expires": request.date_expires.map(|date| date.to_string()),
    });
    let (blocks, login_json, bootstrap, result) = match login {
        Ok((account, stored_in)) => {
            let mut report = hardware_login_report(account, stored_in);
            if bootstrap {
                report.blocks.extend(bootstrap_cleanup_blocks());
            }
            (
                report.blocks,
                report.json["lpcli"].take(),
                bootstrap,
                Ok(()),
            )
        }
        Err(error) => (
            vec![
                status(
                    Level::Error,
                    format!(
                        "Launchpad issued a hardware-bound grant bound to {}, but logging in \
                         with it failed.",
                        key_label(key)
                    ),
                ),
                note(format!("Revoke the unused grant at {OAUTH_TOKENS_URL}.")),
            ],
            serde_json::Value::Null,
            // Keep the classic token: it is still the only working login.
            false,
            Err(LpError::Partial(format!(
                "Launchpad issued a hardware-bound grant bound to {}, but logging in with it \
                 failed: {error}. Revoke the unused grant at {OAUTH_TOKENS_URL}.",
                key_label(key)
            ))),
        ),
    };
    IdentityOutcome {
        report: Some(lpcli_report(
            blocks,
            serde_json::json!({
                "issued": true,
                "security_key": security_key_json(key),
                "grant": grant,
                "login": login_json,
                "remove_classic_bootstrap_token": bootstrap,
            }),
        )),
        result,
    }
}

/// The steps that remove the classic bootstrap token after `issue`.
fn bootstrap_cleanup_blocks() -> Vec<Block> {
    vec![
        Block::Break,
        note("Now remove the classic bootstrap token, which does not need your key:"),
        // `+me` resolves to the logged-in user, so the account name is not
        // needed in (or leaked through) this output.
        note(format!(
            "1. Revoke the \"{}\" authorization at {OAUTH_TOKENS_URL}",
            auth::CONSUMER_KEY
        )),
        Block::Note(vec![
            Span::Text("2. Run ".into()),
            Span::Command("lpcli logout --classic".into()),
            Span::Text(" to delete the local copy.".into()),
        ]),
    ]
}

pub(crate) async fn handle_hardware_token(
    cmd: HardwareTokenCommand,
    output: &Output,
    quiet: bool,
    no_input: bool,
    cache_timeout: std::time::Duration,
) -> lpcli::error::Result<()> {
    match cmd {
        HardwareTokenCommand::ClearCache => {
            handle_clear_cache()?;
            output.print(&clear_cache_report())
        }

        HardwareTokenCommand::Show => {
            let store = GrantStore::from_env();
            let credentials = store.load()?;
            output.print(&grant_show_report(credentials.as_ref(), &store))
        }

        HardwareTokenCommand::Keys => {
            let client = authenticated_client(cache_timeout)?;
            let keys = registered_keys_raw(&client).await?;
            output.print(&hardware_keys_report(&keys))
        }

        HardwareTokenCommand::Register { name, classic } => {
            // Registration pauses so the user can connect the new key; fail
            // before contacting Launchpad rather than at that pause.
            if no_input {
                return Err(no_input_error("Security-key registration"));
            }
            let client = if classic {
                classic_authenticated_client()?
            } else {
                authenticated_client(cache_timeout)?
            };
            eprintln!("Registering a security key with Launchpad...");
            if client.is_hardware_bound() {
                eprintln!("First authenticate with the key bound to your stored grant.");
                eprintln!(
                    "To use an existing OAuth session instead, rerun registration with --classic."
                );
            } else {
                eprintln!("Authenticating API requests with the stored classic OAuth session.");
            }
            let key =
                hardware_tokens::register_hardware_key_raw(&client, &name, usb_authenticator())
                    .await?;
            print_mutation(
                output,
                quiet,
                &register_report(&key, &name),
                raw_self_link(&key.raw),
            )
        }

        HardwareTokenCommand::RevokeKey { key } => {
            let client = authenticated_client(cache_timeout)?;
            let registered = find_registered_key(&client, &key).await?;
            let link = registered
                .self_link
                .as_deref()
                .ok_or_else(|| LpError::Other("The security key has no self_link.".to_string()))?;
            hardware_tokens::revoke_registered_authenticator(&client, link).await?;
            print_mutation(output, quiet, &revoke_key_report(&registered), None)
        }

        HardwareTokenCommand::Issue {
            key,
            permission,
            context,
            expires,
            consumer,
            print,
        } => {
            let login = HardwareLogin::new().cache_timeout(cache_timeout);
            let client = authenticated_client(cache_timeout)?;
            let registered = find_registered_key(&client, &key).await?;
            let request = IssueGrantRequest {
                consumer_key: consumer.clone(),
                authenticator_link: registered.self_link.clone().ok_or_else(|| {
                    LpError::Other("The security key has no self_link.".to_string())
                })?,
                permission: permission.into(),
                context,
                date_expires: expires,
            };
            let grant =
                hardware_tokens::issue_hardware_bound_oauth_token(&client, &request).await?;

            if print {
                // The grant is the result, so it is printed even with --quiet,
                // and in launchpadlib's format whatever the output mode.
                println!("{}", grant.to_issuance_json().as_str());
                eprintln!("Warning: this is the grant secret; it will not be shown again.");
                return Ok(());
            }

            eprintln!(
                "Issued a hardware-bound grant bound to {}. Logging in with it...",
                key_label(&registered)
            );
            let login = login.grant(grant).consumer(consumer);
            // A failed login is partial: Launchpad has already issued the
            // grant, and its secret is lost with this process.
            let outcome =
                hardware_tokens::login_with_hardware_token(login, usb_authenticator()).await;
            let login = match &outcome {
                Ok(outcome) => Ok((outcome.account.as_str(), outcome.stored_in.as_ref())),
                Err(error) => Err(error),
            };
            // The result has no `self_link`, so --quiet prints nothing.
            issue_outcome(&registered, &request, login, !client.is_hardware_bound())
                .finish(output, quiet)
        }

        HardwareTokenCommand::Revoke { token } => {
            let store = GrantStore::from_env();
            let stored = store.load()?;
            let token_key = match (token, &stored) {
                (Some(token), _) => token,
                (None, Some(credentials)) => credentials.grant().token().to_string(),
                (None, None) => {
                    return Err(LpError::HardwareToken(
                        "No hardware-bound grant is stored; pass --token to revoke another grant."
                            .to_string(),
                    ));
                }
            };

            let client = authenticated_client(cache_timeout)?;
            hardware_tokens::revoke_hardware_bound_oauth_token(&client, &token_key).await?;
            let deleted = stored
                .as_ref()
                .is_some_and(|credentials| credentials.grant().token() == token_key)
                .then(|| store.delete());
            grant_revoke_outcome(stored.is_some(), deleted).finish(output, quiet)
        }
    }
}

pub(crate) fn identity_supports_json(command: &Command) -> bool {
    matches!(
        command,
        Command::Login(_)
            | Command::Logout(_)
            | Command::HardwareToken(
                HardwareTokenCommand::Keys
                    | HardwareTokenCommand::Show
                    | HardwareTokenCommand::ClearCache
                    | HardwareTokenCommand::Register { .. }
                    | HardwareTokenCommand::RevokeKey { .. }
                    | HardwareTokenCommand::Issue { .. }
                    | HardwareTokenCommand::Revoke { .. }
            )
    )
}

pub(crate) fn identity_supports_tsv(command: &Command) -> bool {
    matches!(command, Command::HardwareToken(HardwareTokenCommand::Keys))
}
