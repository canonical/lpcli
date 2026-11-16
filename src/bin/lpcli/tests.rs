//! Unit tests for the `lpcli` binary: CLI argument parsing and helper
//! functions used by the command handlers.

use super::*;

mod documentation_examples {
    use super::*;

    /// Collect logical shell lines and inline commands with their Markdown source lines.
    fn candidates(markdown: &str) -> Vec<(usize, String)> {
        let mut examples = Vec::new();
        let mut fence: Option<(&str, bool)> = None;
        let mut pending = String::new();
        let mut start_line = 0;

        for (index, line) in markdown.lines().enumerate() {
            let line_number = index + 1;
            let trimmed = line.trim_start();
            if let Some((delimiter, shell)) = fence {
                if trimmed.starts_with(delimiter) {
                    if !pending.is_empty() {
                        examples.push((start_line, std::mem::take(&mut pending)));
                    }
                    fence = None;
                } else if shell && (!trimmed.is_empty() || !pending.is_empty()) {
                    if pending.is_empty() {
                        start_line = line_number;
                    } else {
                        // Join explicit continuations before tokenizing: shlex otherwise emits
                        // an empty argument for a standalone backslash-newline. Keep a trailing
                        // backslash until another line arrives so dangling ones remain errors.
                        pending.pop();
                    }
                    pending.push_str(line);
                    // shlex distinguishes continuations from escaped backslashes and comments.
                    if !(line.ends_with('\\') && shlex::split(&pending).is_none()) {
                        examples.push((start_line, std::mem::take(&mut pending)));
                    }
                }
                continue;
            }

            if let Some(delimiter) = ["```", "~~~"]
                .into_iter()
                .find(|delimiter| trimmed.starts_with(delimiter))
            {
                let language = trimmed[delimiter.len()..].split_whitespace().next();
                fence = Some((delimiter, matches!(language, Some("bash" | "sh"))));
                continue;
            }

            for inline in line.split('`').skip(1).step_by(2) {
                let inline = inline.trim();
                // A bare `lpcli` in prose names the program/consumer, not a command example.
                if inline
                    .strip_prefix("lpcli")
                    .is_some_and(|rest| rest.starts_with(char::is_whitespace))
                {
                    examples.push((line_number, inline.to_owned()));
                }
            }
        }
        if !pending.is_empty() {
            examples.push((start_line, pending));
        }
        examples
    }

    // shlex parses words, not redirection operators. Check placeholder spelling in the
    // source, since tokenization removes the quotes/escapes that make angle brackets literal.
    fn has_unquoted_placeholder(command: &str) -> bool {
        let mut quote = None;
        let mut word_start = true;
        let mut chars = command.chars().peekable();
        while let Some(ch) = chars.next() {
            match ch {
                '\\' if quote != Some('\'') => {
                    chars.next();
                    word_start = false;
                }
                '\'' | '"' if quote.is_none() => {
                    quote = Some(ch);
                    word_start = false;
                }
                '\'' | '"' if quote == Some(ch) => quote = None,
                '#' if quote.is_none() && word_start => {
                    for ch in chars.by_ref() {
                        if ch == '\n' {
                            break;
                        }
                    }
                    word_start = true;
                }
                '<' if quote.is_none() => {
                    if chars
                        .clone()
                        .take_while(|ch| !ch.is_whitespace() && !matches!(ch, '\'' | '"' | '\\'))
                        .any(|ch| ch == '>')
                    {
                        return true;
                    }
                    word_start = false;
                }
                _ if quote.is_none() => word_start = ch.is_whitespace(),
                _ => {}
            }
        }
        false
    }

    fn command_args(command: &str) -> Result<Option<Vec<String>>, &'static str> {
        let mut words =
            shlex::split(command).ok_or("invalid shell quoting or dangling backslash")?;
        let start = if words.first().is_some_and(|word| word == "lpcli") {
            Some(0)
        } else {
            // Support the documented `cat ... | lpcli ...` form, not arbitrary shell scripts.
            // Check command boundaries rather than treating any argument named lpcli as a command.
            words
                .windows(2)
                .position(|pair| pair[0] == "|" && pair[1] == "lpcli")
                .map(|pipe| pipe + 1)
        };
        if start.is_some() && has_unquoted_placeholder(command) {
            return Err(
                "unquoted <...> placeholder is shell redirection, not a CLI argument; use a concrete value or quote literal text",
            );
        }
        Ok(start.map(|start| words.drain(start..).collect()))
    }

    fn validation_failures(document: &str, markdown: &str) -> (usize, Vec<String>) {
        let mut count = 0;
        let mut failures = Vec::new();
        for (line, command) in candidates(markdown) {
            let args = match command_args(&command) {
                Ok(Some(args)) => args,
                Ok(None) => continue,
                Err(error) => {
                    count += 1;
                    failures.push(format!("{document}:{line}: {}\n  {error}", command.trim()));
                    continue;
                }
            };
            count += 1;
            if let Err(error) = Cli::try_parse_from(&args).and_then(|cli| validate_cli(&cli))
                && !matches!(
                    error.kind(),
                    clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
                )
            {
                failures.push(format!("{document}:{line}: {}\n  {error}", command.trim()));
            }
        }
        (count, failures)
    }

    #[test]
    fn documented_cli_examples_parse_and_validate() {
        let mut total = 0;
        let mut failures = Vec::new();
        for (document, markdown) in [
            ("README.md", include_str!("../../../README.md")),
            ("AGENTS.md", include_str!("../../../AGENTS.md")),
        ] {
            let (count, errors) = validation_failures(document, markdown);
            total += count;
            failures.extend(errors);
            eprintln!("{document}: checked {count} lpcli command examples");
        }
        assert!(total > 0, "no documented lpcli command examples found");
        assert!(
            failures.is_empty(),
            "{} of {total} documented lpcli examples failed:\n\n{}",
            failures.len(),
            failures.join("\n\n")
        );
    }

    #[test]
    fn extracts_shell_fences_and_inline_commands_only() {
        let markdown = "Use `lpcli --help` and `lpcli --version`; `lpcli` names the program.\n\
                        ```bash\n\
                        lpcli login\n\
                        cargo build --release\n\
                        ```\n\
                        ~~~sh\n\
                        lpcli status\n\
                        ~~~\n\
                        ```rust\n\
                        `lpcli not-a-command`\n\
                        ```\n\
                        ```\n\
                        lpcli <COMMAND> [OPTIONS]\n\
                        ```\n\
                        Ignore `lpcli-helper --help` and `echo lpcli --help`.\n";
        let examples: Vec<_> = candidates(markdown)
            .into_iter()
            .filter_map(|(line, command)| command_args(&command).unwrap().map(|args| (line, args)))
            .collect();
        assert_eq!(
            examples,
            vec![
                (1, vec!["lpcli".to_owned(), "--help".to_owned()]),
                (1, vec!["lpcli".to_owned(), "--version".to_owned()]),
                (3, vec!["lpcli".to_owned(), "login".to_owned()]),
                (7, vec!["lpcli".to_owned(), "status".to_owned()]),
            ]
        );
    }

    #[test]
    fn shell_quotes_and_comments_preserve_argument_contents() {
        let command =
            r#"lpcli bug comment --bug-id 123456 --message "A \"quote\", # and | lpcli" # ignored"#;
        assert_eq!(
            command_args(command).unwrap().unwrap(),
            [
                "lpcli",
                "bug",
                "comment",
                "--bug-id",
                "123456",
                "--message",
                "A \"quote\", # and | lpcli"
            ]
        );
        let command = "lpcli bug comment --bug-id 123456 --message 'literal # | and \\ slash'";
        assert_eq!(
            command_args(command).unwrap().unwrap().last().unwrap(),
            "literal # | and \\ slash"
        );
        assert!(command_args("# lpcli not-a-command").unwrap().is_none());
        assert_eq!(
            command_args("lpcli bug comment --bug-id 1 --message ''")
                .unwrap()
                .unwrap()
                .last()
                .unwrap(),
            ""
        );
    }

    #[test]
    fn unquoted_placeholders_fail_but_literal_brackets_and_comments_are_allowed() {
        for command in [
            "lpcli person show --name <person>",
            "lpcli login --hardware-token --account <you> --grant-token <oauth_token>",
        ] {
            // These string arguments pass clap, but are not argv in a shell invocation.
            let cli = Cli::try_parse_from(shlex::split(command).unwrap()).unwrap();
            validate_cli(&cli).unwrap();
            let (count, failures) = validation_failures("placeholder.md", &format!("`{command}`"));
            assert_eq!(count, 1);
            assert_eq!(failures.len(), 1);
            assert!(failures[0].starts_with("placeholder.md:1:"));
            assert!(failures[0].contains("unquoted <...> placeholder"));
        }
        for command in [
            "lpcli person show --name '<person>'",
            "lpcli person show --name \"<person>\"",
            r"lpcli person show --name \<person\>",
            "lpcli person show --name person#tag # <placeholder>",
            r#"lpcli person show --name "A \"quote\" and <literal>""#,
        ] {
            let (count, failures) = validation_failures("literal.md", &format!("`{command}`"));
            assert_eq!(count, 1);
            assert!(failures.is_empty(), "{}", failures.join("\n"));
        }
    }

    #[test]
    fn continuations_use_shell_unescaping_and_keep_start_lines() {
        let markdown = "```bash\n\
                        # This comment does not continue \\\n\
                        lpcli hardware-token register \\\n\
                            \"My Yubi\\\n\
                        Key\" # trailing comment \\\n\
                        lpcli status\n\
                        lpcli bug comment --bug-id 1 --message backslash\\\\\n\
                        lpcli logout\n\
                        ```";
        let examples: Vec<_> = candidates(markdown)
            .into_iter()
            .filter_map(|(line, command)| command_args(&command).unwrap().map(|args| (line, args)))
            .collect();
        assert_eq!(examples.len(), 4);
        assert_eq!(examples[0].0, 3);
        assert_eq!(
            examples[0].1,
            ["lpcli", "hardware-token", "register", "My YubiKey"]
        );
        assert_eq!(
            examples[1],
            (6, vec!["lpcli".to_owned(), "status".to_owned()])
        );
        assert_eq!(examples[2].1.last().unwrap(), "backslash\\");
        assert_eq!(examples[3].0, 8);
        let (_, failures) = validation_failures("dangling.md", "```sh\nlpcli status \\\n```");
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("dangling backslash"));
    }

    #[test]
    fn input_pipe_extracts_lpcli_but_not_upstream_arguments() {
        let command = "cat 'description with spaces.txt' | lpcli bug set-description \\\n    --bug-id 123456 --description-file - # stdin";
        let examples = candidates(&format!("```sh\n{command}\n```"));
        assert_eq!(examples.len(), 1);
        assert_eq!(examples[0].0, 2);
        assert_eq!(
            command_args(&examples[0].1).unwrap().unwrap(),
            [
                "lpcli",
                "bug",
                "set-description",
                "--bug-id",
                "123456",
                "--description-file",
                "-"
            ]
        );
        let (count, failures) = validation_failures("pipe.md", &format!("```sh\n{command}\n```"));
        assert_eq!(count, 1);
        assert!(failures.is_empty(), "{}", failures.join("\n"));
        assert!(command_args("echo lpcli --help").unwrap().is_none());
        assert!(command_args("cat lpcli | cat").unwrap().is_none());
    }

    #[test]
    fn validation_aggregates_parse_shell_and_semantic_errors() {
        let markdown = "```sh\n\
                        lpcli not-a-command\n\
                        lpcli bug show\n\
                        lpcli package series --series not-a-series\n\
                        lpcli --public login\n\
                        lpcli bug comment --bug-id 1 --message 'unclosed\n\
                        lpcli status\n\
                        ```\n\
                        Finally, `lpcli --version`.";
        let (count, failures) = validation_failures("examples.md", markdown);
        assert_eq!(count, 7);
        assert_eq!(failures.len(), 5, "{}", failures.join("\n"));
        for (failure, line) in failures.iter().zip(2..=6) {
            assert!(
                failure.starts_with(&format!("examples.md:{line}:")),
                "{failure}"
            );
        }
        assert!(failures[2].contains("not-a-series"));
        assert!(failures[3].contains("--public"));
        assert!(failures[4].contains("invalid shell quoting"));
    }

    #[test]
    fn only_help_and_version_error_kinds_count_as_success() {
        let markdown = "```bash\n\
                        lpcli --help\n\
                        lpcli --version\n\
                        lpcli bug --help\n\
                        lpcli help bug\n\
                        lpcli\n\
                        lpcli bug\n\
                        lpcli not-a-command --help\n\
                        ```";
        let (count, failures) = validation_failures("help.md", markdown);
        assert_eq!(count, 7);
        assert_eq!(failures.len(), 3, "{}", failures.join("\n"));
        for (failure, line) in failures.iter().zip(6..=8) {
            assert!(
                failure.starts_with(&format!("help.md:{line}:")),
                "{failure}"
            );
        }
    }
}

#[test]
fn cli_hardware_clear_cache_parses_without_credentials_or_token_arguments() {
    for args in [
        vec!["lpcli", "hardware-token", "clear-cache"],
        vec!["lpcli", "--authenticated", "hardware-token", "clear-cache"],
    ] {
        let cli = Cli::try_parse_from(args).unwrap();
        assert!(matches!(
            cli.command,
            Command::HardwareToken(HardwareTokenCommand::ClearCache)
        ));
        validate_cli(&cli).unwrap();
    }
    assert!(
        Cli::try_parse_from([
            "lpcli",
            "hardware-token",
            "clear-cache",
            "--token",
            "secret"
        ])
        .is_err()
    );
}

#[test]
fn cli_hardware_clear_cache_help_distinguishes_clearing_from_revoking() {
    let help = Cli::try_parse_from(["lpcli", "hardware-token", "clear-cache", "--help"])
        .unwrap_err()
        .to_string();
    let help = help.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(help.contains("without deleting the grant or revoking anything"));
}

#[tokio::test]
async fn clear_cache_dispatch_skips_malformed_config_and_propagates_clear_failure() {
    use lpcli::error::LpError;
    for clear_fails in [false, true] {
        let cli = Cli::try_parse_from(["lpcli", "hardware-token", "clear-cache"]).unwrap();
        validate_cli(&cli).unwrap();
        let config_reads = std::cell::Cell::new(0);
        let clears = std::cell::Cell::new(0);
        let result = run_command(
            cli,
            || {
                config_reads.set(config_reads.get() + 1);
                Err(LpError::Config("malformed config".into()))
            },
            || {
                clears.set(clears.get() + 1);
                if clear_fails {
                    Err(LpError::HardwareToken("cache unavailable".into()))
                } else {
                    Ok(())
                }
            },
        )
        .await;
        assert_eq!(config_reads.get(), 0);
        assert_eq!(clears.get(), 1);
        if clear_fails {
            assert!(matches!(result, Err(LpError::HardwareToken(_))));
        } else {
            result.unwrap();
        }
    }
}

#[tokio::test]
async fn other_commands_still_require_valid_config_before_dispatch() {
    let cli = Cli::try_parse_from(["lpcli", "hardware-token", "show"]).unwrap();
    validate_cli(&cli).unwrap();
    let result = run_command(
        cli,
        || Err(lpcli::error::LpError::Config("malformed config".into())),
        || panic!("only clear-cache bypasses configuration"),
    )
    .await;
    assert!(matches!(result, Err(lpcli::error::LpError::Config(_))));
}

#[test]
fn hardware_cache_config_is_converted_without_overriding_zero() {
    assert_eq!(
        hardware_token_cache_timeout(&config::AppConfig::default()),
        std::time::Duration::from_secs(300)
    );
    for seconds in [0, 1, 600, 86_400] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        std::fs::write(
            &path,
            format!("[authentication]\nhardware_token_cache_seconds = {seconds}\n"),
        )
        .unwrap();
        let settings = config::load_from(&path).unwrap();
        assert_eq!(
            hardware_token_cache_timeout(&settings),
            std::time::Duration::from_secs(seconds)
        );
    }
}

fn parsed_read_policy(args: &[&str]) -> ReadAuthPolicy {
    let cli = Cli::try_parse_from(args).unwrap();
    read_auth_policy(&cli.command, cli.authenticated, cli.public)
}

#[test]
fn direct_reads_use_auth_by_default_and_support_explicit_public() {
    for args in [
        vec!["bug", "show", "--bug-id", "59"],
        vec!["bug", "comments", "--bug-id", "59"],
        vec!["bug", "tasks", "--bug-id", "59"],
        vec!["bug", "attachments", "--bug-id", "59"],
        vec!["bug", "watches", "--bug-id", "59"],
        vec!["person", "show", "--name", "ubuntu"],
        vec!["package", "ppa", "--owner", "ubuntu", "--ppa", "test"],
        vec!["project", "show", "--name", "ubuntu"],
        vec!["cve", "show", "--sequence", "2026-1234"],
        vec!["git", "show", "--path", "~ubuntu/test/+git/test"],
        vec!["git", "default", "--target", "ubuntu"],
        vec!["spec", "show", "--target", "ubuntu", "--name", "test"],
        vec!["question", "show", "--question-id", "59"],
        vec!["snap", "show", "--owner", "ubuntu", "--name", "test"],
    ] {
        let mut full = vec!["lpcli"];
        full.extend(args);
        assert_eq!(
            parsed_read_policy(&full),
            ReadAuthPolicy::Authenticated,
            "{full:?}"
        );
        let mut public = full.clone();
        public.push("--public");
        assert_eq!(parsed_read_policy(&public), ReadAuthPolicy::Anonymous);
        full.push("--authenticated");
        assert_eq!(
            parsed_read_policy(&full),
            ReadAuthPolicy::Authenticated,
            "{full:?}"
        );
    }
}

#[test]
fn private_inclusive_collections_and_identity_require_auth() {
    for args in [
        vec!["bug", "search", "--target", "ubuntu"],
        vec!["bug", "subscriptions", "--bug-id", "59"],
        vec!["person", "bugs", "--name", "ubuntu"],
        vec!["person", "ppas", "--name", "ubuntu"],
        vec!["person", "members", "--team", "ubuntu"],
        vec!["person", "search", "--query", "ubuntu"],
        vec!["person", "owned-teams", "--name", "ubuntu"],
        vec![
            "package",
            "ppa-sources",
            "--owner",
            "ubuntu",
            "--ppa",
            "test",
        ],
        vec!["project", "search", "--query", "test"],
        vec!["project", "list-series", "--project", "test"],
        vec!["git", "list-person-repos", "--name", "ubuntu"],
        vec!["git", "refs", "--path", "~ubuntu/test/+git/test"],
        vec!["git", "proposals", "--path", "~ubuntu/test/+git/test"],
        vec![
            "git",
            "comments",
            "--path",
            "~ubuntu/test/+git/test",
            "--id",
            "59",
        ],
        vec!["snap", "find", "--owner", "ubuntu"],
        vec!["snap", "builds", "--owner", "ubuntu", "--name", "test"],
        vec!["spec", "list", "--target", "test"],
        vec!["question", "search", "--target", "ubuntu"],
        vec!["webhook", "list", "--target", "ubuntu"],
        vec!["status"],
        vec!["hardware-token", "keys"],
        vec!["access-token", "list", "--project", "test"],
    ] {
        let mut full = vec!["lpcli"];
        full.extend(args);
        assert_eq!(
            parsed_read_policy(&full),
            ReadAuthPolicy::Authenticated,
            "{full:?}"
        );
    }
}

#[test]
fn mutations_never_use_anonymous_fallback() {
    for args in [
        vec!["bug", "add-comment", "--bug-id", "59", "--message", "test"],
        vec![
            "bug",
            "delete-comment",
            "--bug-id",
            "59",
            "--comment-index",
            "1",
        ],
        vec!["bug", "subscribe", "--bug-id", "59", "--name", "ubuntu"],
        vec!["bug", "tag", "--bug-id", "59", "--add-tag", "test"],
        vec!["queue", "accept", "--name", "test", "--arch", "source"],
        vec!["queue", "reject", "--name", "test", "--arch", "source"],
        vec![
            "snap",
            "request-builds",
            "--owner",
            "ubuntu",
            "--name",
            "test",
        ],
        vec![
            "webhook",
            "ping",
            "--webhook-url",
            "https://api.launchpad.net/devel/test",
        ],
    ] {
        let mut full = vec!["lpcli"];
        full.extend(args);
        assert_eq!(
            parsed_read_policy(&full),
            ReadAuthPolicy::Authenticated,
            "{full:?}"
        );
    }
}

#[test]
fn public_distribution_reads_do_not_require_auth() {
    for args in [
        vec!["queue", "search", "--status", "New"],
        vec![
            "queue", "info", "--name", "test", "--arch", "source", "--status", "New",
        ],
        vec![
            "queue", "download", "--name", "test", "--arch", "source", "--status", "New",
        ],
        vec!["package", "list-series"],
        vec!["package", "search", "--series", "resolute"],
        vec!["bug", "remote-bug-trackers"],
    ] {
        let mut full = vec!["lpcli"];
        full.extend(args);
        assert_eq!(
            parsed_read_policy(&full),
            ReadAuthPolicy::Anonymous,
            "{full:?}"
        );
        full.insert(1, "--authenticated");
        assert_eq!(parsed_read_policy(&full), ReadAuthPolicy::Authenticated);
    }
}

fn mock_read_client(server: &mockito::Server, signed: bool) -> LaunchpadClient {
    LaunchpadClient::new(signed.then(|| auth::Credentials::new("lpcli", "test-token", "secret")))
        .with_base_url(server.url())
}

#[test]
fn public_mode_conflicts_with_auth_and_rejects_mutations_and_identity() {
    assert!(
        Cli::try_parse_from([
            "lpcli",
            "--public",
            "--authenticated",
            "bug",
            "show",
            "--bug-id",
            "59"
        ])
        .is_err()
    );
    for args in [
        vec!["status"],
        vec!["hardware-token", "keys"],
        vec!["hardware-token", "clear-cache"],
        vec!["bug", "add-comment", "--bug-id", "59", "--message", "test"],
        vec!["queue", "accept", "--name", "test", "--arch", "source"],
    ] {
        let mut full = vec!["lpcli", "--public"];
        full.extend(args);
        let cli = Cli::try_parse_from(full).unwrap();
        assert!(validate_cli(&cli).is_err());
        assert_eq!(
            read_auth_policy(&cli.command, false, true),
            ReadAuthPolicy::Authenticated
        );
    }
    let cli = Cli::try_parse_from(["lpcli", "--public", "bug", "search"]).unwrap();
    assert!(validate_cli(&cli).is_ok());
    assert_eq!(
        parsed_read_policy(&[
            "lpcli",
            "queue",
            "search",
            "--status",
            "New",
            "--archive",
            "~owner/+archive/private"
        ]),
        ReadAuthPolicy::Authenticated
    );
    assert_eq!(
        parsed_read_policy(&[
            "lpcli",
            "queue",
            "search",
            "--status",
            "New",
            "--archive",
            "primary"
        ]),
        ReadAuthPolicy::Anonymous
    );
    for args in [
        vec![
            "lpcli", "queue", "search", "--distro", "Ubuntu", "--status", "New",
        ],
        vec![
            "lpcli",
            "queue",
            "search",
            "--status",
            "New",
            "--archive",
            "/ubuntu/+archive/primary/",
        ],
        vec![
            "lpcli",
            "queue",
            "search",
            "--status",
            "New",
            "--archive",
            "https://api.launchpad.net/devel/ubuntu/+archive/primary/",
        ],
        vec![
            "lpcli", "queue", "info", "--distro", "Ubuntu", "--name", "test", "--arch", "source",
            "--status", "New",
        ],
    ] {
        assert_eq!(
            parsed_read_policy(&args),
            ReadAuthPolicy::Anonymous,
            "{args:?}"
        );
    }
}

#[test]
fn private_direct_reads_never_downgrade_missing_or_failed_credentials() {
    use lpcli::error::LpError;
    for authenticated in [false, true] {
        let mut args = vec!["lpcli", "bug", "show", "--bug-id", "59"];
        if authenticated {
            args.push("--authenticated");
        }
        let policy = parsed_read_policy(&args);
        assert!(matches!(
            read_client_with_auth(policy, || Err(LpError::NotAuthenticated)),
            Err(LpError::NotAuthenticated)
        ));
        for error in [
            LpError::HardwareKeyCancelled,
            LpError::Config("bad grant".into()),
            LpError::Keyring("unavailable".into()),
            LpError::HardwareTokenGrantRejected,
        ] {
            let expected = error.to_string();
            let result = read_client_with_auth(policy, || Err(error));
            assert_eq!(result.err().unwrap().to_string(), expected);
        }
    }
    assert!(
        read_client_with_auth(ReadAuthPolicy::Anonymous, || panic!(
            "--public must not load credentials"
        ))
        .is_ok()
    );
}

#[tokio::test]
async fn private_capable_metadata_is_signed_from_the_first_request() {
    for authenticated in [false, true] {
        let mut args = vec!["lpcli", "bug", "comments", "--bug-id", "59"];
        if authenticated {
            args.push("--authenticated");
        }
        let policy = parsed_read_policy(&args);
        let mut server = mockito::Server::new_async().await;
        let signed = server
            .mock("GET", "/bugs/59/messages")
            .match_header(
                "authorization",
                mockito::Matcher::Regex("oauth_token=\"test-token\"".into()),
            )
            .with_body(r#"{"entries":[{"content":"private"}]}"#)
            .create_async()
            .await;
        let attachments = server
            .mock("GET", "/bugs/59/attachments")
            .match_header(
                "authorization",
                mockito::Matcher::Regex("oauth_token=\"test-token\"".into()),
            )
            .with_body(r#"{"entries":[]}"#)
            .create_async()
            .await;
        let loads = std::cell::Cell::new(0);
        let client = read_client_with_auth(policy, || {
            loads.set(loads.get() + 1);
            Ok(mock_read_client(&server, true))
        })
        .unwrap();
        let read = fetch_bug_comments(&client, 59).await.unwrap();
        assert_eq!(loads.get(), 1);
        assert_eq!(read.comments[0].content.as_deref(), Some("private"));
        signed.assert_async().await;
        attachments.assert_async().await;
    }
}

#[tokio::test]
async fn public_comments_do_not_load_auth_or_retry_on_denial() {
    for status in [200, 401, 403, 404, 500] {
        let mut server = mockito::Server::new_async().await;
        let comments = server
            .mock("GET", "/bugs/59/messages")
            .match_header("authorization", mockito::Matcher::Missing)
            .with_status(status)
            .with_body(r#"{"entries":[{"content":"public"}]}"#)
            .create_async()
            .await;
        let mut attachments_builder = server
            .mock("GET", "/bugs/59/attachments")
            .match_header("authorization", mockito::Matcher::Missing)
            .with_body(r#"{"entries":[]}"#);
        if status != 200 {
            // `fetch_bug_comments` uses `tokio::try_join!`, which cancels the
            // concurrent attachments request if `get_bug_comments` errors first.
            attachments_builder = attachments_builder.expect_at_most(1);
        }
        let attachments = attachments_builder.create_async().await;
        let client = read_client_with_auth(ReadAuthPolicy::Anonymous, || panic!("must not prompt"))
            .unwrap()
            .with_base_url(server.url());
        let result = fetch_bug_comments(&client, 59).await;
        assert_eq!(result.is_ok(), status == 200);
        comments.assert_async().await;
        attachments.assert_async().await;
    }
}

#[tokio::test]
async fn failed_page_exposes_no_partial_data_and_is_not_replayed() {
    let mut server = mockito::Server::new_async().await;
    let first = server.mock("GET", "/bugs/59/messages")
        .with_body(serde_json::json!({"entries":[{"content":"must not render"}], "next_collection_link":format!("{}/next",server.url())}).to_string()).create_async().await;
    let next = server
        .mock("GET", "/next")
        .with_status(401)
        .create_async()
        .await;
    let attachments = server
        .mock("GET", "/bugs/59/attachments")
        .with_body(r#"{"entries":[]}"#)
        .expect_at_most(1)
        .create_async()
        .await;
    let client = mock_read_client(&server, true);
    let result = fetch_bug_comments(&client, 59).await;
    assert!(matches!(
        result,
        Err(lpcli::error::LpError::Api { status: 401, .. })
    ));
    first.assert_async().await;
    next.assert_async().await;
    attachments.assert_async().await;
}

#[tokio::test]
async fn optional_url_enrichment_is_tolerant_and_off_origin_unsigned() {
    let mut api = mockito::Server::new_async().await;
    let mut files = mockito::Server::new_async().await;
    let signed = mock_read_client(&api, true);
    let local = api
        .mock("GET", "/attachment")
        .match_header(
            "authorization",
            mockito::Matcher::Regex("oauth_token=\"test-token\"".into()),
        )
        .with_status(302)
        .with_header("location", &format!("{}/file", files.url()))
        .create_async()
        .await;
    let external = files
        .mock("GET", "/denied")
        .match_header("authorization", mockito::Matcher::Missing)
        .with_status(403)
        .create_async()
        .await;
    let attachments: Vec<bugs::BugAttachment> = serde_json::from_value(serde_json::json!([
        {"data_link":format!("{}/attachment",api.url())},
        {"data_link":format!("{}/denied",files.url())},
        {"data_link":"not a URL"}, {}
    ]))
    .unwrap();
    let urls = resolve_all_attachment_urls(&signed, &attachments).await;
    assert_eq!(
        urls,
        vec![
            format!("{}/file", files.url()),
            "—".into(),
            "—".into(),
            "—".into()
        ]
    );
    local.assert_async().await;
    external.assert_async().await;
}

#[tokio::test]
async fn metadata_auth_is_not_sent_to_transfers_or_redirected_hosts() {
    let mut api = mockito::Server::new_async().await;
    let mut files = mockito::Server::new_async().await;
    let metadata = api
        .mock("GET", "/metadata")
        .match_header(
            "authorization",
            mockito::Matcher::Regex("oauth_token=\"test-token\"".into()),
        )
        .with_body(
            serde_json::json!([
                format!("{}/source.dsc", api.url()),
                format!("{}/direct.tar", files.url())
            ])
            .to_string(),
        )
        .create_async()
        .await;
    let redirect = api
        .mock("GET", "/source.dsc")
        .match_header("authorization", mockito::Matcher::Missing)
        .with_status(302)
        .with_header("location", &format!("{}/source.dsc", files.url()))
        .create_async()
        .await;
    let transfer = files
        .mock("GET", "/source.dsc")
        .match_header("authorization", mockito::Matcher::Missing)
        .with_body("source")
        .create_async()
        .await;
    let direct = files
        .mock("GET", "/direct.tar")
        .match_header("authorization", mockito::Matcher::Missing)
        .with_body("archive")
        .create_async()
        .await;
    let client = mock_read_client(&api, true);
    let urls: Vec<String> = client.get("/metadata").await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    let outcomes = download_public_files(&urls, dir.path()).await.unwrap();
    assert!(
        outcomes
            .iter()
            .all(|o| matches!(o, download::DownloadOutcome::Success(_)))
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("source.dsc")).unwrap(),
        "source"
    );
    metadata.assert_async().await;
    redirect.assert_async().await;
    transfer.assert_async().await;
    direct.assert_async().await;
}

#[tokio::test]
async fn queue_info_restricts_archive_only_for_anonymous_reads() {
    for primary_archive_only in [true, false] {
        let mut server = mockito::Server::new_async().await;
        let query_matcher = if primary_archive_only {
            mockito::Matcher::AllOf(vec![
                mockito::Matcher::UrlEncoded("ws.op".into(), "getPackageUploads".into()),
                mockito::Matcher::UrlEncoded(
                    "archive".into(),
                    format!("{}/ubuntu/+archive/primary", server.url()),
                ),
            ])
        } else {
            mockito::Matcher::Exact(
                "ws.op=getPackageUploads&status=New&name=curl&exact_match=true&ws.size=30".into(),
            )
        };
        let uploads = server
            .mock("GET", "/ubuntu/noble")
            .match_query(query_matcher)
            .with_body(r#"{"entries":[]}"#)
            .create_async()
            .await;
        let client = mock_read_client(&server, !primary_archive_only);
        let err = handle_queue_info(
            &Display::from_width(100),
            &client,
            "curl".into(),
            None,
            "New".into(),
            QueueArch::Source,
            "ubuntu".into(),
            Some("noble".into()),
            primary_archive_only,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, lpcli::error::LpError::NotFound(_)));
        uploads.assert_async().await;
    }
}

#[test]
fn cli_hardware_registration_can_use_classic_credentials() {
    for classic in [false, true] {
        let mut args = vec!["lpcli", "hardware-token", "register", "Backup key"];
        if classic {
            args.push("--classic");
        }
        let cli = Cli::try_parse_from(args).unwrap();
        match cli.command {
            Command::HardwareToken(HardwareTokenCommand::Register {
                name,
                classic: parsed_classic,
            }) => {
                assert_eq!(name, "Backup key");
                assert_eq!(parsed_classic, classic);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }
}

// -----------------------------------------------------------------------
// Helpers shared by multiple tests
// -----------------------------------------------------------------------

/// Build a minimal `BugTask` whose only meaningful field is `target_link`.
fn make_task(target_link: &str) -> bugs::BugTask {
    bugs::BugTask {
        self_link: Some(format!(
            "https://api.launchpad.net/devel{target_link}/+bugtask"
        )),
        bug_link: Some("https://api.launchpad.net/devel/bugs/1".to_string()),
        title: Some("Test bug".to_string()),
        status: Some("New".to_string()),
        importance: Some("Undecided".to_string()),
        assignee_link: None,
        date_created: None,
        bug_target_display_name: None,
        target_link: Some(format!("https://api.launchpad.net/devel{target_link}")),
        bug_watch_link: None,
    }
}

/// A representative set of tasks covering several targets, series, and
/// also series-less rows (distribution-level and upstream project tasks).
fn sample_tasks() -> Vec<bugs::BugTask> {
    vec![
        // rust-alacritty: noble and jammy only
        make_task("/ubuntu/noble/+source/rust-alacritty"),
        make_task("/ubuntu/jammy/+source/rust-alacritty"),
        // rust-eza: noble, jammy, and focal
        make_task("/ubuntu/noble/+source/rust-eza"),
        make_task("/ubuntu/jammy/+source/rust-eza"),
        make_task("/ubuntu/focal/+source/rust-eza"),
        // Series-less rows — included when SeriesFilter::All is used
        make_task("/ubuntu/+source/rust-alacritty"),
        make_task("/rust-alacritty"),
    ]
}

/// Extract the target_link strings from a slice of task references so that
/// test assertions stay readable.
fn target_links<'a>(tasks: &'a [&'a bugs::BugTask]) -> Vec<&'a str> {
    tasks
        .iter()
        .filter_map(|t| t.target_link.as_deref())
        .collect()
}

// -----------------------------------------------------------------------
// parse_comma_separated
// -----------------------------------------------------------------------

#[test]
fn parse_comma_separated_single() {
    assert_eq!(parse_comma_separated("noble"), vec!["noble"]);
}

#[test]
fn parse_comma_separated_two() {
    assert_eq!(
        parse_comma_separated("noble, jammy"),
        vec!["noble", "jammy"]
    );
}

#[test]
fn parse_comma_separated_trims_whitespace() {
    assert_eq!(
        parse_comma_separated("  noble  ,  jammy  "),
        vec!["noble", "jammy"]
    );
}

#[test]
fn parse_comma_separated_ignores_empty_segments() {
    assert_eq!(
        parse_comma_separated(",noble,,jammy,"),
        vec!["noble", "jammy"]
    );
}

// -----------------------------------------------------------------------
// Ubuntu series and pocket parsing
// -----------------------------------------------------------------------

#[test]
fn validate_ubuntu_series_accepts_code_name_and_version() {
    assert!(validate_ubuntu_series("noble").is_ok());
    assert!(validate_ubuntu_series("Noble Numbat").is_ok());
    assert!(validate_ubuntu_series("24.04").is_ok());
    assert!(validate_ubuntu_series("devel").is_ok());
}

#[test]
fn validate_ubuntu_series_rejects_unknown_series() {
    let err = validate_ubuntu_series("not-a-series").unwrap_err();
    assert!(err.contains("not-a-series"), "err: {err}");
    assert!(err.contains("noble"), "err: {err}");
}

#[test]
fn validate_ubuntu_series_list_trims_and_skips_empty_entries() {
    assert!(validate_ubuntu_series_list("  noble  ,, jammy , ").is_ok());
    assert!(validate_ubuntu_series_list("Noble Numbat, 24.04").is_ok());
    assert!(validate_ubuntu_series_list("noble, not-a-series").is_err());
}

#[test]
fn pocket_parser_is_case_insensitive() {
    assert_eq!(parse_pocket("Security").unwrap(), Pocket::Security);
    assert_eq!(parse_pocket("security").unwrap(), Pocket::Security);
    assert_eq!(parse_pocket("SECURITY").unwrap(), Pocket::Security);
}

#[test]
fn pocket_parser_rejects_invalid_pocket() {
    let err = parse_pocket("testing").unwrap_err();
    assert!(err.contains("testing"), "err: {err}");
    assert!(err.contains("release"), "err: {err}");
}

#[test]
fn queue_status_parser_returns_launchpad_spelling() {
    for (input, expected) in [
        ("new", "New"),
        ("unapproved", "Unapproved"),
        ("accepted", "Accepted"),
        ("done", "Done"),
        ("rejected", "Rejected"),
        ("New", "New"),
        ("UNAPPROVED", "Unapproved"),
        (" Done ", "Done"),
    ] {
        assert_eq!(
            parse_queue_status(input).unwrap(),
            expected,
            "input: {input}"
        );
    }
}

#[test]
fn queue_status_parser_rejects_unknown_status() {
    let err = parse_queue_status("pending").unwrap_err();
    assert!(err.contains("pending"), "err: {err}");
    assert!(
        err.contains("new, unapproved, accepted, done, rejected"),
        "err: {err}"
    );
    assert!(parse_queue_status("").is_err());
}

#[test]
fn cli_queue_search_accepts_lowercase_status() {
    let cli = Cli::try_parse_from([
        "lpcli", "queue", "search", "--pocket", "proposed", "--status", "new",
    ])
    .unwrap();
    match cli.command {
        Command::Queue(QueueCommand::Search { pocket, status, .. }) => {
            assert_eq!(pocket, Some(Pocket::Proposed));
            assert_eq!(status.as_deref(), Some("New"));
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn cli_queue_commands_normalise_status() {
    for subcommand in ["download", "info", "accept", "reject"] {
        let cli = Cli::try_parse_from([
            "lpcli",
            "queue",
            subcommand,
            "--name",
            "curl",
            "--status",
            "unapproved",
            "--arch",
            "source",
        ])
        .unwrap();
        let status = match cli.command {
            Command::Queue(
                QueueCommand::Download { status, .. }
                | QueueCommand::Info { status, .. }
                | QueueCommand::Accept { status, .. }
                | QueueCommand::Reject { status, .. },
            ) => status,
            other => panic!("unexpected command: {other:?}"),
        };
        assert_eq!(status, "Unapproved", "subcommand: {subcommand}");
    }
}

#[test]
fn cli_queue_accept_and_reject_default_to_new() {
    for subcommand in ["accept", "reject"] {
        let cli = Cli::try_parse_from([
            "lpcli", "queue", subcommand, "--name", "curl", "--arch", "source",
        ])
        .unwrap();
        match cli.command {
            Command::Queue(
                QueueCommand::Accept { status, .. } | QueueCommand::Reject { status, .. },
            ) => assert_eq!(status, "New", "subcommand: {subcommand}"),
            other => panic!("unexpected command: {other:?}"),
        }
    }
}

#[test]
fn cli_queue_rejects_unknown_status() {
    let result = Cli::try_parse_from(["lpcli", "queue", "search", "--status", "pending"]);
    assert!(result.is_err());
}

#[test]
fn pocket_launchpad_names_are_capitalised() {
    assert_eq!(pocket_launchpad_name(Pocket::Release), "Release");
    assert_eq!(pocket_launchpad_name(Pocket::Security), "Security");
    assert_eq!(pocket_launchpad_name(Pocket::Updates), "Updates");
    assert_eq!(pocket_launchpad_name(Pocket::Proposed), "Proposed");
    assert_eq!(pocket_launchpad_name(Pocket::Backports), "Backports");
}

#[test]
fn queue_arch_parser_accepts_architectures() {
    assert_eq!(
        parse_queue_arch("amd64").unwrap(),
        QueueArch::Arch(Architecture::Amd64)
    );
    assert_eq!(
        parse_queue_arch("ARM64").unwrap(),
        QueueArch::Arch(Architecture::Arm64)
    );
    assert_eq!(
        parse_queue_arch("riscv64").unwrap(),
        QueueArch::Arch(Architecture::Riscv64)
    );
}

#[test]
fn queue_arch_parser_accepts_special_values() {
    assert_eq!(parse_queue_arch("source").unwrap(), QueueArch::Source);
    assert_eq!(parse_queue_arch("SOURCE").unwrap(), QueueArch::Source);
    assert_eq!(parse_queue_arch("all").unwrap(), QueueArch::All);
    assert_eq!(parse_queue_arch("ALL").unwrap(), QueueArch::All);
}

#[test]
fn queue_arch_parser_rejects_unknown_arch() {
    let err = parse_queue_arch("not-an-arch").unwrap_err();
    assert!(err.contains("not-an-arch"), "err: {err}");
    assert!(err.contains("amd64"), "err: {err}");
    assert!(err.contains("source"), "err: {err}");
}

#[test]
fn queue_arch_display_is_canonical() {
    assert_eq!(QueueArch::Source.to_string(), "source");
    assert_eq!(QueueArch::All.to_string(), "all");
    assert_eq!(QueueArch::Arch(Architecture::Amd64).to_string(), "amd64");
}

#[test]
fn cli_rejects_invalid_arch() {
    let result = Cli::try_parse_from([
        "lpcli",
        "queue",
        "download",
        "--name",
        "curl",
        "--status",
        "New",
        "--arch",
        "not-an-arch",
    ]);
    assert!(result.is_err());
}

#[test]
fn cli_accepts_valid_arch() {
    let cli = Cli::try_parse_from([
        "lpcli", "queue", "download", "--name", "curl", "--status", "New", "--arch", "amd64",
    ])
    .unwrap();
    match cli.command {
        Command::Queue(QueueCommand::Download { arch, .. }) => {
            assert_eq!(arch, QueueArch::Arch(Architecture::Amd64));
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn canonical_series_uses_short_name_for_ubuntu() {
    assert_eq!(canonical_series("ubuntu", "Noble Numbat"), "noble");
    assert_eq!(canonical_series("Ubuntu", "24.04"), "noble");
}

#[test]
fn canonical_series_preserves_non_ubuntu_series() {
    assert_eq!(canonical_series("debian", "bookworm"), "bookworm");
    assert_eq!(canonical_series("debian", "Noble Numbat"), "Noble Numbat");
}

#[test]
fn canonical_series_list_normalises_ubuntu_series() {
    assert_eq!(
        canonical_series_list("ubuntu", "  Noble  ,, jammy , "),
        "noble, jammy"
    );
    assert_eq!(
        canonical_series_list("debian", "  bookworm  ,, trixie , "),
        "  bookworm  ,, trixie , "
    );
}

#[test]
fn canonical_bug_series_normalises_ubuntu_target() {
    let (series, many) = canonical_bug_series(
        Some("ubuntu"),
        Some("Noble".to_string()),
        Some("  Noble  ,, jammy , ".to_string()),
    );
    assert_eq!(series.as_deref(), Some("noble"));
    assert_eq!(many.as_deref(), Some("noble, jammy"));
}

#[test]
fn canonical_bug_series_preserves_non_ubuntu_target() {
    let (series, many) = canonical_bug_series(
        Some("debian"),
        Some("Bookworm".to_string()),
        Some("  bookworm  ,, trixie , ".to_string()),
    );
    assert_eq!(series.as_deref(), Some("Bookworm"));
    assert_eq!(many.as_deref(), Some("  bookworm  ,, trixie , "));
}

#[test]
fn validate_series_for_distro_rejects_invalid_ubuntu_series() {
    assert!(validate_series_for_distro("ubuntu", "not-a-series").is_err());
    assert!(validate_series_for_distro("debian", "not-a-series").is_ok());
}

#[test]
fn cli_rejects_invalid_pocket() {
    let result = Cli::try_parse_from([
        "lpcli", "package", "search", "--series", "noble", "--pocket", "testing",
    ]);
    assert!(result.is_err());
}

#[test]
fn cli_accepts_case_insensitive_pocket() {
    let cli = Cli::try_parse_from([
        "lpcli", "package", "search", "--series", "noble", "--pocket", "Release",
    ])
    .unwrap();
    match cli.command {
        Command::Package(PackageCommand::Search { pocket, .. }) => {
            assert_eq!(pocket, Some(Pocket::Release));
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

#[test]
fn validate_cli_rejects_invalid_ubuntu_series_for_package() {
    let cli =
        Cli::try_parse_from(["lpcli", "package", "series", "--series", "not-a-series"]).unwrap();
    assert!(validate_cli(&cli).is_err());
}

#[test]
fn validate_cli_allows_non_ubuntu_series_for_package() {
    let cli = Cli::try_parse_from([
        "lpcli", "package", "series", "--series", "bookworm", "--distro", "debian",
    ])
    .unwrap();
    assert!(validate_cli(&cli).is_ok());
}

#[test]
fn validate_cli_rejects_invalid_ubuntu_series_for_queue() {
    let cli = Cli::try_parse_from([
        "lpcli",
        "queue",
        "search",
        "--status",
        "New",
        "--series",
        "not-a-series",
    ])
    .unwrap();
    assert!(validate_cli(&cli).is_err());
}

#[test]
fn validate_cli_allows_non_ubuntu_series_for_queue() {
    let cli = Cli::try_parse_from([
        "lpcli", "queue", "search", "--status", "New", "--series", "bookworm", "--distro", "debian",
    ])
    .unwrap();
    assert!(validate_cli(&cli).is_ok());
}

#[test]
fn validate_cli_rejects_invalid_ubuntu_series_for_add_task() {
    let cli = Cli::try_parse_from([
        "lpcli",
        "bug",
        "add-task",
        "--bug-id",
        "1",
        "--series",
        "not-a-series",
    ])
    .unwrap();
    assert!(validate_cli(&cli).is_err());
}

#[test]
fn validate_cli_allows_full_series_names_for_add_task() {
    let cli = Cli::try_parse_from([
        "lpcli",
        "bug",
        "add-task",
        "--bug-id",
        "1",
        "--series",
        "Noble Numbat",
    ])
    .unwrap();
    assert!(validate_cli(&cli).is_ok());
}

#[test]
fn validate_cli_allows_non_ubuntu_series_for_add_task() {
    let cli = Cli::try_parse_from([
        "lpcli", "bug", "add-task", "--bug-id", "1", "--target", "debian", "--series", "bookworm",
    ])
    .unwrap();
    assert!(validate_cli(&cli).is_ok());
}

#[test]
fn validate_cli_rejects_invalid_ubuntu_series_for_link_remote() {
    let cli = Cli::try_parse_from([
        "lpcli",
        "bug",
        "link-remote",
        "--bug-id",
        "1",
        "--bug-tracker",
        "debbugs",
        "--remote-bug",
        "1",
        "--target",
        "ubuntu",
        "--series",
        "not-a-series",
    ])
    .unwrap();
    assert!(validate_cli(&cli).is_err());
}

#[test]
fn validate_cli_allows_full_series_names_for_link_remote() {
    let cli = Cli::try_parse_from([
        "lpcli",
        "bug",
        "link-remote",
        "--bug-id",
        "1",
        "--bug-tracker",
        "debbugs",
        "--remote-bug",
        "1",
        "--target",
        "ubuntu",
        "--series",
        "Noble Numbat",
    ])
    .unwrap();
    assert!(validate_cli(&cli).is_ok());
}

#[test]
fn validate_cli_allows_non_ubuntu_series_for_link_remote() {
    let cli = Cli::try_parse_from([
        "lpcli",
        "bug",
        "link-remote",
        "--bug-id",
        "1",
        "--bug-tracker",
        "debbugs",
        "--remote-bug",
        "1",
        "--target",
        "debian",
        "--series",
        "bookworm",
    ])
    .unwrap();
    assert!(validate_cli(&cli).is_ok());
}

#[test]
fn validate_cli_does_not_validate_ambiguous_bug_series() {
    // The target is a package name, so the series could belong to any
    // distribution. Validation is intentionally skipped here.
    let cli = Cli::try_parse_from([
        "lpcli",
        "bug",
        "set-status",
        "--bug-id",
        "1",
        "--target",
        "curl",
        "--series",
        "bookworm",
        "--status",
        "new",
    ])
    .unwrap();
    assert!(validate_cli(&cli).is_ok());
}

// -----------------------------------------------------------------------
// TargetFilter::from_args
// -----------------------------------------------------------------------

#[test]
fn target_filter_from_all_targets_flag() {
    let f = TargetFilter::from_args(None, None, true);
    assert_eq!(f, TargetFilter::All);
}

#[test]
fn target_filter_from_many_targets() {
    let f = TargetFilter::from_args(None, Some("rust-alacritty, rust-eza".into()), false);
    assert_eq!(
        f,
        TargetFilter::Many(vec!["rust-alacritty".into(), "rust-eza".into()])
    );
}

#[test]
fn target_filter_from_single_target() {
    let f = TargetFilter::from_args(Some("rust-alacritty".into()), None, false);
    assert_eq!(f, TargetFilter::One("rust-alacritty".into()));
}

// -----------------------------------------------------------------------
// SeriesFilter::from_args
// -----------------------------------------------------------------------

#[test]
fn series_filter_from_all_series_flag() {
    let f = SeriesFilter::from_args(None, None, true, false);
    assert_eq!(f, SeriesFilter::All);
}

#[test]
fn series_filter_from_many_series() {
    let f = SeriesFilter::from_args(None, Some("noble, jammy".into()), false, false);
    assert_eq!(f, SeriesFilter::Many(vec!["noble".into(), "jammy".into()]));
}

#[test]
fn series_filter_from_single_series() {
    let f = SeriesFilter::from_args(Some("noble".into()), None, false, false);
    assert_eq!(f, SeriesFilter::One("noble".into()));
}

#[test]
fn series_filter_from_no_series_flag() {
    let f = SeriesFilter::from_args(None, None, false, true);
    assert_eq!(f, SeriesFilter::NoSeries);
}

// -----------------------------------------------------------------------
// collect_matching_tasks — successful combinations
// -----------------------------------------------------------------------

#[test]
fn collect_one_target_one_series() {
    let tasks = sample_tasks();
    let result = collect_matching_tasks(
        &tasks,
        1,
        &TargetFilter::One("rust-alacritty".into()),
        &SeriesFilter::One("noble".into()),
    )
    .unwrap();
    assert_eq!(result.len(), 1);
    assert!(target_links(&result)[0].contains("noble/+source/rust-alacritty"));
}

#[test]
fn collect_one_target_one_series_case_insensitive() {
    let tasks = sample_tasks();
    let result = collect_matching_tasks(
        &tasks,
        1,
        &TargetFilter::One("Rust-Alacritty".into()),
        &SeriesFilter::One("Noble".into()),
    )
    .unwrap();
    assert_eq!(result.len(), 1);
}

#[test]
fn collect_one_target_many_series() {
    let tasks = sample_tasks();
    let result = collect_matching_tasks(
        &tasks,
        1,
        &TargetFilter::One("rust-alacritty".into()),
        &SeriesFilter::Many(vec!["noble".into(), "jammy".into()]),
    )
    .unwrap();
    assert_eq!(result.len(), 2);
}

#[test]
fn collect_many_targets_one_series() {
    let tasks = sample_tasks();
    let result = collect_matching_tasks(
        &tasks,
        1,
        &TargetFilter::Many(vec!["rust-alacritty".into(), "rust-eza".into()]),
        &SeriesFilter::One("noble".into()),
    )
    .unwrap();
    assert_eq!(result.len(), 2);
}

#[test]
fn collect_many_targets_many_series() {
    let tasks = sample_tasks();
    // rust-alacritty × {noble, jammy} + rust-eza × {noble, jammy} = 4
    let result = collect_matching_tasks(
        &tasks,
        1,
        &TargetFilter::Many(vec!["rust-alacritty".into(), "rust-eza".into()]),
        &SeriesFilter::Many(vec!["noble".into(), "jammy".into()]),
    )
    .unwrap();
    assert_eq!(result.len(), 4);
}

#[test]
fn collect_all_targets_one_series() {
    let tasks = sample_tasks();
    // noble has rust-alacritty and rust-eza → 2 tasks
    let result = collect_matching_tasks(
        &tasks,
        1,
        &TargetFilter::All,
        &SeriesFilter::One("noble".into()),
    )
    .unwrap();
    assert_eq!(result.len(), 2);
}

#[test]
fn collect_one_target_all_series() {
    let tasks = sample_tasks();
    // rust-alacritty: noble, jammy (series-specific) + ubuntu/+source (series-less)
    // + /rust-alacritty (project task, also series-less) = 4
    let result = collect_matching_tasks(
        &tasks,
        1,
        &TargetFilter::One("rust-alacritty".into()),
        &SeriesFilter::All,
    )
    .unwrap();
    assert_eq!(result.len(), 4);
    let links = target_links(&result);
    assert!(links.iter().any(|l| l.contains("noble")));
    assert!(links.iter().any(|l| l.contains("jammy")));
    assert!(
        links
            .iter()
            .any(|l| l.contains("/ubuntu/+source/rust-alacritty"))
    );
    assert!(links.iter().any(|l| l.ends_with("/rust-alacritty")));
}

#[test]
fn collect_all_targets_many_series() {
    let tasks = sample_tasks();
    // noble + jammy: rust-alacritty × 2 + rust-eza × 2 = 4
    let result = collect_matching_tasks(
        &tasks,
        1,
        &TargetFilter::All,
        &SeriesFilter::Many(vec!["noble".into(), "jammy".into()]),
    )
    .unwrap();
    assert_eq!(result.len(), 4);
}

#[test]
fn collect_all_targets_all_series() {
    let tasks = sample_tasks();
    // Series-specific: noble/alacritty, jammy/alacritty, noble/eza, jammy/eza, focal/eza = 5
    // Series-less: ubuntu/+source/rust-alacritty, /rust-alacritty = 2
    // Total = 7
    let result = collect_matching_tasks(&tasks, 1, &TargetFilter::All, &SeriesFilter::All).unwrap();
    assert_eq!(result.len(), 7);
}

#[test]
fn collect_many_targets_all_series() {
    let tasks = sample_tasks();
    // rust-eza: noble, jammy, focal = 3 (no series-less row)
    // rust-alacritty: noble, jammy = 2 + ubuntu/+source (series-less) + /rust-alacritty = 4
    // Total = 7
    let result = collect_matching_tasks(
        &tasks,
        1,
        &TargetFilter::Many(vec!["rust-alacritty".into(), "rust-eza".into()]),
        &SeriesFilter::All,
    )
    .unwrap();
    assert_eq!(result.len(), 7);
}

// -----------------------------------------------------------------------
// collect_matching_tasks — error cases
// -----------------------------------------------------------------------

#[test]
fn collect_invalid_single_target_returns_error() {
    let tasks = sample_tasks();
    let err = collect_matching_tasks(
        &tasks,
        42,
        &TargetFilter::One("nonexistent-pkg".into()),
        &SeriesFilter::One("noble".into()),
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("nonexistent-pkg"), "msg: {msg}");
    assert!(msg.contains("42"), "msg: {msg}");
    assert!(
        msg.contains("rust-alacritty") || msg.contains("rust-eza"),
        "msg: {msg}"
    );
}

#[test]
fn collect_invalid_one_of_many_targets_returns_error() {
    let tasks = sample_tasks();
    let err = collect_matching_tasks(
        &tasks,
        42,
        &TargetFilter::Many(vec!["rust-alacritty".into(), "ghost-pkg".into()]),
        &SeriesFilter::One("noble".into()),
    )
    .unwrap_err();
    let msg = err.to_string();
    // The missing target must be named in the error.
    assert!(msg.contains("ghost-pkg"), "msg: {msg}");
    // The valid targets must appear in the 'Available targets' list.
    assert!(msg.contains("rust-alacritty"), "msg: {msg}");
}

#[test]
fn collect_all_of_many_targets_invalid_returns_error() {
    let tasks = sample_tasks();
    let err = collect_matching_tasks(
        &tasks,
        42,
        &TargetFilter::Many(vec!["ghost-a".into(), "ghost-b".into()]),
        &SeriesFilter::One("noble".into()),
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("ghost-a") && msg.contains("ghost-b"),
        "msg: {msg}"
    );
}

#[test]
fn collect_invalid_single_series_returns_error() {
    let tasks = sample_tasks();
    let err = collect_matching_tasks(
        &tasks,
        42,
        &TargetFilter::One("rust-alacritty".into()),
        &SeriesFilter::One("groovy".into()),
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("groovy"), "msg: {msg}");
    assert!(msg.contains("noble") || msg.contains("jammy"), "msg: {msg}");
}

#[test]
fn collect_invalid_one_of_many_series_returns_error() {
    let tasks = sample_tasks();
    let err = collect_matching_tasks(
        &tasks,
        42,
        &TargetFilter::One("rust-alacritty".into()),
        &SeriesFilter::Many(vec!["noble".into(), "eoan".into()]),
    )
    .unwrap_err();
    let msg = err.to_string();
    // The missing series must be named in the error.
    assert!(msg.contains("eoan"), "msg: {msg}");
    // The valid series must appear in the 'Available series' list.
    assert!(msg.contains("noble"), "msg: {msg}");
}

#[test]
fn collect_valid_target_and_series_but_no_intersection_returns_error() {
    // rust-alacritty has no focal task; rust-eza has focal.
    let tasks = sample_tasks();
    let err = collect_matching_tasks(
        &tasks,
        42,
        &TargetFilter::One("rust-alacritty".into()),
        &SeriesFilter::One("focal".into()),
    )
    .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("No tasks found"), "msg: {msg}");
}

#[test]
fn collect_one_target_no_series() {
    let tasks = sample_tasks();
    // rust-alacritty series-less rows: /ubuntu/+source/rust-alacritty, /rust-alacritty = 2
    let result = collect_matching_tasks(
        &tasks,
        1,
        &TargetFilter::One("rust-alacritty".into()),
        &SeriesFilter::NoSeries,
    )
    .unwrap();
    assert_eq!(result.len(), 2);
    for link in target_links(&result) {
        assert!(
            !link.contains("/noble/") && !link.contains("/jammy/") && !link.contains("/focal/"),
            "expected no series in link: {link}"
        );
    }
}

#[test]
fn collect_all_targets_no_series() {
    let tasks = sample_tasks();
    // Only series-less tasks in sample_tasks: /ubuntu/+source/rust-alacritty, /rust-alacritty = 2
    let result =
        collect_matching_tasks(&tasks, 1, &TargetFilter::All, &SeriesFilter::NoSeries).unwrap();
    assert_eq!(result.len(), 2);
}

#[test]
fn collect_series_less_tasks_included_by_all_series() {
    // SeriesFilter::All includes both series-specific and series-less tasks.
    let tasks = vec![
        make_task("/ubuntu/+source/rust-alacritty"),
        make_task("/rust-alacritty"),
        make_task("/ubuntu/noble/+source/rust-alacritty"),
    ];
    let result = collect_matching_tasks(&tasks, 1, &TargetFilter::All, &SeriesFilter::All).unwrap();
    assert_eq!(result.len(), 3);
    let links = target_links(&result);
    assert!(links.iter().any(|l| l.contains("noble")));
    assert!(
        links
            .iter()
            .any(|l| l.contains("/ubuntu/+source/rust-alacritty"))
    );
    assert!(links.iter().any(|l| l.ends_with("/rust-alacritty")));
}

#[test]
fn collect_empty_tasks_list_returns_error() {
    let tasks: Vec<bugs::BugTask> = vec![];
    let err = collect_matching_tasks(
        &tasks,
        1,
        &TargetFilter::One("rust-alacritty".into()),
        &SeriesFilter::One("noble".into()),
    )
    .unwrap_err();
    // With no tasks the target validation fires first.
    assert!(err.to_string().contains("rust-alacritty"));
}

// -----------------------------------------------------------------------
// build_table (pre-existing)
// -----------------------------------------------------------------------

#[test]
fn build_table_smoke() {
    let display = Display::from_width(100);
    let table = build_table(&display, vec!["Col A", "Col B"]);
    let rendered = table.to_string();
    assert!(rendered.contains("Col A"));
    assert!(rendered.contains("Col B"));
}

#[test]
fn cli_rejects_removed_width_option() {
    for args in [
        vec!["lpcli", "--width", "150", "status"],
        vec!["lpcli", "status", "--width", "150"],
        vec!["lpcli", "bug", "search", "--width", "150"],
        vec!["lpcli", "--width=150", "status"],
    ] {
        let err = Cli::try_parse_from(&args).unwrap_err();
        assert_eq!(
            err.kind(),
            clap::error::ErrorKind::UnknownArgument,
            "{args:?}"
        );
        assert!(err.to_string().contains("--width"), "{args:?}: {err}");
    }
}

#[test]
fn cli_help_does_not_list_width_option() {
    for args in [
        vec!["lpcli", "--help"],
        vec!["lpcli", "status", "--help"],
        vec!["lpcli", "bug", "search", "--help"],
    ] {
        let help = Cli::try_parse_from(&args).unwrap_err();
        assert_eq!(help.kind(), clap::error::ErrorKind::DisplayHelp, "{args:?}");
        assert!(!help.to_string().contains("--width"), "{args:?}: {help}");
    }
}

// -----------------------------------------------------------------------
// parse_tags_from_vec
// -----------------------------------------------------------------------

#[test]
fn parse_tags_from_vec_space_separated() {
    let args = vec!["homestar".into(), "cheat".into(), "strongbad".into()];
    assert_eq!(
        parse_tags_from_vec(&args),
        vec!["homestar", "cheat", "strongbad"]
    );
}

#[test]
fn parse_tags_from_vec_single_comma_separated_string() {
    let args = vec!["homestar, cheat, strongbad".into()];
    assert_eq!(
        parse_tags_from_vec(&args),
        vec!["homestar", "cheat", "strongbad"]
    );
}

#[test]
fn parse_tags_from_vec_shell_split_trailing_commas() {
    // Simulates what the shell hands to the process when the user types:
    //   --add-many-tags homestar, cheat, strongbad
    let args = vec!["homestar,".into(), "cheat,".into(), "strongbad".into()];
    assert_eq!(
        parse_tags_from_vec(&args),
        vec!["homestar", "cheat", "strongbad"]
    );
}

#[test]
fn parse_tags_from_vec_filters_empty_segments() {
    let args = vec![",,,".into(), "  ".into(), "  java  ".into()];
    assert_eq!(parse_tags_from_vec(&args), vec!["java"]);
}

// -----------------------------------------------------------------------
// apply_tag_removals
// -----------------------------------------------------------------------

#[test]
fn apply_tag_removals_single_tag() {
    let current = vec!["apple".into(), "banana".into(), "pear".into()];
    let result = apply_tag_removals(current, Some("pear".into()), None, false);
    assert_eq!(result, vec!["apple", "banana"]);
}

#[test]
fn apply_tag_removals_many_tags_space_separated() {
    let current = vec!["lettuce".into(), "tomato".into(), "cucumber".into()];
    let result = apply_tag_removals(
        current,
        None,
        Some(vec!["lettuce".into(), "tomato".into()]),
        false,
    );
    assert_eq!(result, vec!["cucumber"]);
}

#[test]
fn apply_tag_removals_many_tags_comma_separated_string() {
    let current = vec!["lettuce".into(), "tomato".into(), "cucumber".into()];
    let result = apply_tag_removals(current, None, Some(vec!["lettuce, tomato".into()]), false);
    assert_eq!(result, vec!["cucumber"]);
}

#[test]
fn apply_tag_removals_all_tags() {
    let current = vec!["spam".into(), "eggs".into(), "sausage".into()];
    let result = apply_tag_removals(current, None, None, true);
    assert!(result.is_empty());
}

#[test]
fn apply_tag_removals_all_tags_ignores_other_flags() {
    // remove_all_tags takes precedence over the specific remove flags.
    let current = vec!["spam".into(), "eggs".into()];
    let result = apply_tag_removals(
        current,
        Some("spam".into()),
        Some(vec!["eggs".into()]),
        true,
    );
    assert!(result.is_empty());
}

#[test]
fn apply_tag_removals_no_flags_returns_unchanged() {
    let current = vec!["alpha".into(), "beta".into()];
    let result = apply_tag_removals(current.clone(), None, None, false);
    assert_eq!(result, current);
}

#[test]
fn apply_tag_removals_case_insensitive() {
    let current = vec!["Apple".into(), "Banana".into()];
    let result = apply_tag_removals(current, Some("APPLE".into()), None, false);
    assert_eq!(result, vec!["Banana"]);
}

#[test]
fn apply_tag_removals_nonexistent_tag_leaves_list_unchanged() {
    let current = vec!["alpha".into(), "beta".into()];
    let result = apply_tag_removals(current.clone(), Some("gamma".into()), None, false);
    assert_eq!(result, current);
}

// -----------------------------------------------------------------------
// apply_tag_additions
// -----------------------------------------------------------------------

#[test]
fn apply_tag_additions_single_tag() {
    let current = vec!["existing".into()];
    let result = apply_tag_additions(current, Some("java".into()), None);
    assert_eq!(result, vec!["existing", "java"]);
}

#[test]
fn apply_tag_additions_many_tags_space_separated() {
    let current: Vec<String> = vec![];
    let result = apply_tag_additions(
        current,
        None,
        Some(vec!["homestar".into(), "cheat".into(), "strongbad".into()]),
    );
    assert_eq!(result, vec!["homestar", "cheat", "strongbad"]);
}

#[test]
fn apply_tag_additions_many_tags_comma_separated_string() {
    let current: Vec<String> = vec![];
    let result = apply_tag_additions(
        current,
        None,
        Some(vec!["homestar, cheat, strongbad".into()]),
    );
    assert_eq!(result, vec!["homestar", "cheat", "strongbad"]);
}

#[test]
fn apply_tag_additions_no_flags_returns_unchanged() {
    let current = vec!["alpha".into(), "beta".into()];
    let result = apply_tag_additions(current.clone(), None, None);
    assert_eq!(result, current);
}

#[test]
fn apply_tag_additions_skips_duplicates() {
    let current = vec!["java".into(), "python".into()];
    let result = apply_tag_additions(current, Some("java".into()), None);
    // "java" must not appear twice.
    assert_eq!(result, vec!["java", "python"]);
}

#[test]
fn apply_tag_additions_case_insensitive_duplicate_check() {
    let current = vec!["Java".into()];
    let result = apply_tag_additions(current, Some("JAVA".into()), None);
    // Case-insensitive match: "JAVA" is already present as "Java".
    assert_eq!(result, vec!["Java"]);
}

// -----------------------------------------------------------------------
// remove + add combined
// -----------------------------------------------------------------------

#[test]
fn remove_all_then_add_many() {
    // Bug has: baseball, football, basketball
    // --remove-all-tags --add-many-tags "cricket, badminton"
    let current = vec!["baseball".into(), "football".into(), "basketball".into()];
    let after_remove = apply_tag_removals(current, None, None, true);
    let final_tags =
        apply_tag_additions(after_remove, None, Some(vec!["cricket, badminton".into()]));
    assert_eq!(final_tags, vec!["cricket", "badminton"]);
}

#[test]
fn remove_one_then_add_one() {
    let current = vec!["apple".into(), "banana".into(), "pear".into()];
    let after_remove = apply_tag_removals(current, Some("pear".into()), None, false);
    let final_tags = apply_tag_additions(after_remove, Some("mango".into()), None);
    assert_eq!(final_tags, vec!["apple", "banana", "mango"]);
}

#[test]
fn remove_many_then_add_single() {
    let current = vec!["lettuce".into(), "tomato".into(), "cucumber".into()];
    let after_remove = apply_tag_removals(
        current,
        None,
        Some(vec!["lettuce".into(), "tomato".into()]),
        false,
    );
    let final_tags = apply_tag_additions(after_remove, Some("spinach".into()), None);
    assert_eq!(final_tags, vec!["cucumber", "spinach"]);
}

// -----------------------------------------------------------------------
// CLI argument parsing — Tag subcommand
// -----------------------------------------------------------------------

/// Helper: parse a `BugCommand::Tag` from CLI tokens and return the variant.
fn parse_tag_cmd(args: &[&str]) -> BugCommand {
    let full: Vec<&str> = std::iter::once("lpcli")
        .chain(std::iter::once("bug"))
        .chain(std::iter::once("tag"))
        .chain(args.iter().copied())
        .collect();
    let cli = Cli::try_parse_from(full).expect("CLI parsing failed");
    match cli.command {
        Command::Bug(cmd) => cmd,
        _ => panic!("Expected Bug command"),
    }
}

#[test]
fn cli_tag_add_tag_single() {
    let cmd = parse_tag_cmd(&["--bug-id", "42", "--add-tag", "java"]);
    let BugCommand::Tag {
        bug_id,
        add_tag,
        add_many_tags,
        remove_tag,
        remove_many_tags,
        remove_all_tags,
    } = cmd
    else {
        panic!("Expected Tag variant");
    };
    assert_eq!(bug_id, 42);
    assert_eq!(add_tag, Some("java".to_string()));
    assert!(add_many_tags.is_none());
    assert!(remove_tag.is_none());
    assert!(remove_many_tags.is_none());
    assert!(!remove_all_tags);
}

#[test]
fn cli_tag_add_many_tags_space_separated() {
    let cmd = parse_tag_cmd(&[
        "--bug-id",
        "42",
        "--add-many-tags",
        "homestar",
        "cheat",
        "strongbad",
    ]);
    let BugCommand::Tag { add_many_tags, .. } = cmd else {
        panic!()
    };
    let tags = parse_tags_from_vec(&add_many_tags.unwrap());
    assert_eq!(tags, vec!["homestar", "cheat", "strongbad"]);
}

#[test]
fn cli_tag_add_many_tags_comma_separated() {
    let cmd = parse_tag_cmd(&[
        "--bug-id",
        "42",
        "--add-many-tags",
        "homestar, cheat, strongbad",
    ]);
    let BugCommand::Tag { add_many_tags, .. } = cmd else {
        panic!()
    };
    let tags = parse_tags_from_vec(&add_many_tags.unwrap());
    assert_eq!(tags, vec!["homestar", "cheat", "strongbad"]);
}

#[test]
fn cli_tag_remove_tag_single() {
    let cmd = parse_tag_cmd(&["--bug-id", "99", "--remove-tag", "pear"]);
    let BugCommand::Tag {
        remove_tag,
        remove_all_tags,
        ..
    } = cmd
    else {
        panic!()
    };
    assert_eq!(remove_tag, Some("pear".to_string()));
    assert!(!remove_all_tags);
}

#[test]
fn cli_tag_remove_many_tags() {
    let cmd = parse_tag_cmd(&["--bug-id", "99", "--remove-many-tags", "lettuce", "tomato"]);
    let BugCommand::Tag {
        remove_many_tags, ..
    } = cmd
    else {
        panic!()
    };
    let tags = parse_tags_from_vec(&remove_many_tags.unwrap());
    assert_eq!(tags, vec!["lettuce", "tomato"]);
}

#[test]
fn cli_tag_remove_all_tags() {
    let cmd = parse_tag_cmd(&["--bug-id", "7", "--remove-all-tags"]);
    let BugCommand::Tag {
        remove_all_tags,
        remove_tag,
        remove_many_tags,
        ..
    } = cmd
    else {
        panic!()
    };
    assert!(remove_all_tags);
    assert!(remove_tag.is_none());
    assert!(remove_many_tags.is_none());
}

#[test]
fn cli_tag_remove_all_and_add_many() {
    let cmd = parse_tag_cmd(&[
        "--bug-id",
        "1",
        "--remove-all-tags",
        "--add-many-tags",
        "cricket",
        "badminton",
    ]);
    let BugCommand::Tag {
        remove_all_tags,
        add_many_tags,
        add_tag,
        ..
    } = cmd
    else {
        panic!()
    };
    assert!(remove_all_tags);
    assert!(add_tag.is_none());
    let tags = parse_tags_from_vec(&add_many_tags.unwrap());
    assert_eq!(tags, vec!["cricket", "badminton"]);
}

#[test]
fn cli_tag_add_tag_and_remove_tag_together() {
    let cmd = parse_tag_cmd(&["--bug-id", "1", "--remove-tag", "old", "--add-tag", "new"]);
    let BugCommand::Tag {
        remove_tag,
        add_tag,
        ..
    } = cmd
    else {
        panic!()
    };
    assert_eq!(remove_tag, Some("old".to_string()));
    assert_eq!(add_tag, Some("new".to_string()));
}

#[test]
fn cli_tag_add_tag_and_add_many_tags_mutually_exclusive() {
    let full = [
        "lpcli",
        "bug",
        "tag",
        "--bug-id",
        "1",
        "--add-tag",
        "java",
        "--add-many-tags",
        "python",
    ];
    assert!(
        Cli::try_parse_from(full).is_err(),
        "--add-tag and --add-many-tags must be mutually exclusive"
    );
}

#[test]
fn cli_tag_remove_tag_and_remove_many_tags_mutually_exclusive() {
    let full = [
        "lpcli",
        "bug",
        "tag",
        "--bug-id",
        "1",
        "--remove-tag",
        "apple",
        "--remove-many-tags",
        "banana",
    ];
    assert!(
        Cli::try_parse_from(full).is_err(),
        "--remove-tag and --remove-many-tags must be mutually exclusive"
    );
}

#[test]
fn cli_tag_remove_tag_and_remove_all_tags_mutually_exclusive() {
    let full = [
        "lpcli",
        "bug",
        "tag",
        "--bug-id",
        "1",
        "--remove-tag",
        "apple",
        "--remove-all-tags",
    ];
    assert!(
        Cli::try_parse_from(full).is_err(),
        "--remove-tag and --remove-all-tags must be mutually exclusive"
    );
}

#[test]
fn cli_tag_remove_many_tags_and_remove_all_tags_mutually_exclusive() {
    let full = [
        "lpcli",
        "bug",
        "tag",
        "--bug-id",
        "1",
        "--remove-many-tags",
        "apple",
        "--remove-all-tags",
    ];
    assert!(
        Cli::try_parse_from(full).is_err(),
        "--remove-many-tags and --remove-all-tags must be mutually exclusive"
    );
}

// -----------------------------------------------------------------------
// CLI argument parsing — Create subcommand
// -----------------------------------------------------------------------

/// Helper: parse a `BugCommand::Create` from CLI tokens and return the variant.
fn parse_create_cmd(args: &[&str]) -> BugCommand {
    let full: Vec<&str> = std::iter::once("lpcli")
        .chain(std::iter::once("bug"))
        .chain(std::iter::once("create"))
        .chain(args.iter().copied())
        .collect();
    let cli = Cli::try_parse_from(full).expect("CLI parsing failed");
    match cli.command {
        Command::Bug(cmd) => cmd,
        _ => panic!("Expected Bug command"),
    }
}

#[test]
fn cli_create_bug_basic() {
    let cmd = parse_create_cmd(&[
        "--target",
        "ubuntu",
        "--package",
        "curl",
        "--title",
        "crash on boot",
        "--description",
        "detailed description",
    ]);
    let BugCommand::Create {
        target,
        package,
        title,
        description,
        attachment_file,
        attachment_url,
        attachment_description,
        attachment_type,
    } = cmd
    else {
        panic!("Expected Create variant");
    };
    assert_eq!(target, "ubuntu");
    assert_eq!(package, Some("curl".to_string()));
    assert_eq!(title, "crash on boot");
    assert_eq!(description, "detailed description");
    assert!(attachment_file.is_none());
    assert!(attachment_url.is_none());
    assert!(attachment_description.is_none());
    assert!(attachment_type.is_none());
}

#[test]
fn cli_create_bug_with_attachment_file() {
    let cmd = parse_create_cmd(&[
        "-t",
        "launchpad",
        "-T",
        "new bug",
        "-d",
        "some issue",
        "-f",
        "/tmp/patch.diff",
        "--attachment-description",
        "fix for crash",
        "-a",
        "Patch",
    ]);
    let BugCommand::Create {
        target,
        package,
        title,
        description,
        attachment_file,
        attachment_url,
        attachment_description,
        attachment_type,
    } = cmd
    else {
        panic!("Expected Create variant");
    };
    assert_eq!(target, "launchpad");
    assert!(package.is_none());
    assert_eq!(title, "new bug");
    assert_eq!(description, "some issue");
    assert_eq!(attachment_file, Some("/tmp/patch.diff".to_string()));
    assert!(attachment_url.is_none());
    assert_eq!(attachment_description, Some("fix for crash".to_string()));
    assert_eq!(attachment_type, Some("Patch".to_string()));
}

#[test]
fn cli_create_bug_with_attachment_url() {
    let cmd = parse_create_cmd(&[
        "-T",
        "new bug",
        "-d",
        "some issue",
        "-u",
        "https://example.com/log.txt",
    ]);
    let BugCommand::Create {
        target,
        package,
        title,
        description,
        attachment_file,
        attachment_url,
        attachment_description,
        attachment_type,
    } = cmd
    else {
        panic!("Expected Create variant");
    };
    assert_eq!(target, "ubuntu"); // default value
    assert!(package.is_none());
    assert_eq!(title, "new bug");
    assert_eq!(description, "some issue");
    assert!(attachment_file.is_none());
    assert_eq!(
        attachment_url,
        Some("https://example.com/log.txt".to_string())
    );
    assert!(attachment_description.is_none());
    assert!(attachment_type.is_none());
}

#[test]
fn format_size_standalone_bytes() {
    assert_eq!(format_size(0), "0 B");
    assert_eq!(format_size(512), "512 B");
    assert_eq!(format_size(1023), "1023 B");
}

#[test]
fn format_size_binary_prefixes() {
    assert_eq!(format_size(1_024), "1.0 KiB");
    assert_eq!(format_size(1_536), "1.5 KiB");
    assert_eq!(format_size(1_048_576), "1.0 MiB");
    assert_eq!(format_size(1_073_741_824), "1.0 GiB");
}

// -----------------------------------------------------------------------
// CLI argument parsing — AddTask with remote bug tracker
//
// Bug IDs (42) and remote bug IDs (12345) are arbitrary fake values used
// only to exercise argument parsing; no API calls are made in these tests.
// -----------------------------------------------------------------------

#[test]
fn cli_add_task_with_remote_bug_and_tracker() {
    let full = [
        "lpcli",
        "bug",
        "add-task",
        "--bug-id",
        "42",
        "--target",
        "debian",
        "--package",
        "arctica-greeter",
        "--bug-tracker",
        "debbugs",
        "--remote-bug",
        "12345",
    ];
    let cli = Cli::try_parse_from(full).expect("CLI parsing failed");
    let Command::Bug(BugCommand::AddTask {
        bug_id,
        target,
        package,
        bug_tracker,
        remote_bug,
        ..
    }) = cli.command
    else {
        panic!("Expected AddTask");
    };
    assert_eq!(bug_id, 42);
    assert_eq!(target, "debian");
    assert_eq!(package.as_deref(), Some("arctica-greeter"));
    assert_eq!(bug_tracker.as_deref(), Some("debbugs"));
    assert_eq!(remote_bug.as_deref(), Some("12345"));
}

#[test]
fn cli_link_remote_full() {
    let full = [
        "lpcli",
        "bug",
        "link-remote",
        "--bug-id",
        "42",
        "--target",
        "debian",
        "--package",
        "arctica-greeter",
        "--bug-tracker",
        "debbugs",
        "--remote-bug",
        "12345",
    ];
    let cli = Cli::try_parse_from(full).expect("CLI parsing failed");
    let Command::Bug(BugCommand::LinkRemote {
        bug_id,
        target,
        package,
        bug_tracker,
        remote_bug,
        github,
        series,
        task_url,
    }) = cli.command
    else {
        panic!("Expected LinkRemote");
    };
    assert_eq!(bug_id, 42);
    assert_eq!(target.as_deref(), Some("debian"));
    assert_eq!(package.as_deref(), Some("arctica-greeter"));
    assert_eq!(bug_tracker.as_deref(), Some("debbugs"));
    assert_eq!(remote_bug.as_deref(), Some("12345"));
    assert!(github.is_none(), "explicit form must not set --github");
    assert!(series.is_none());
    assert!(task_url.is_none());
}

#[test]
fn cli_link_remote_aliases() {
    for alias in &["link-remote", "link-task"] {
        let full = [
            "lpcli",
            "bug",
            alias,
            "--bug-id",
            "42",
            "--tracker",
            "debbugs",
            "--remote",
            "12345",
        ];
        let cli = Cli::try_parse_from(full).expect("CLI parsing failed for alias");
        let Command::Bug(BugCommand::LinkRemote {
            bug_id,
            bug_tracker,
            remote_bug,
            github,
            ..
        }) = cli.command
        else {
            panic!("Expected LinkRemote for alias {alias}");
        };
        assert_eq!(bug_id, 42, "alias {alias}");
        assert_eq!(bug_tracker.as_deref(), Some("debbugs"));
        assert_eq!(remote_bug.as_deref(), Some("12345"));
        assert!(github.is_none(), "explicit form must not set --github");
    }
}

#[test]
fn cli_unlink_remote() {
    let full = [
        "lpcli",
        "bug",
        "unlink-remote",
        "--bug-id",
        "42",
        "--target",
        "debian",
    ];
    let cli = Cli::try_parse_from(full).expect("CLI parsing failed");
    let Command::Bug(BugCommand::UnlinkRemote { bug_id, target, .. }) = cli.command else {
        panic!("Expected UnlinkRemote");
    };
    assert_eq!(bug_id, 42);
    assert_eq!(target.as_deref(), Some("debian"));
}

#[test]
fn cli_watches() {
    let full = ["lpcli", "bug", "watches", "--bug-id", "42"];
    let cli = Cli::try_parse_from(full).expect("CLI parsing failed");
    let Command::Bug(BugCommand::Watches { bug_id }) = cli.command else {
        panic!("Expected Watches");
    };
    assert_eq!(bug_id, 42);
}

#[test]
fn cli_remote_bug_trackers_filters_and_limits() {
    let cli = Cli::try_parse_from([
        "lpcli",
        "bug",
        "remote-bug-trackers",
        "--limit",
        "2",
        "--search",
        "DebÜG",
    ])
    .expect("CLI parsing failed");
    let Command::Bug(BugCommand::RemoteBugTrackers { limit, search, .. }) = cli.command else {
        panic!("Expected RemoteBugTrackers");
    };
    assert_eq!(limit, Some(2));
    assert_eq!(search.as_deref(), Some("DebÜG"));
}

#[test]
fn cli_remote_bug_trackers_accepts_short_options() {
    let cli = Cli::try_parse_from([
        "lpcli",
        "bug",
        "remote-bug-trackers",
        "-l",
        "3",
        "-s",
        "debian",
    ])
    .expect("CLI parsing failed");
    let Command::Bug(BugCommand::RemoteBugTrackers { limit, search, .. }) = cli.command else {
        panic!("Expected RemoteBugTrackers");
    };
    assert_eq!(limit, Some(3));
    assert_eq!(search.as_deref(), Some("debian"));
}

#[test]
fn cli_remote_bug_trackers_rejects_zero_limit() {
    let result = Cli::try_parse_from(["lpcli", "bug", "remote-bug-trackers", "--limit", "0"]);
    assert!(result.is_err(), "the limit must be positive");
}

#[test]
fn remote_link_failures_include_all_targets_and_missing_self_link() {
    let failures = vec![
        remote_link_failure("noble", "debbugs", "1150409", "Launchpad API error 500"),
        remote_link_failure("jammy", "debbugs", "1150409", "new task has no self_link"),
    ];
    let err = remote_link_failure_error(42, &failures)
        .expect("remote-link failures should become a command error");
    let message = err.to_string();

    assert!(message.contains("bug #42"));
    assert!(message.contains("task creation may already have succeeded"));
    assert!(message.contains("series 'noble'"));
    assert!(message.contains("series 'jammy'"));
    assert!(message.contains("new task has no self_link"));
}

#[test]
fn no_remote_link_failures_preserve_success() {
    assert!(remote_link_failure_error(42, &[]).is_none());
}

// -----------------------------------------------------------------------
// find_task_to_link unit tests
// -----------------------------------------------------------------------

fn make_test_task(
    self_link: &str,
    target_link: &str,
    display_name: &str,
    watch_link: Option<&str>,
) -> bugs::BugTask {
    bugs::BugTask {
        self_link: Some(self_link.to_string()),
        bug_link: Some("https://api.launchpad.net/devel/bugs/42".to_string()),
        title: Some("Test bug".to_string()),
        status: Some("New".to_string()),
        importance: Some("Undecided".to_string()),
        assignee_link: None,
        date_created: None,
        bug_target_display_name: Some(display_name.to_string()),
        target_link: Some(target_link.to_string()),
        bug_watch_link: watch_link.map(|w| w.to_string()),
    }
}

#[test]
fn find_task_to_link_by_task_url() {
    let t1 = make_test_task(
        "https://api.launchpad.net/devel/ubuntu/+source/curl/+bug/42",
        "https://api.launchpad.net/devel/ubuntu/+source/curl",
        "curl (Ubuntu)",
        None,
    );
    let t2 = make_test_task(
        "https://api.launchpad.net/devel/debian/+source/curl/+bug/42",
        "https://api.launchpad.net/devel/debian/+source/curl",
        "curl (Debian)",
        None,
    );
    let tasks = vec![t1, t2];
    let selector = TaskLinkSelector {
        task_url: Some("https://api.launchpad.net/devel/debian/+source/curl/+bug/42".to_string()),
        ..Default::default()
    };
    let found = find_task_to_link(&tasks, 42, &selector).expect("Should find task by url");

    assert_eq!(
        found.self_link.as_deref(),
        Some("https://api.launchpad.net/devel/debian/+source/curl/+bug/42")
    );
}

#[test]
fn find_task_to_link_by_target_and_distro() {
    let t1 = make_test_task(
        "https://api.launchpad.net/devel/ubuntu/+source/curl/+bug/42",
        "https://api.launchpad.net/devel/ubuntu/+source/curl",
        "curl (Ubuntu)",
        None,
    );
    let t2 = make_test_task(
        "https://api.launchpad.net/devel/debian/+source/curl/+bug/42",
        "https://api.launchpad.net/devel/debian/+source/curl",
        "curl (Debian)",
        None,
    );
    let tasks = vec![t1, t2];
    let selector = TaskLinkSelector {
        target: Some("debian".to_string()),
        ..Default::default()
    };
    let found = find_task_to_link(&tasks, 42, &selector).expect("Should find debian task");

    assert_eq!(
        found.bug_target_display_name.as_deref(),
        Some("curl (Debian)")
    );
}

#[test]
fn find_task_to_link_prefers_series_less() {
    let t1 = make_test_task(
        "https://api.launchpad.net/devel/ubuntu/+source/curl/+bug/42",
        "https://api.launchpad.net/devel/ubuntu/+source/curl",
        "curl (Ubuntu)",
        None,
    );
    let t2 = make_test_task(
        "https://api.launchpad.net/devel/ubuntu/noble/+source/curl/+bug/42",
        "https://api.launchpad.net/devel/ubuntu/noble/+source/curl",
        "curl (Ubuntu Noble)",
        None,
    );
    let tasks = vec![t1, t2];
    let selector = TaskLinkSelector {
        target: Some("curl".to_string()),
        ..Default::default()
    };
    let found = find_task_to_link(&tasks, 42, &selector).expect("Should prefer series-less task");

    assert_eq!(
        found.bug_target_display_name.as_deref(),
        Some("curl (Ubuntu)")
    );
}

#[test]
fn find_task_to_link_with_series() {
    let t1 = make_test_task(
        "https://api.launchpad.net/devel/ubuntu/+source/curl/+bug/42",
        "https://api.launchpad.net/devel/ubuntu/+source/curl",
        "curl (Ubuntu)",
        None,
    );
    let t2 = make_test_task(
        "https://api.launchpad.net/devel/ubuntu/noble/+source/curl/+bug/42",
        "https://api.launchpad.net/devel/ubuntu/noble/+source/curl",
        "curl (Ubuntu Noble)",
        None,
    );
    let tasks = vec![t1, t2];
    let selector = TaskLinkSelector {
        target: Some("curl".to_string()),
        series: Some("noble".to_string()),
        ..Default::default()
    };
    let found = find_task_to_link(&tasks, 42, &selector).expect("Should match noble series");

    assert_eq!(
        found.bug_target_display_name.as_deref(),
        Some("curl (Ubuntu Noble)")
    );
}

#[test]
fn find_task_to_link_ambiguous_fails() {
    let t1 = make_test_task(
        "https://api.launchpad.net/devel/ubuntu/+source/curl/+bug/42",
        "https://api.launchpad.net/devel/ubuntu/+source/curl",
        "curl (Ubuntu)",
        None,
    );
    let t2 = make_test_task(
        "https://api.launchpad.net/devel/debian/+source/curl/+bug/42",
        "https://api.launchpad.net/devel/debian/+source/curl",
        "curl (Debian)",
        None,
    );
    let tasks = vec![t1, t2];
    let selector = TaskLinkSelector {
        package: Some("curl".to_string()),
        ..Default::default()
    };
    let res = find_task_to_link(&tasks, 42, &selector);
    assert!(res.is_err());
}

#[test]
fn find_task_to_link_not_found_fails() {
    let t1 = make_test_task(
        "https://api.launchpad.net/devel/ubuntu/+source/curl/+bug/42",
        "https://api.launchpad.net/devel/ubuntu/+source/curl",
        "curl (Ubuntu)",
        None,
    );
    let tasks = vec![t1];
    let selector = TaskLinkSelector {
        target: Some("debian".to_string()),
        ..Default::default()
    };
    let res = find_task_to_link(&tasks, 42, &selector);
    assert!(res.is_err());
}
// Remote bug linking / tracker registration — test additions. Compact argv
// strings are split on ASCII whitespace; values with embedded whitespace
// (title/summary/name, bad-whitespace refs) keep explicit argv form.

use TrackerType::{Debbugs, Github, Gitlab, GoogleCode};

const WATCH_LINKED_URL: &str = "https://api.launchpad.net/devel/bugs/2174521/+watch/167246";

#[rustfmt::skip]
#[test]
fn cli_link_remote_github_shorthand_and_explicit_pair_parse() {
    // The shorthand targets tasks with the same selectors as the explicit
    // form (-t/-p, and the -u task-url form); the paired explicit form
    // keeps working unchanged now that both arguments are optional.
    let parse = |argv: &str| Cli::try_parse_from(argv.split_ascii_whitespace()).expect("CLI parsing failed");
    let cli = parse("lpcli bug link-remote -b 42 -t ubuntu -p apport --github canonical/apport#658");
    let Command::Bug(BugCommand::LinkRemote { bug_id, target, package, github, bug_tracker, remote_bug, .. }) = cli.command else { panic!("Expected LinkRemote") };
    assert_eq!(bug_id, 42);
    assert_eq!(target.as_deref(), Some("ubuntu"));
    assert_eq!(package.as_deref(), Some("apport"));
    let github = github.expect("--github must parse into owner/repo/number");
    assert_eq!((github.owner(), github.repo(), github.number()), ("canonical", "apport", 658));
    assert!(bug_tracker.is_none() && remote_bug.is_none());
    let task_url = "https://api.launchpad.net/devel/ubuntu/+source/apport/+bug/42";
    let cli = parse("lpcli bug link-remote -b 42 -u https://api.launchpad.net/devel/ubuntu/+source/apport/+bug/42 --github canonical/apport#658");
    let Command::Bug(BugCommand::LinkRemote { task_url: parsed_url, target, github, .. }) = cli.command else { panic!("Expected LinkRemote") };
    assert_eq!(parsed_url.as_deref(), Some(task_url));
    assert!(target.is_none(), "the task-url form needs no -t target");
    assert_eq!(github.expect("--github must parse").number(), 658);
    let cli = parse("lpcli bug link-remote -b 42 -g debbugs -r 1150409");
    let Command::Bug(BugCommand::LinkRemote { bug_tracker, remote_bug, github, .. }) = cli.command else { panic!("Expected LinkRemote") };
    assert_eq!(bug_tracker.as_deref(), Some("debbugs"));
    assert_eq!(remote_bug.as_deref(), Some("1150409"));
    assert!(github.is_none());
}

#[test]
fn cli_link_remote_github_group_conflicts() {
    // --github and the --bug-tracker/--remote-bug pair are mutually
    // exclusive, each pair member requires the other, and with neither
    // there is nothing to link to.
    let rejected = [
        (
            "--github canonical/apport#658 --bug-tracker debbugs",
            "--github mutex --bug-tracker",
        ),
        (
            "--github canonical/apport#658 --remote-bug 658",
            "--github mutex --remote-bug",
        ),
        (
            "--github canonical/apport#658 -g debbugs -r 1",
            "--github mutex explicit pair",
        ),
        (
            "--bug-tracker debbugs",
            "--bug-tracker requires --remote-bug",
        ),
        ("--remote-bug 658", "--remote-bug requires --bug-tracker"),
        ("", "--github or --bug-tracker is required"),
    ];
    for (extra, reason) in rejected {
        let argv = format!("lpcli bug link-remote -b 42 {extra}");
        assert!(
            Cli::try_parse_from(argv.split_ascii_whitespace()).is_err(),
            "link-remote '{extra}' must be rejected ({reason})"
        );
    }
}

#[rustfmt::skip]
#[test]
fn cli_link_remote_github_ref_grammar() {
    // CLI-only strictness on top of the library parser (which tolerates
    // surrounding whitespace and accepts github.com URLs for programmatic
    // use): whitespace and URLs are rejected outright at the value parser.
    for reference in [
        "canonical/apport#0", "canonical/apport", "canonical#658",
        "canonical/apport#six", "canonical/apport #658", "canonical/apport#658 ",
    ] {
        assert!(parse_github_ref(reference).is_err(), "GitHub ref '{reference}' must be rejected");
    }
    let url_error = parse_github_ref("https://github.com/canonical/apport/issues/658")
        .expect_err("full URLs are rejected");
    assert!(url_error.contains("not a URL"), "URL rejection must name the expected form: {url_error}");
    // Owner/repo are canonicalised to lower case by the library's
    // GitHubBugRef; leading zeros collapse to the canonical decimal.
    let parsed = parse_github_ref("Canonical/Apport#007").expect("must parse");
    assert_eq!((parsed.owner(), parsed.repo(), parsed.number()), ("canonical", "apport", 7));
}

#[test]
fn cli_remote_bug_trackers_json_output_flag() {
    // Table output is the default; --json combines with the search/limit
    // filters for machine-readable output.
    let parse = |argv: &str| {
        Cli::try_parse_from(argv.split_ascii_whitespace()).expect("CLI parsing failed")
    };
    let Command::Bug(BugCommand::RemoteBugTrackers { json, .. }) =
        parse("lpcli bug remote-bug-trackers").command
    else {
        panic!("Expected RemoteBugTrackers")
    };
    assert!(!json, "default output must stay the compact table");
    let Command::Bug(BugCommand::RemoteBugTrackers {
        limit,
        search,
        json,
    }) = parse("lpcli bug remote-bug-trackers --json --search canonical/apport --limit 5").command
    else {
        panic!("Expected RemoteBugTrackers")
    };
    assert!(json);
    assert_eq!(limit, Some(5));
    assert_eq!(search.as_deref(), Some("canonical/apport"));
}

/// A representative [`bugs::BugTracker`] for serialization tests (sparse
/// JSON fixture: absent optional fields deserialise as `None`).
#[rustfmt::skip]
fn make_tracker(name: &str) -> bugs::BugTracker {
    serde_json::from_value(serde_json::json!({
        "self_link": format!("https://api.launchpad.net/devel/bugs/bugtrackers/{name}"),
        "name": name,
        "title": format!("{name} tracker"),
        "summary": format!("Summary of {name}"),
        "base_url": "https://example.org/issues",
        "bug_tracker_type": "GitHub Issues",
        "web_link": format!("https://bugs.launchpad.net/bugs/bugtrackers/{name}"),
    }))
    .expect("tracker fixture must deserialise")
}

#[rustfmt::skip]
#[test]
fn trackers_to_json_shape() {
    // The output is a JSON array sorted by name with a fixed key set:
    // optional fields always serialize (null when unset), identical input
    // gives identical bytes, and the caller's data stays untouched.
    let trackers = vec![make_tracker("debbugs"), make_tracker("auto-github-canonical-apport")];
    let text = trackers_to_json(&trackers).expect("serialization should not fail");
    let value: serde_json::Value = serde_json::from_str(&text).expect("output must be valid JSON");
    let entries = value.as_array().expect("JSON output must be an array");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["name"], "auto-github-canonical-apport");
    assert_eq!(entries[1]["name"], "debbugs");
    for entry in entries {
        for key in ["self_link", "title", "bug_tracker_type", "base_url"] {
            assert!(entry.get(key).is_some(), "key '{key}' must always be present");
        }
    }
    assert_eq!(
        trackers.iter().map(|t| t.name.as_deref()).collect::<Vec<_>>(),
        vec![Some("debbugs"), Some("auto-github-canonical-apport")],
        "the caller's input must stay untouched"
    );
    assert_eq!(trackers_to_json(&[]).expect("serialization should not fail"), "[]", "an empty result must stay an empty JSON array");
    let all_null: serde_json::Value = serde_json::from_str(
        &trackers_to_json(std::slice::from_ref(&bugs::BugTracker::default())).expect("serialization should not fail"),
    ).expect("output must be valid JSON");
    for key in ["self_link", "name", "title", "summary", "base_url", "bug_tracker_type", "web_link"] {
        assert!(all_null[0].get(key).is_some_and(|v| v.is_null()), "key '{key}' must stay null");
    }
    let again = trackers_to_json(&trackers).expect("serialization should not fail");
    assert_eq!(text, again, "same input must produce identical bytes");
}

#[rustfmt::skip]
#[test]
fn bug_task_display_name_prefers_display_name_then_target_link_then_self_link() {
    // Sparse task fixture (absent optional fields deserialise as `None`):
    // the display name falls back display_name -> target_link -> self_link
    // -> a stable placeholder.
    let mut task: bugs::BugTask = serde_json::from_value(serde_json::json!({
        "self_link": "https://api.launchpad.net/devel/ubuntu/+source/curl/+bug/42",
        "target_link": "https://api.launchpad.net/devel/ubuntu/+source/curl",
        "bug_target_display_name": "curl (Ubuntu)",
    })).expect("task fixture must deserialise");
    assert_eq!(bug_task_display_name(&task), "curl (Ubuntu)");
    task.bug_target_display_name = None;
    assert_eq!(bug_task_display_name(&task), "https://api.launchpad.net/devel/ubuntu/+source/curl", "falls back to target_link");
    task.target_link = None;
    assert_eq!(bug_task_display_name(&task), "https://api.launchpad.net/devel/ubuntu/+source/curl/+bug/42", "falls back to self_link");
    task.self_link = None;
    assert_eq!(bug_task_display_name(&task), "unknown task", "all-absent fields fall back to a stable placeholder");
}

#[rustfmt::skip]
#[test]
fn cli_bug_tracker_create_parses_options() {
    // Without --type parsing succeeds and leaves the type unset (inference
    // happens in validate_cli/resolve_tracker_create); the optional metadata
    // parses as given (whitespace values keep explicit argv); required
    // arguments and unknown values fail at parse time.
    let parse = |argv: &str| Cli::try_parse_from(argv.split_ascii_whitespace()).expect("CLI parsing failed");
    let Command::Bug(BugCommand::Tracker { command: TrackerCommand::Create { base_url, tracker_type, name, title, summary } }) = parse("lpcli bug tracker create --base-url https://github.com/canonical/apport/issues").command else { panic!("Expected Tracker Create") };
    assert_eq!(base_url, "https://github.com/canonical/apport/issues");
    assert!(tracker_type.is_none() && name.is_none() && title.is_none() && summary.is_none());
    let cli = Cli::try_parse_from([
        "lpcli", "bug", "tracker", "create", "--base-url", "https://bugs.debian.org/",
        "--type", "debbugs", "--name", "debbugs", "--title", "Debian Bug tracker",
        "--summary", "Debian's bug tracking system",
    ]).expect("CLI parsing failed");
    let Command::Bug(BugCommand::Tracker { command: TrackerCommand::Create { base_url, tracker_type, name, title, summary } }) = cli.command else { panic!("Expected Tracker Create") };
    assert_eq!(base_url, "https://bugs.debian.org/");
    assert_eq!(tracker_type, Some(Debbugs));
    assert_eq!(
        (name.as_deref(), title.as_deref(), summary.as_deref()),
        (Some("debbugs"), Some("Debian Bug tracker"), Some("Debian's bug tracking system"))
    );
    for argv in [
        "lpcli bug tracker create",
        "lpcli bug tracker",
        "lpcli bug tracker create --base-url https://example.org/issues --type google-plus",
    ] {
        assert!(Cli::try_parse_from(argv.split_ascii_whitespace()).is_err(), "'{argv}' must be rejected at parse time");
    }
}

#[rustfmt::skip]
#[test]
fn cli_bug_tracker_create_type_aliases_map_to_launchpad_strings() {
    // Every CLI alias parses to the expected enum variant whose
    // launchpad_name is the literal human string ensureBugTracker expects;
    // GitHub shares the library constant. The argv prefix is shared; only
    // the alias rows vary.
    let argv_prefix = ["lpcli", "bug", "tracker", "create", "--base-url", "https://example.org/issues", "--type"];
    let cases = [
        ("github", Github, "GitHub Issues"),
        ("gh", Github, "GitHub Issues"),
        ("gitlab", Gitlab, "GitLab Issues"),
        ("gl", Gitlab, "GitLab Issues"),
        ("bugzilla", TrackerType::Bugzilla, "Bugzilla"),
        ("debbugs", Debbugs, "Debbugs"),
        ("trac", TrackerType::Trac, "Trac"),
        ("google-code", GoogleCode, "Google Code"),
        ("googlecode", GoogleCode, "Google Code"),
        ("roundup", TrackerType::Roundup, "Roundup"),
        ("sourceforge", TrackerType::Sourceforge, "SourceForge or SourceForge derivative"),
        ("mantis", TrackerType::Mantis, "Mantis"),
        ("rt", TrackerType::Rt, "Request Tracker (RT)"),
        ("request-tracker", TrackerType::Rt, "Request Tracker (RT)"),
        ("savane", TrackerType::Savane, "Savane"),
        ("php-project", TrackerType::PhpProject, "PHP Project Bugtracker"),
        ("phpproject", TrackerType::PhpProject, "PHP Project Bugtracker"),
        ("email", TrackerType::Email, "Email Address"),
    ];
    for (value, expected, launchpad_name) in cases {
        let cli = Cli::try_parse_from(argv_prefix.iter().copied().chain(std::iter::once(value)))
            .unwrap_or_else(|e| panic!("--type {value} must parse: {e}"));
        let Command::Bug(BugCommand::Tracker { command: TrackerCommand::Create { tracker_type, .. } }) = cli.command else { panic!("Expected Tracker Create for --type {value}") };
        assert_eq!(tracker_type, Some(expected), "--type {value}");
        assert_eq!(tracker_type.unwrap().launchpad_name(), launchpad_name, "--type {value}");
    }
    assert_eq!(TrackerType::Github.launchpad_name(), bugs::BUG_TRACKER_TYPE_GITHUB);
}

#[rustfmt::skip]
#[test]
fn resolve_tracker_create_inference_and_name_rules() {
    // A canonical GitHub issues URL (owner/repo case-insensitive) infers
    // "GitHub Issues" with no name — Launchpad owns the auto-github- name;
    // non-GitHub URLs use the requested type; the name is only sent when
    // given.
    let debian = "https://bugs.debian.org/";
    for (base_url, tracker_type, name, expected) in [
        ("https://github.com/canonical/apport/issues", None, None, ("GitHub Issues", None)),
        ("https://github.com/Canonical/Apport/issues", None, None, ("GitHub Issues", None)),
        (debian, Some(Debbugs), Some("debbugs"), ("Debbugs", Some("debbugs"))),
        (debian, Some(Debbugs), None, ("Debbugs", None)),
    ] {
        let resolved = resolve_tracker_create(base_url, tracker_type, name)
            .unwrap_or_else(|e| panic!("{base_url} with {tracker_type:?} must resolve: {e}"));
        assert_eq!((resolved.bug_tracker_type, resolved.name.as_deref()), expected);
        assert_eq!(resolved.base_url, base_url);
    }
    let message = resolve_tracker_create(debian, None, None)
        .expect_err("non-GitHub base URLs require an explicit --type");
    assert!(
        message.contains("cannot infer --type") && message.contains("bugs.debian.org"),
        "error must name the cause and the offending URL: {message}"
    );
    for name in ["", "   ", "two words", "slash/name"] {
        assert!(
            resolve_tracker_create(debian, Some(Debbugs), Some(name)).is_err(),
            "tracker name '{name}' must be rejected"
        );
    }
    // The same rules run through validate_cli, i.e. before authentication
    // or any network access. (The invalid-name argv keeps its explicit
    // form: the value contains whitespace.)
    for (command, expected_ok, fragment) in [
        ("lpcli bug tracker create --base-url https://github.com/canonical/apport/issues", true, ""),
        ("lpcli bug tracker create --base-url https://gitlab.gnome.org/GNOME/mutter/issues --type gitlab", true, ""),
        ("lpcli bug tracker create --base-url https://bugs.debian.org/", false, "cannot infer --type"),
        ("lpcli bug tracker create --base-url https://github.com/canonical/apport --type github", false, "canonical GitHub repository issues base URL"),
        ("lpcli bug tracker create --base-url https://github.com/ --type google-code", false, "github-issues"),
    ] {
        let cli = Cli::try_parse_from(command.split_ascii_whitespace()).expect("the configuration must parse");
        let result = validate_cli(&cli);
        assert_eq!(result.is_ok(), expected_ok, "tracker create '{command}'");
        if let Err(error) = result {
            assert!(error.to_string().contains(fragment), "unexpected error: {error}");
        }
    }
    let cli = Cli::try_parse_from([
        "lpcli", "bug", "tracker", "create", "--base-url", "https://bugs.debian.org/",
        "--type", "debbugs", "--name", "two words",
    ]).expect("the configuration must parse");
    assert!(validate_cli(&cli).is_err(), "invalid names must fail validation");
}

#[rustfmt::skip]
#[test]
fn resolve_tracker_create_github_host_rules() {
    // --type github requires the canonical issues form; another type on a
    // github.com URL recreates the 'github-issues' (Google Code) trap; both
    // paths refuse non-canonical github.com URLs, before any network.
    for base_url in [
        "https://github.com/canonical/apport",
        "https://github.com/canonical/apport/pulls",
        "https://github.com/canonical/apport/issues/658",
        "https://github.com/canonical/apport/issues/extra",
    ] {
        let message = resolve_tracker_create(base_url, Some(Github), None)
            .expect_err("--type github must require the canonical /issues form");
        assert!(
            message.contains("canonical GitHub repository issues base URL"),
            "error must document the required shape for '{base_url}': {message}"
        );
    }
    let message = resolve_tracker_create("https://github.com/", Some(GoogleCode), None)
        .expect_err("github.com base URL must use --type github");
    assert!(message.contains("github-issues"), "must mention the trap: {message}");
    for base_url in [
        "http://github.com/canonical/apport/issues",
        "https://user@github.com/canonical/apport/issues",
        "https://user:pass@github.com/canonical/apport/issues",
        "https://github.com:8443/canonical/apport/issues",
        "https://github.com:443/canonical/apport/issues",
        "https://github.com:/canonical/apport/issues",
        "https:github.com/canonical/apport/issues",
        "https://github.com//canonical/apport/issues",
    ] {
        let inferred = resolve_tracker_create(base_url, None, None)
            .expect_err("inference must refuse a non-canonical GitHub URL");
        assert!(inferred.contains("cannot infer --type"), "for '{base_url}': {inferred}");
        let explicit = resolve_tracker_create(base_url, Some(Github), None)
            .expect_err("--type github must refuse a non-canonical GitHub URL");
        assert!(
            explicit.contains("canonical GitHub repository issues base URL"),
            "for '{base_url}': {explicit}"
        );
    }
}

#[rustfmt::skip]
#[test]
fn tracker_create_base_url_rejections_and_self_hosted_ports() {
    // Malformed base URLs (including embedded credentials) are rejected by
    // the clap value parser, before authentication or any network access.
    for base_url in [
        "not-a-url",
        "github.com/canonical/apport/issues",
        "ftp://bugs.debian.org/",
        "https://github.com/canonical/apport/issues?shard=1",
        "https://bugs.debian.org/#fragment",
        "",
        "https://user:pass@bugs.example.org/trac/proj",
        "https://user@bugs.example.org/trac/proj",
        "https://:pass@bugs.example.org/trac/proj",
    ] {
        let argv = format!("lpcli bug tracker create --base-url {base_url}");
        assert!(Cli::try_parse_from(argv.split_ascii_whitespace()).is_err(), "--base-url '{base_url}' must be rejected at parse time");
    }
    // Custom ports are legitimate for self-hosted trackers (only github.com
    // URLs are restricted to the canonical shape) and keep their exact value.
    for base_url in ["https://bugs.example.com:8443/trac/proj", "http://localhost:8080/bugs/"] {
        assert_eq!(parse_tracker_base_url(base_url).ok().as_deref(), Some(base_url), "self-hosted '{base_url}' must stay accepted unchanged");
        let argv = format!("lpcli bug tracker create --base-url {base_url}");
        assert!(Cli::try_parse_from(argv.split_ascii_whitespace()).is_ok(), "self-hosted '{base_url}' must parse for tracker create");
    }
    // Non-canonical github.com URLs parse but must fail validate_cli for
    // both inference and explicit --type github (custom ports and repeated
    // path segments included).
    for extra in ["", "--type github"] {
        for base_url in [
            "https://github.com:8443/canonical/apport/issues",
            "https://github.com//canonical/apport/issues",
            "http://github.com/canonical/apport/issues",
        ] {
            let argv = format!("lpcli bug tracker create --base-url {base_url} {extra}");
            let cli = Cli::try_parse_from(argv.split_ascii_whitespace()).expect("the URL shape must still parse");
            assert!(validate_cli(&cli).is_err(), "tracker create with '{base_url}' (extra '{extra}') must fail validation");
        }
    }
}

#[rustfmt::skip]
#[test]
fn format_ensure_tracker_result_prints_returned_identity_and_state() {
    // The printed identity comes from the returned entry, not the request,
    // and the created/reused state is visible.
    let tracker = bugs::BugTracker {
        name: Some("auto-github-canonical-apport".to_string()),
        bug_tracker_type: Some("GitLab Issues".to_string()),
        base_url: Some("https://gitlab.gnome.org/GNOME/mutter/issues".to_string()),
        ..Default::default()
    };
    let joined = format_ensure_tracker_result(&tracker, false).join("\n");
    assert!(
        joined.contains("auto-github-canonical-apport")
            && joined.contains("reused (already existed)")
            && joined.contains("GitLab Issues")
            && joined.contains("https://gitlab.gnome.org/GNOME/mutter/issues"),
        "the returned identity and state must be printed: {joined}"
    );
    assert!(
        format_ensure_tracker_result(&tracker, true).join("\n").contains("created"),
        "a newly registered tracker must be reported as created"
    );
}

#[rustfmt::skip]
#[test]
fn format_ensure_tracker_result_survives_missing_fields() {
    let lines = format_ensure_tracker_result(&bugs::BugTracker::default(), true);
    assert!(lines.join("\n").contains("(unknown name)"), "a nameless returned tracker must still produce readable output");
}

#[rustfmt::skip]
#[test]
fn watch_url_line_prints_url_and_stays_visible_when_missing() {
    // The final watch URL is printed unconditionally for both link forms: a
    // missing URL must stay visible rather than be silently skipped.
    let watch = bugs::BugWatch { url: Some("https://github.com/canonical/apport/issues/658".to_string()), ..Default::default() };
    assert_eq!(watch_url_line(&watch), "  URL: https://github.com/canonical/apport/issues/658");
    assert!(watch_url_line(&bugs::BugWatch::default()).contains("no watch URL"), "a missing watch URL must stay visible");
}

#[rustfmt::skip]
#[test]
fn watch_linked_tasks_cell_visibility() {
    // Tasks are grouped by the final path segment of bug_watch_link (so a
    // beta-namespace link still matches); unlinked tasks are ignored; names
    // are joined; unknown-id watches are orphans (also with no tasks); a
    // watch without a usable ID is "unknown".
    let task = |display: Option<&str>, watch_link: Option<&str>| {
        let mut t = make_test_task(
            "https://api.launchpad.net/devel/ubuntu/noble/+source/curl/+bug/42",
            "https://api.launchpad.net/devel/ubuntu/noble/+source/curl",
            "curl (Ubuntu Noble)",
            watch_link,
        );
        if let Some(display) = display {
            t.bug_target_display_name = Some(display.to_string());
        }
        t
    };
    let links = collect_watch_task_links(&[
        task(None, Some(WATCH_LINKED_URL)),
        task(Some("curl (via beta)"), Some("https://api.launchpad.net/beta/bugs/2174521/+watch/167246")),
        task(None, None),
    ]);
    assert_eq!(links.len(), 1, "tasks without a watch link are ignored");
    assert_eq!(links["167246"].len(), 2);
    let cell = watch_linked_tasks_cell(Some("167246"), &links);
    assert!(cell.contains("curl (Ubuntu Noble)") && cell.contains("curl (via beta)"), "linked names listed: {cell}");
    assert!(cell.contains(", "), "multiple tasks are joined with ', '");
    assert_eq!(watch_linked_tasks_cell(Some("167245"), &links), "orphan");
    assert_eq!(watch_linked_tasks_cell(Some("167245"), &collect_watch_task_links(&[])), "orphan");
    assert_eq!(watch_linked_tasks_cell(None, &links), "unknown");
}

#[rustfmt::skip]
#[test]
fn cli_bug_watches_and_tasks_short_bug_id_flags() {
    // -b 2174521 (space-separated) for watches and -b2174521 (attached
    // value) for tasks parse identically to the long --bug-id form.
    let Command::Bug(BugCommand::Watches { bug_id }) = Cli::try_parse_from("lpcli bug watches -b 2174521".split_ascii_whitespace()).expect("CLI parsing failed").command else { panic!("Expected Watches") };
    assert_eq!(bug_id, 2174521);
    let Command::Bug(BugCommand::Tasks { bug_id }) = Cli::try_parse_from("lpcli bug tasks -b2174521".split_ascii_whitespace()).expect("CLI parsing failed").command else { panic!("Expected Tasks") };
    assert_eq!(bug_id, 2174521);
}

#[rustfmt::skip]
#[test]
fn find_task_to_link_without_selector_auto_links_single_task() {
    // Current intended selection semantics for BOTH link forms: with no
    // selector at all, a single remaining task is auto-selected; multiple
    // candidates are refused rather than guessed between. (Sparse task
    // fixture: absent optional fields deserialise as `None`.)
    let task = |display: &str, target: &str| -> bugs::BugTask {
        serde_json::from_value(serde_json::json!({
            "self_link": format!("https://api.launchpad.net/devel{target}/+bug/2174521"),
            "bug_link": "https://api.launchpad.net/devel/bugs/2174521",
            "bug_target_display_name": display,
            "target_link": format!("https://api.launchpad.net/devel{target}"),
        }))
        .expect("task fixture must deserialise")
    };
    let single = vec![task("apport (Ubuntu)", "/ubuntu/+source/apport")];
    let found = find_task_to_link(&single, 2174521, &TaskLinkSelector::default())
        .expect("a single task must be auto-selected when no selector is given");
    assert_eq!(found.bug_target_display_name.as_deref(), Some("apport (Ubuntu)"));
    let tasks = vec![
        task("apport (Ubuntu)", "/ubuntu/+source/apport"),
        task("apport (Debian)", "/debian/+source/apport"),
    ];
    match find_task_to_link(&tasks, 2174521, &TaskLinkSelector::default()) {
        Err(lpcli::error::LpError::Other(msg)) => {
            assert!(msg.contains("Multiple matching tasks"), "unexpected message: {msg}");
        }
        other => panic!("Expected the ambiguous-selection error, got {other:?}"),
    }
    // Regression test: a sole series-less task must not be guessed when no
    // selector was given (here Debian would previously have been chosen).
    let mixed = vec![
        task("apport (Debian)", "/debian/+source/apport"),
        task("apport (Ubuntu Noble)", "/ubuntu/noble/+source/apport"),
    ];
    match find_task_to_link(&mixed, 2174521, &TaskLinkSelector::default()) {
        Err(lpcli::error::LpError::Other(msg)) => {
            for expected in ["Multiple matching tasks", "apport (Debian)", "apport (Ubuntu Noble)", "No task was selected or modified", "--task-url"] {
                assert!(msg.contains(expected), "missing '{expected}' in: {msg}");
            }
        }
        other => panic!("Expected the ambiguous-selection error, got {other:?}"),
    }
    // An explicit target keeps the documented series-less preference.
    let selected = TaskLinkSelector { target: Some("apport".to_string()), ..Default::default() };
    let found = find_task_to_link(&mixed, 2174521, &selected).expect("the target narrows the candidates");
    assert_eq!(found.bug_target_display_name.as_deref(), Some("apport (Debian)"));
}

// -----------------------------------------------------------------------
// bug set-description
// -----------------------------------------------------------------------

/// `bug set-description --description` parses the bug id and description text.
#[test]
fn cli_bug_set_description_accepts_description_text() {
    let cli = Cli::try_parse_from([
        "lpcli",
        "bug",
        "set-description",
        "--bug-id",
        "2174396",
        "--description",
        "Crash when unlocking the session.",
    ])
    .unwrap();
    match cli.command {
        Command::Bug(BugCommand::SetDescription {
            bug_id,
            description,
            description_file,
        }) => {
            assert_eq!(bug_id, 2174396);
            assert_eq!(
                description.as_deref(),
                Some("Crash when unlocking the session.")
            );
            assert!(description_file.is_none());
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

/// `bug set-description --description-file` parses the file path.
#[test]
fn cli_bug_set_description_accepts_description_file() {
    let cli = Cli::try_parse_from([
        "lpcli",
        "bug",
        "set-description",
        "-b",
        "42",
        "--description-file",
        "/tmp/description.txt",
    ])
    .unwrap();
    match cli.command {
        Command::Bug(BugCommand::SetDescription {
            bug_id,
            description,
            description_file,
        }) => {
            assert_eq!(bug_id, 42);
            assert!(description.is_none());
            assert_eq!(description_file.as_deref(), Some("/tmp/description.txt"));
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

/// `-` is accepted as the `--description-file` value (meaning stdin).
#[test]
fn cli_bug_set_description_accepts_stdin_marker() {
    let cli = Cli::try_parse_from([
        "lpcli",
        "bug",
        "set-description",
        "-b",
        "42",
        "--description-file",
        "-",
    ])
    .unwrap();
    match cli.command {
        Command::Bug(BugCommand::SetDescription {
            description_file, ..
        }) => {
            assert_eq!(description_file.as_deref(), Some("-"));
        }
        other => panic!("unexpected command: {other:?}"),
    }
}

/// Omitting both `--description` and `--description-file` is an error.
#[test]
fn cli_bug_set_description_requires_description_or_file() {
    let err =
        Cli::try_parse_from(["lpcli", "bug", "set-description", "--bug-id", "42"]).unwrap_err();
    assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
}

/// Passing both `--description` and `--description-file` is an error.
#[test]
fn cli_bug_set_description_rejects_description_and_file_together() {
    let err = Cli::try_parse_from([
        "lpcli",
        "bug",
        "set-description",
        "--bug-id",
        "42",
        "--description",
        "text",
        "--description-file",
        "/tmp/description.txt",
    ])
    .unwrap_err();
    assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
}

/// `read_description_file` reads the file contents verbatim, including
/// newlines and unicode text.
#[test]
fn read_description_file_reads_file_contents() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("description.txt");
    std::fs::write(&path, "line one\nline two ✓\n").unwrap();

    let text =
        read_description_file(path.to_str().unwrap()).expect("reading the file should succeed");
    assert_eq!(text, "line one\nline two ✓\n");
}

/// `read_description_file` reports a helpful error naming the path for a
/// missing file (mirroring the attachment-reader error pattern).
#[test]
fn read_description_file_missing_file_is_an_error() {
    let err = read_description_file("/nonexistent/description.txt").unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("Failed to read description file '/nonexistent/description.txt'"),
        "msg: {msg}"
    );
}

/// `resolve_new_description` returns the text given via `--description`.
#[test]
fn resolve_new_description_accepts_text() {
    let text = resolve_new_description(Some("Updated description.".into()), None)
        .expect("text should be accepted");
    assert_eq!(text, "Updated description.");
}

/// `resolve_new_description` reads the file given via `--description-file`.
#[test]
fn resolve_new_description_reads_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("description.txt");
    std::fs::write(&path, "description from a file").unwrap();

    let text = resolve_new_description(None, Some(path.to_str().unwrap()))
        .expect("file input should be accepted");
    assert_eq!(text, "description from a file");
}

/// `resolve_new_description` rejects an empty `--description` value.
#[test]
fn resolve_new_description_rejects_empty_text() {
    let err = resolve_new_description(Some(String::new()), None).unwrap_err();
    assert!(err.to_string().contains("must not be empty"), "msg: {err}");
}

/// `resolve_new_description` rejects a whitespace-only `--description` value.
#[test]
fn resolve_new_description_rejects_whitespace_only_text() {
    let err = resolve_new_description(Some("  \n\t".into()), None).unwrap_err();
    assert!(err.to_string().contains("must not be empty"), "msg: {err}");
}

/// `resolve_new_description` rejects an empty description file, so an empty
/// pipe or file cannot silently wipe the current description.
#[test]
fn resolve_new_description_rejects_empty_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty.txt");
    std::fs::write(&path, "").unwrap();

    let err = resolve_new_description(None, Some(path.to_str().unwrap())).unwrap_err();
    assert!(err.to_string().contains("must not be empty"), "msg: {err}");
}

/// `read_description_from_reader` reads stdin-style input verbatim.
#[test]
fn read_description_from_reader_reads_input() {
    use std::io::Cursor;

    let text = read_description_from_reader(Cursor::new(b"piped description\n".to_vec()))
        .expect("reading should succeed");
    assert_eq!(text, "piped description\n");
}

/// `read_description_from_reader` surfaces non-UTF-8 input as an I/O error.
#[test]
fn read_description_from_reader_rejects_non_utf8() {
    use std::io::Cursor;

    let err = read_description_from_reader(Cursor::new(vec![0xff, 0xfe, 0x41]))
        .expect_err("non-UTF-8 input should be an error");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

// skills command (embedded agent skill documentation)
// -----------------------------------------------------------------------

/// Both root help forms surface the embedded agent skills command.
#[test]
fn root_help_lists_agent_skills_command() {
    for args in [vec!["lpcli", "--help"], vec!["lpcli", "help"]] {
        let err = Cli::try_parse_from(&args).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::DisplayHelp, "{args:?}");
        let help = err
            .to_string()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        assert!(help.contains("skills"), "{args:?}: {help}");
        assert!(
            help.contains("Agent Skills (coding agents should read this first)"),
            "{args:?}: {help}"
        );
    }
}
