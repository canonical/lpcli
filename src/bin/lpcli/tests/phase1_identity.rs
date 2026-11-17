//! Phase 1 CLI tests for identity commands.

use super::*;
use crate::output::tests::{assert_plain_invariants, assert_tsv_invariants};
use lpcli::error::LpError;

const MODES: [OutputMode; 4] = [
    OutputMode::Rich,
    OutputMode::Plain,
    OutputMode::Json,
    OutputMode::Tsv,
];

/// Grant values that must never reach stdout.
const SECRETS: [&str; 2] = ["grant-token-key", "grant-secret-value"];

/// The grant's account, which `hardware-token show` also never prints.
const GRANT_ACCOUNT: &str = "grant-account";

fn parse(args: &[&str]) -> Cli {
    Cli::try_parse_from(args).unwrap()
}

fn render(report: &Report, mode: OutputMode) -> String {
    Output::fixed(mode, false, 100).render(report)
}

/// Snapshot `report` in each mode the command supports, checking the
/// per-mode invariants and that no secret appears.
fn assert_modes(name: &str, report: &Report, modes: &[OutputMode], records: usize) {
    for &mode in modes {
        let rendered = render(report, mode);
        match mode {
            OutputMode::Plain => assert_plain_invariants(&rendered),
            OutputMode::Tsv => assert_tsv_invariants(&rendered, records),
            OutputMode::Json => {
                let value: serde_json::Value = serde_json::from_str(&rendered).unwrap();
                assert_eq!(value, report.json);
            }
            OutputMode::Rich => assert!(!rendered.contains('\u{1b}')),
        }
        for secret in SECRETS {
            assert!(!rendered.contains(secret), "{name} {mode:?} leaks {secret}");
        }
        insta::assert_snapshot!(format!("identity_{name}_{}", mode.name()), rendered);
    }
}

fn stored_grant(credential_id: Option<&str>) -> lpcli::hardware_tokens::HardwareTokenCredentials {
    let mut grant =
        HardwareTokenGrant::new("grant-token-key", "grant-secret-value", GRANT_ACCOUNT).unwrap();
    if let Some(id) = credential_id {
        grant = grant.with_credential_id(id).unwrap();
    }
    lpcli::hardware_tokens::HardwareTokenCredentials::new(
        grant,
        hardware_tokens::HARDWARE_CONSUMER_KEY,
    )
    .unwrap()
}

fn registered_key(id: u64, name: &str, revoked: bool) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": name,
        "attestation_format": "packed",
        "transports": ["usb"],
        "date_created": "2026-01-02T03:04:05.123456+00:00",
        "date_last_used": if revoked { serde_json::Value::Null } else { "2026-02-03T04:05:06+00:00".into() },
        "date_revoked": if revoked { "2026-03-01T00:00:00+00:00".into() } else { serde_json::Value::Null },
        "is_revoked": revoked,
        "self_link": format!("https://api.launchpad.net/devel/~example-owner/+authenticator/{id}"),
        "resource_type_link": "https://api.launchpad.net/devel/#webauthn_authenticator",
        "http_etag": format!("\"etag-{id}\""),
    })
}

// -- Output-mode support ------------------------------------------------------

#[test]
fn identity_json_and_tsv_support() {
    for (args, json, tsv) in [
        (&["lpcli", "hardware-token", "keys"][..], true, true),
        (&["lpcli", "hardware-token", "show"], true, false),
        (&["lpcli", "hardware-token", "clear-cache"], true, false),
        (&["lpcli", "hardware-token", "revoke-key", "1"], true, false),
        (&["lpcli", "hardware-token", "revoke"], true, false),
        (&["lpcli", "logout"], true, false),
        (&["lpcli", "login", "--hardware-token"], true, false),
        (&["lpcli", "login"], true, false),
        (&["lpcli", "hardware-token", "register", "Key"], true, false),
        (
            &["lpcli", "hardware-token", "register", "Key", "--classic"],
            true,
            false,
        ),
        (
            &["lpcli", "hardware-token", "issue", "--key", "1"],
            true,
            false,
        ),
        // `--print` prints the grant as launchpadlib's JSON in every mode.
        (
            &["lpcli", "hardware-token", "issue", "--key", "1", "--print"],
            true,
            false,
        ),
    ] {
        let cli = parse(args);
        assert_eq!(identity_supports_json(&cli.command), json, "{args:?}");
        assert_eq!(identity_supports_tsv(&cli.command), tsv, "{args:?}");
        assert_eq!(supports_json_output(&cli.command), json, "{args:?}");
        assert_eq!(supports_tsv_output(&cli.command), tsv, "{args:?}");
    }
    // An explicit structured format for an unported command is a usage error.
    let cli = parse(&["lpcli", "--format", "tsv", "hardware-token", "show"]);
    assert_eq!(validate_cli(&cli).unwrap_err().exit_code(), 2);
    let cli = parse(&["lpcli", "--format", "json", "hardware-token", "clear-cache"]);
    validate_cli(&cli).unwrap();
    for args in [
        &[
            "lpcli",
            "--format",
            "json",
            "hardware-token",
            "issue",
            "--key",
            "1",
        ][..],
        &[
            "lpcli",
            "--format",
            "json",
            "hardware-token",
            "issue",
            "--key",
            "1",
            "--print",
        ],
        &[
            "lpcli",
            "--format",
            "json",
            "hardware-token",
            "register",
            "Key",
        ],
    ] {
        validate_cli(&parse(args)).unwrap();
    }
    let cli = parse(&[
        "lpcli",
        "--format",
        "tsv",
        "hardware-token",
        "issue",
        "--key",
        "1",
    ]);
    assert_eq!(validate_cli(&cli).unwrap_err().exit_code(), 2);
}

// -- Reports --------------------------------------------------------------------

#[test]
fn hardware_keys_report_passes_launchpad_collection_through() {
    let raw = serde_json::json!({
        "total_size": 2,
        "start": 0,
        "entries": [registered_key(7, "Primary\tkey", false), registered_key(8, "Old", true)],
    });
    let keys: Raw<Vec<RegisteredAuthenticator>> = Raw {
        value: serde_json::from_value(raw["entries"].clone()).unwrap(),
        raw: raw.clone(),
    };
    let report = hardware_keys_report(&keys);
    assert_eq!(
        report.json, raw,
        "JSON must be Launchpad's collection unchanged"
    );
    assert_modes("keys", &report, &MODES, 2);

    let empty = Raw {
        value: Vec::new(),
        raw: serde_json::json!({"total_size": 0, "start": 0, "entries": []}),
    };
    assert_modes("keys_empty", &hardware_keys_report(&empty), &MODES, 0);
}

#[tokio::test]
async fn hardware_keys_raw_fetches_every_page_unchanged() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let me = format!("{base}/~example-owner");
    let root = server
        .mock("GET", "/")
        .with_body(serde_json::json!({"me_link": me}).to_string())
        .create_async()
        .await;
    let person = server
        .mock("GET", "/~example-owner")
        .with_body(serde_json::json!({"name": "example-owner", "self_link": me}).to_string())
        .create_async()
        .await;
    let first = registered_key(7, "Primary", false);
    let second = registered_key(8, "Old", true);
    let page1 = server
        .mock("GET", "/~example-owner")
        .match_query(mockito::Matcher::UrlEncoded(
            "ws.op".into(),
            "getRegisteredAuthenticators".into(),
        ))
        .with_body(
            serde_json::json!({
                "total_size": 2,
                "start": 0,
                "entries": [first],
                "next_collection_link": format!("{base}/page2"),
            })
            .to_string(),
        )
        .create_async()
        .await;
    let page2 = server
        .mock("GET", "/page2")
        .with_body(serde_json::json!({"start": 1, "entries": [second]}).to_string())
        .create_async()
        .await;

    let keys = registered_keys_raw(&mock_read_client(&server, true))
        .await
        .unwrap();
    assert_eq!(
        keys.raw,
        serde_json::json!({"total_size": 2, "start": 0, "entries": [first, second]})
    );
    assert_eq!(keys.value.len(), 2);
    assert!(keys.value[0].is_active() && !keys.value[1].is_active());
    for mock in [root, person, page1, page2] {
        mock.assert_async().await;
    }
}

#[test]
fn grant_show_never_includes_account_token_or_secret() {
    let store = GrantStore::File("/home/example/.config/lpcli/grant.json".into());
    let credentials = stored_grant(Some("AAEC"));
    let report = grant_show_report(Some(&credentials), &store);
    assert_eq!(
        report.json,
        serde_json::json!({"lpcli": {"hardware_token_grant": {
            "consumer": "lpcli-hardware",
            "credential_id": "AAEC",
            "service_root": "https://api.launchpad.net/",
            "storage": "file",
            "path": "/home/example/.config/lpcli/grant.json",
        }}})
    );
    let modes = [OutputMode::Rich, OutputMode::Plain, OutputMode::Json];
    assert_modes("show", &report, &modes, 0);
    for mode in modes {
        assert!(!render(&report, mode).contains(GRANT_ACCOUNT), "{mode:?}");
    }

    let keyring = grant_show_report(Some(&stored_grant(None)), &GrantStore::Keyring);
    assert_eq!(
        keyring.json["lpcli"]["hardware_token_grant"]["storage"],
        "keyring"
    );
    assert_modes("show_none", &grant_show_report(None, &store), &modes, 0);
}

#[test]
fn login_reports_never_include_tokens() {
    let modes = [OutputMode::Rich, OutputMode::Plain, OutputMode::Json];
    assert_modes("login_classic", &classic_login_report(false), &modes, 0);
    assert_modes(
        "login_classic_shadowed",
        &classic_login_report(true),
        &modes,
        0,
    );
    let path = StoredIn::File("/tmp/grant.json".into());
    assert_modes(
        "login_hardware_file",
        &hardware_login_report("~someone", Some(&path)),
        &modes,
        0,
    );
    for (stored_in, storage) in [
        (Some(StoredIn::Keyring), "keyring"),
        (Some(StoredIn::Memory), "memory"),
        (None, "unchanged"),
    ] {
        let report = hardware_login_report("someone", stored_in.as_ref());
        assert_eq!(report.json["lpcli"]["grant_storage"], storage);
        assert_eq!(report.json["lpcli"]["account"], "someone");
    }
}

#[test]
fn clear_cache_and_revoke_reports_are_lpcli_objects() {
    let modes = [OutputMode::Rich, OutputMode::Plain, OutputMode::Json];
    assert_modes("clear_cache", &clear_cache_report(), &modes, 0);

    let key: RegisteredAuthenticator =
        serde_json::from_value(registered_key(7, "Primary", false)).unwrap();
    let report = revoke_key_report(&key);
    assert_eq!(report.json["lpcli"]["revoked"], true);
    assert_eq!(report.json["lpcli"]["security_key"]["id"], 7);
    assert_modes("revoke_key", &report, &modes, 0);

    let outcome = grant_revoke_outcome(true, Some(Ok(())));
    outcome.result.unwrap();
    assert_modes("revoke", &outcome.report.unwrap(), &modes, 0);
    for (stored, expected) in [(true, "unchanged"), (false, "none")] {
        let outcome = grant_revoke_outcome(stored, None);
        outcome.result.unwrap();
        assert_eq!(
            outcome.report.unwrap().json["lpcli"]["stored_copy"],
            expected
        );
    }
}

// -- Registration ----------------------------------------------------------------

/// A software security key that creates one credential and signs nothing.
struct FakeKey;

impl lpcli::hardware_key::Authenticator for FakeKey {
    fn get_assertion(
        &self,
        _: &lpcli::hardware_key::AssertionRequest,
    ) -> lpcli::error::Result<lpcli::hardware_key::Assertion> {
        panic!("a classic session never needs an assertion")
    }

    fn make_credential(
        &self,
        request: &lpcli::hardware_key::RegistrationRequest,
    ) -> lpcli::error::Result<lpcli::hardware_key::Registration> {
        assert_eq!(request.rp_id, "launchpad.net");
        Ok(lpcli::hardware_key::Registration {
            credential_id: b"new-credential".to_vec(),
            attestation_object: b"fake-attestation".to_vec(),
        })
    }
}

/// Mock a registration with a classic session whose read-back of key 42
/// answers `status` and `read_back`.
async fn mock_registration(
    server: &mut mockito::Server,
    status: usize,
    read_back: &str,
) -> Vec<mockito::Mock> {
    let base = server.url();
    let me = format!("{base}/~example-owner");
    let encode = lpcli::hardware_tokens::encode_base64url;
    let options = serde_json::json!({"publicKey": {
        "rp": {"id": "launchpad.net", "name": "Launchpad"},
        "user": {"id": encode(b"user"), "name": "example-owner", "displayName": "Example"},
        "challenge": encode(&[9; 32]),
        "pubKeyCredParams": [{"type": "public-key", "alg": -7}],
    }});
    let op = |name: &str| mockito::Matcher::Regex(format!("ws.op={name}"));
    vec![
        server
            .mock("GET", "/")
            .with_body(serde_json::json!({"me_link": me}).to_string())
            .create_async()
            .await,
        server
            .mock("GET", "/~example-owner")
            .with_body(serde_json::json!({"name": "example-owner", "self_link": me}).to_string())
            .create_async()
            .await,
        server
            .mock("POST", "/~example-owner")
            .match_body(op("beginWebAuthnRegistration"))
            .with_body(options.to_string())
            .create_async()
            .await,
        server
            .mock("POST", "/~example-owner")
            .match_body(mockito::Matcher::AllOf(vec![
                op("completeWebAuthnRegistration"),
                mockito::Matcher::Regex("name=Backup\\+key".into()),
            ]))
            .with_status(201)
            .with_header("location", &format!("{me}/+authenticator/42"))
            .create_async()
            .await,
        server
            .mock("GET", "/~example-owner/+authenticator/42")
            .with_status(status)
            .with_body(read_back)
            .create_async()
            .await,
    ]
}

#[tokio::test]
async fn register_prints_the_registered_key_as_launchpad_returned_it() {
    let mut server = isolated_mock_server().await;
    let entry = registered_key(42, "Backup key", false);
    let mocks = mock_registration(&mut server, 200, &entry.to_string()).await;
    let key = lpcli::hardware_tokens::register_hardware_key_raw(
        &mock_read_client(&server, true),
        "Backup key",
        Arc::new(FakeKey),
    )
    .await
    .unwrap();
    for mock in mocks {
        mock.assert_async().await;
    }

    let report = register_report(&key, "Backup key");
    assert_eq!(report.json, entry, "the read-back entry, unchanged");
    assert_modes(
        "register",
        &report,
        &[OutputMode::Rich, OutputMode::Plain, OutputMode::Json],
        0,
    );
    // --quiet prints only the registered key's self_link.
    assert_eq!(
        raw_self_link(&key.raw),
        Some("https://api.launchpad.net/devel/~example-owner/+authenticator/42")
    );
    // An entry without a self_link prints nothing with --quiet.
    assert_eq!(raw_self_link(&serde_json::json!({"id": 42})), None);
}

#[tokio::test]
async fn register_read_back_failure_is_partial_with_exit_code_7() {
    let mut server = isolated_mock_server().await;
    let base = server.url();
    let mocks = mock_registration(&mut server, 503, "Service unavailable").await;
    let err = lpcli::hardware_tokens::register_hardware_key_raw(
        &mock_read_client(&server, true),
        "Backup key",
        Arc::new(FakeKey),
    )
    .await
    .unwrap_err();
    for mock in mocks {
        mock.assert_async().await;
    }

    assert!(matches!(err, LpError::Partial(_)), "{err:?}");
    assert_eq!(err.exit_code(), 7);
    let json = Output::fixed(OutputMode::Json, false, 100).render_error(&err);
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["error"]["kind"], "partial");
    let message = value["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("Registering the security key succeeded"),
        "{message}"
    );
    assert!(message.contains("lpcli hardware-token keys"), "{message}");
    insta::assert_snapshot!(
        "identity_register_partial_json",
        json.replace(&base, "[server]")
    );
}

// -- Issuing a grant -------------------------------------------------------------

fn issue_request(key: &RegisteredAuthenticator) -> IssueGrantRequest {
    IssueGrantRequest {
        consumer_key: hardware_tokens::HARDWARE_CONSUMER_KEY.into(),
        authenticator_link: key.self_link.clone().unwrap(),
        permission: PermissionArg::ReadAnything.into(),
        context: Some("ubuntu/curl".into()),
        date_expires: Some("2027-01-02T00:00:00Z".parse().unwrap()),
    }
}

/// Issue a grant from a mocked Launchpad, whose response holds the grant's
/// token key and secret.
async fn issued_grant() -> HardwareTokenGrant {
    let mut server = isolated_mock_server().await;
    let me = format!("{}/~example-owner", server.url());
    let _root = server
        .mock("GET", "/")
        .with_body(serde_json::json!({"me_link": me}).to_string())
        .create_async()
        .await;
    let _person = server
        .mock("GET", "/~example-owner")
        .with_body(serde_json::json!({"name": "example-owner", "self_link": me}).to_string())
        .create_async()
        .await;
    let issue = server
        .mock("POST", "/~example-owner")
        .match_body(mockito::Matcher::Regex(
            "ws.op=issueHardwareBoundOAuthToken".into(),
        ))
        .with_body(
            serde_json::json!({
                "oauth_token": SECRETS[0],
                "oauth_token_secret": SECRETS[1],
            })
            .to_string(),
        )
        .create_async()
        .await;
    let key: RegisteredAuthenticator =
        serde_json::from_value(registered_key(7, "Primary", false)).unwrap();
    let grant = hardware_tokens::issue_hardware_bound_oauth_token(
        &mock_read_client(&server, true),
        &issue_request(&key),
    )
    .await
    .unwrap();
    issue.assert_async().await;
    grant
}

#[tokio::test]
async fn issue_reports_the_grant_without_its_token_key_or_secret() {
    let grant = issued_grant().await;
    assert_eq!(grant.token(), SECRETS[0]);
    let key: RegisteredAuthenticator =
        serde_json::from_value(registered_key(7, "Primary", false)).unwrap();
    let request = issue_request(&key);
    let modes = [OutputMode::Rich, OutputMode::Plain, OutputMode::Json];

    // Issued with a classic token, which should now be removed.
    let path = StoredIn::File("/tmp/grant.json".into());
    let outcome = issue_outcome(&key, &request, Ok((grant.account(), Some(&path))), true);
    outcome.result.unwrap();
    let report = outcome.report.unwrap();
    assert_eq!(
        report.json,
        serde_json::json!({"lpcli": {
            "issued": true,
            "security_key": {
                "id": 7,
                "name": "Primary",
                "self_link": "https://api.launchpad.net/devel/~example-owner/+authenticator/7",
            },
            "grant": {
                "consumer": "lpcli-hardware",
                "permission": "Read Anything",
                "context": "ubuntu/curl",
                "date_expires": "2027-01-02T00:00:00Z",
            },
            "login": {
                "credentials": "hardware_token",
                "account": "example-owner",
                "grant_storage": "file",
                "path": "/tmp/grant.json",
            },
            "remove_classic_bootstrap_token": true,
        }})
    );
    assert_modes("issue", &report, &modes, 0);
    // No self_link: --quiet prints nothing.
    assert_eq!(raw_self_link(&report.json), None);

    // Issued with a hardware-bound session: no bootstrap token to remove.
    let outcome = issue_outcome(&key, &request, Ok((grant.account(), None)), false);
    outcome.result.unwrap();
    let report = outcome.report.unwrap();
    assert_eq!(
        report.json["lpcli"]["remove_classic_bootstrap_token"],
        false
    );
    assert_eq!(report.json["lpcli"]["login"]["grant_storage"], "unchanged");
    assert!(!render(&report, OutputMode::Plain).contains("bootstrap"));
    assert_modes("issue_hardware_session", &report, &modes, 0);
}

#[tokio::test]
async fn issue_with_failed_login_is_partial_with_exit_code_7() {
    let key: RegisteredAuthenticator =
        serde_json::from_value(registered_key(7, "Primary", false)).unwrap();
    let failure = LpError::HardwareKey("no security key found".into());
    let outcome = issue_outcome(&key, &issue_request(&key), Err(&failure), true);
    let err = outcome.result.unwrap_err();
    assert!(matches!(err, LpError::Partial(_)), "{err:?}");
    assert_eq!(err.exit_code(), 7);
    assert!(err.to_string().contains("no security key found"));
    assert!(err.to_string().contains(OAUTH_TOKENS_URL));

    let report = outcome.report.unwrap();
    assert_eq!(report.json["lpcli"]["issued"], true);
    assert_eq!(report.json["lpcli"]["login"], serde_json::Value::Null);
    // The classic token is still the only working login.
    assert_eq!(
        report.json["lpcli"]["remove_classic_bootstrap_token"],
        false
    );
    assert_modes(
        "issue_partial",
        &report,
        &[OutputMode::Rich, OutputMode::Plain, OutputMode::Json],
        0,
    );
}

#[tokio::test]
async fn issue_print_keeps_launchpadlib_grant_format() {
    // `--print` is the one output that contains the grant, unchanged in
    // every mode so launchpadlib can read it.
    let grant = issued_grant().await;
    let printed: serde_json::Value =
        serde_json::from_str(grant.to_issuance_json().as_str()).unwrap();
    assert_eq!(printed["oauth_token"], SECRETS[0]);
    assert_eq!(printed["oauth_token_secret"], SECRETS[1]);
    assert!(printed.get("lpcli").is_none());
}

// -- Exit codes for partial cleanup and revocation -------------------------------

fn logout_args(args: &[&str]) -> LogoutArgs {
    let mut argv = vec!["lpcli", "logout"];
    argv.extend_from_slice(args);
    match parse(&argv).command {
        Command::Logout(args) => args,
        _ => unreachable!(),
    }
}

fn keyring_failure() -> LpError {
    LpError::Keyring("backend locked".into())
}

#[test]
fn logout_attempts_every_step_and_reports_partial_cleanup_with_exit_code_7() {
    let modes = [OutputMode::Rich, OutputMode::Plain, OutputMode::Json];

    let outcome = logout_outcome(&logout_args(&[]), || Ok(()), || Ok(()));
    outcome.result.unwrap();
    let report = outcome.report.unwrap();
    assert_eq!(
        report.json,
        serde_json::json!({"lpcli": {
            "classic_credentials": "removed",
            "hardware_token_grant": "removed",
        }})
    );
    assert_modes("logout", &report, &modes, 0);

    // The grant is still removed after the classic credentials fail.
    let grant_deleted = std::cell::Cell::new(false);
    let outcome = logout_outcome(
        &logout_args(&[]),
        || Err(keyring_failure()),
        || {
            grant_deleted.set(true);
            Ok(())
        },
    );
    assert!(grant_deleted.get());
    let err = outcome.result.unwrap_err();
    assert!(matches!(err, LpError::Partial(_)), "{err:?}");
    assert_eq!(err.exit_code(), 7);
    assert!(err.to_string().contains("backend locked"));
    let report = outcome.report.unwrap();
    assert_eq!(report.json["lpcli"]["classic_credentials"], "failed");
    assert_eq!(report.json["lpcli"]["hardware_token_grant"], "removed");
    assert_modes("logout_partial", &report, &modes, 0);
    let json = Output::fixed(OutputMode::Json, false, 100).render_error(&err);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&json).unwrap()["error"]["kind"],
        "partial"
    );

    // Nothing removed: the original error and its exit code, no report.
    let outcome = logout_outcome(
        &logout_args(&["--hardware-token"]),
        || panic!("--hardware-token keeps the classic credentials"),
        || Err(keyring_failure()),
    );
    assert!(outcome.report.is_none());
    let err = outcome.result.unwrap_err();
    assert!(matches!(err, LpError::Keyring(_)));
    assert_eq!(err.exit_code(), 1);

    let outcome = logout_outcome(
        &logout_args(&[]),
        || Err(keyring_failure()),
        || Err(LpError::Io(std::io::ErrorKind::PermissionDenied.into())),
    );
    assert!(outcome.report.is_none());
    assert_eq!(outcome.result.unwrap_err().exit_code(), 1);

    let outcome = logout_outcome(
        &logout_args(&["--classic"]),
        || Ok(()),
        || panic!("--classic keeps the grant"),
    );
    outcome.result.unwrap();
    assert_eq!(
        outcome.report.unwrap().json["lpcli"]["hardware_token_grant"],
        "skipped"
    );
}

#[test]
fn revoke_with_failed_local_removal_is_partial_with_exit_code_7() {
    let outcome = grant_revoke_outcome(true, Some(Err(keyring_failure())));
    let err = outcome.result.unwrap_err();
    assert!(matches!(err, LpError::Partial(_)), "{err:?}");
    assert_eq!(err.exit_code(), 7);
    assert!(err.to_string().contains("lpcli logout --hardware-token"));
    let report = outcome.report.unwrap();
    assert_eq!(
        report.json,
        serde_json::json!({"lpcli": {"revoked": true, "stored_copy": "failed"}})
    );
    assert_modes(
        "revoke_partial",
        &report,
        &[OutputMode::Rich, OutputMode::Plain, OutputMode::Json],
        0,
    );
}

// -- --no-input ----------------------------------------------------------------

#[test]
fn no_input_grant_secret_fails_before_prompting() {
    let err = grant_secret_from(None, true, || panic!("must not prompt")).unwrap_err();
    assert!(matches!(err, LpError::Usage(_)), "{err:?}");
    assert_eq!(err.exit_code(), 2);
    assert!(err.to_string().contains(GRANT_SECRET_ENV));

    // The environment variable never prompts, with or without --no-input.
    for no_input in [false, true] {
        let secret = grant_secret_from(Some("from-env".into()), no_input, || {
            panic!("must not prompt")
        })
        .unwrap();
        assert_eq!(secret.as_str(), "from-env");
    }
    let secret = grant_secret_from(None, false, || Ok("typed".into())).unwrap();
    assert_eq!(secret.as_str(), "typed");
}

async fn dispatch_no_input(args: &[&str]) -> LpError {
    let mut cli = parse(args);
    cli.no_input = true;
    let env = output::Environment::default();
    let mut output = initial_error_output(&cli, &env, (false, false));
    run_command(
        cli,
        || Ok(config::AppConfig::default()),
        || panic!("must not clear the cache"),
        &mut output,
        &env,
        (false, false),
    )
    .await
    .unwrap_err()
}

#[tokio::test]
async fn no_input_classic_login_and_registration_fail_before_prompting() {
    // validate_cli rejects these early; the handlers must also fail before
    // any prompt or network request if they are reached.
    for args in [
        &["lpcli", "login"][..],
        &["lpcli", "hardware-token", "register", "Backup key"],
        &[
            "lpcli",
            "hardware-token",
            "register",
            "Backup key",
            "--classic",
        ],
    ] {
        let mut cli = parse(args);
        cli.no_input = true;
        assert_eq!(validate_cli(&cli).unwrap_err().exit_code(), 2, "{args:?}");

        let err = dispatch_no_input(args).await;
        assert!(matches!(err, LpError::Usage(_)), "{args:?}: {err:?}");
        assert_eq!(err.exit_code(), 2);
        assert!(err.to_string().contains("--no-input"), "{err}");
    }
}

#[tokio::test]
async fn no_input_grant_import_fails_before_reading_the_secret() {
    // Without the environment variable, the secret would be prompted for.
    // Tests never set it, but a developer's shell might.
    if std::env::var_os(GRANT_SECRET_ENV).is_some() {
        return;
    }
    let args = [
        "lpcli",
        "login",
        "--hardware-token",
        "--account",
        "someone",
        "--grant-token",
        "issued-token",
    ];
    let mut cli = parse(&args);
    cli.no_input = true;
    validate_cli(&cli).unwrap();
    let err = dispatch_no_input(&args).await;
    assert!(matches!(err, LpError::Usage(_)), "{err:?}");
    assert!(err.to_string().contains(GRANT_SECRET_ENV));
}

#[test]
fn no_input_security_key_interaction_never_prompts() {
    let interaction = security_key_interaction(true);
    assert!(matches!(
        interaction.prepare_registration(),
        Err(LpError::HardwareKey(_))
    ));
    assert!(matches!(
        interaction.request_pin(0),
        Err(LpError::HardwareKey(_))
    ));
    assert!(matches!(
        interaction.choose("security key", &["a".into(), "b".into()]),
        Err(LpError::HardwareKey(_))
    ));
    interaction.prepare_assertion().unwrap();
}
