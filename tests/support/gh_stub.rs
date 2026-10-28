//! Standalone, std-only fake gh, built by Cargo for the integration harness.
use std::{env, fs, io::Write, path::PathBuf};

fn var(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.into())
}
fn yes(key: &str) -> bool {
    var(key, "0") == "1"
}
fn fail(message: &str, code: i32) -> ! {
    eprintln!("{message}");
    std::process::exit(code)
}
fn quote(value: &str) -> String {
    let mut out = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}
fn append(path: &str, value: &str) {
    writeln!(
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap(),
        "{value}"
    )
    .unwrap();
}
fn count(path: &str) -> u64 {
    let n = fs::read_to_string(path)
        .unwrap_or_default()
        .trim()
        .parse::<u64>()
        .unwrap_or(0)
        + 1;
    fs::write(path, n.to_string()).unwrap();
    n
}
fn option<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.windows(2)
        .find(|pair| pair[0] == name)
        .map(|pair| pair[1].as_str())
}
fn run(repo: &str, id: u64, branch: &str, status: &str, conclusion: &str, event: &str) -> String {
    format!(
        r#"{{"databaseId":{id},"displayTitle":"prepare-release","status":{},"conclusion":{},"url":"https://github.com/{repo}/actions/runs/{id}","headBranch":{},"event":{}}}"#,
        quote(status),
        quote(conclusion),
        quote(branch),
        quote(event)
    )
}
fn pr(repo: &str, number: &str, viewed: bool) -> String {
    let sha = var("STUB_MERGE_SHA", "");
    let merge = if sha.is_empty() {
        "null".into()
    } else {
        format!(r#"{{"oid":{}}}"#, quote(&sha))
    };
    let state = var("STUB_PR_STATE", "OPEN");
    let state = if viewed {
        var("STUB_VIEW_PR_STATE", &state)
    } else {
        state
    };
    format!(
        r#"{{"number":{number},"url":"https://github.com/{repo}/pull/7","title":"Release 1.0.0","body":{},"state":{},"isDraft":false,"mergeCommit":{merge}}}"#,
        var("STUB_PR_BODY_JSON", "\"\""),
        quote(&state)
    )
}
fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let arg = |i| args.get(i).map(String::as_str).unwrap_or("");
    let all = args.join(" ");
    let cmd = format!("{} {}", arg(0), arg(1));
    let log = var("STUB_LOG", "stub.log");
    append(&log, &all.replace('\n', " "));
    append(&format!("{log}.tokens"), &var("GH_TOKEN", ""));
    if all.contains("Authorization: Bearer ") {
        if yes("STUB_APP_REJECTS_JWT") {
            fail("gh: Bad credentials (HTTP 401)", 1)
        }
        if all.contains("access_tokens") {
            let n = count(&format!("{log}.mint"));
            println!(r#"{{"token":"ghs_stub_{n}","expires_at":"2099-01-01T00:00:00Z"}}"#);
        } else if all.contains("/installation") {
            if yes("STUB_APP_NOT_INSTALLED") {
                fail("gh: Not Found (HTTP 404)", 1)
            }
            println!(r#"{{"id":4242}}"#);
        } else {
            fail(&format!("gh stub: unexpected App call: {all}"), 127)
        }
        return;
    }
    let bad = var("STUB_BAD_TOKEN", "");
    if !bad.is_empty() && var("GH_TOKEN", "") == bad {
        fail("gh: Bad credentials (HTTP 401)", 1)
    }
    if yes("STUB_UNAUTHENTICATED") {
        fail(
            "gh: To get started with GitHub CLI, please run: gh auth login",
            4,
        )
    }
    if var("STUB_FLAKY_CMD", "") == cmd {
        let n = count(&format!("{log}.flaky"));
        if n <= var("STUB_FLAKY_COUNT", "0").parse().unwrap() {
            fail(
                "HTTP 504: We couldn't respond to your request in time. Sorry about that. Please try resubmitting your request and contact us if the problem persists. (https://api.github.com/graphql)",
                1,
            )
        }
    }
    let repo = var("STUB_REPO", "acme/widgets");
    let labels_file = format!("{log}.labels");
    let mut labels: Vec<String> = var("STUB_LABELS", "")
        .split(',')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    labels.extend(
        fs::read_to_string(&labels_file)
            .unwrap_or_default()
            .lines()
            .map(str::to_string),
    );
    match cmd.as_str() {
        "repo view" => {
            if args.iter().any(|a| a == "--repo") {
                fail("unknown flag: --repo", 1)
            }
            let repo = if !arg(2).is_empty() && !arg(2).starts_with('-') {
                arg(2)
            } else {
                &repo
            };
            println!(
                r#"{{"nameWithOwner":{},"defaultBranchRef":{{"name":{}}},"url":"https://github.com/{repo}"}}"#,
                quote(repo),
                quote(&var("STUB_DEFAULT_BRANCH", "main"))
            );
        }
        "workflow run" => {
            if yes("STUB_REQUIRES_SHIP_ID") && !args.iter().any(|a| a.starts_with("ship_id=")) {
                fail("required input 'ship_id' not provided", 1)
            }
            fs::write(
                format!("{log}.dispatchref"),
                option(&args, "--ref").unwrap_or(""),
            )
            .unwrap();
        }
        "workflow list" => println!("[]"),
        "run list" => {
            if !yes("STUB_RUN_FOUND") {
                println!("[]");
                return;
            }
            let branch = option(&args, "--branch").unwrap_or("");
            let dispatched = fs::read_to_string(format!("{log}.dispatchref")).unwrap_or_default();
            let existing = var("STUB_EXISTING_RUN_BRANCH", branch);
            let mut runs = Vec::new();
            if !dispatched.is_empty() && (branch.is_empty() || branch == dispatched) {
                runs.push(run(
                    &repo,
                    42,
                    &dispatched,
                    &var("STUB_RUN_STATUS", "completed"),
                    &var("STUB_RUN_CONCLUSION", "success"),
                    "workflow_dispatch",
                ));
            } else if yes("STUB_EXISTING_RUN") && (branch.is_empty() || branch == existing) {
                runs.push(run(
                    &repo,
                    41,
                    &existing,
                    &var("STUB_EXISTING_RUN_STATUS", "completed"),
                    &var("STUB_EXISTING_RUN_CONCLUSION", "success"),
                    "workflow_dispatch",
                ));
            }
            if yes("STUB_PUSH_RUN") {
                runs.push(run(&repo, 99, branch, "completed", "success", "push"));
            }
            println!("[{}]", runs.join(","));
        }
        "run view" => println!(
            "{}",
            run(
                &repo,
                arg(2).parse().unwrap_or(42),
                "release/next",
                &var("STUB_RUN_STATUS", "completed"),
                &var("STUB_RUN_CONCLUSION", "success"),
                "workflow_dispatch"
            )
        ),
        "run download" => {
            let artifact = var("STUB_ARTIFACT", "");
            if artifact.is_empty() {
                fail("no valid artifacts found to download", 1)
            }
            let dir = PathBuf::from(option(&args, "--dir").unwrap_or("."));
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join(var("STUB_ARTIFACT_NAME", "ship.release.json")),
                artifact,
            )
            .unwrap();
        }
        "pr list" => println!(
            "[{}]",
            if yes("STUB_PR_EXISTS") {
                pr(&repo, "7", false)
            } else {
                String::new()
            }
        ),
        "pr view" => println!("{}", pr(&repo, arg(2), true)),
        "label list" => println!(
            "[{}]",
            labels
                .iter()
                .map(|label| format!(r#"{{"name":{}}}"#, quote(label)))
                .collect::<Vec<_>>()
                .join(",")
        ),
        "label create" => {
            if yes("STUB_LABEL_CREATE_FAILS") {
                fail("HTTP 403: Resource not accessible by integration", 1)
            }
            append(&labels_file, arg(2));
        }
        "pr create" => {
            if yes("STUB_PR_STRICT_LABELS") {
                for pair in args.windows(2).filter(|p| p[0] == "--label") {
                    if !labels.contains(&pair[1]) {
                        fail(&format!("could not add label: '{}' not found", pair[1]), 1)
                    }
                }
            }
            println!("https://github.com/{repo}/pull/7");
        }
        "pr edit" | "pr merge" | "pr reopen" | "pr close" | "release edit" | "release upload" => {}
        "release view" => {
            if !yes("STUB_RELEASE_EXISTS") {
                fail("release not found", 1)
            }
            println!(
                r#"{{"tagName":"v1.0.0","isDraft":{}}}"#,
                var("STUB_RELEASE_DRAFT", "false")
            );
        }
        "release create" => println!("https://github.com/{repo}/releases/tag/v1.0.0"),
        _ if arg(0) == "api" => {
            if args.iter().any(|a| a == "--repo") {
                fail("unknown flag: --repo", 1)
            }
            let target = arg(1);
            if target.contains("/pulls/") {
                println!("{}", var("STUB_PR_IDENTITY", "null"));
            } else if target.contains("/branches/") {
                if !yes("STUB_BRANCH_EXISTS") {
                    fail("HTTP 404: Not Found", 1)
                }
            } else if target.contains("/git/ref/") {
                println!("a1b2c3d4e5f6");
            } else if target.contains("/git/commits") {
                println!(
                    r#"{{"sha":"51c8ed0000000000000000000000000000000000","message":"chore(release): 1.0.0","tree":"77ee110000000000000000000000000000000000","parents":["9a5e110000000000000000000000000000000000"],"verified":{}}}"#,
                    var("STUB_SIGN_VERIFIED", "true")
                );
            } else if target.contains("/commits/tags/") {
                println!(
                    r#"{{"sha":{}}}"#,
                    quote(&var(
                        "STUB_TAG_SHA",
                        &var("STUB_MERGE_SHA", "abc1234def5678")
                    ))
                );
            } else if target.contains("/commits/") {
                println!(
                    r#"{{"sha":"c0ffee0000000000000000000000000000000000","message":"chore(release): 1.0.0","tree":"77ee110000000000000000000000000000000000","parents":["9a5e110000000000000000000000000000000000"],"verified":{}}}"#,
                    var("STUB_HEAD_VERIFIED", "false")
                );
            } else if target.contains("/git/matching-refs/heads/ship/prepare-") {
                let stale = var("STUB_STALE_STAGING", "");
                if stale.is_empty() {
                    println!("[]");
                } else {
                    println!(r#"[{{"ref":{}}}]"#, quote(&format!("refs/heads/{stale}")));
                }
            } else if target.contains("/git/refs") {
                if all.contains("refs/tags/") {
                    let error = var("STUB_TAG_ERROR", "");
                    if !error.is_empty() {
                        fail(&error, 1)
                    }
                    if yes("STUB_TAG_EXISTS") {
                        fail("HTTP 422: Reference already exists", 1)
                    }
                }
                if yes("STUB_BRANCH_DELETE_FAILS") && option(&args, "--method") == Some("DELETE") {
                    fail("HTTP 403: Resource not accessible by integration", 1)
                }
            } else {
                println!("{{}}");
            }
        }
        _ => fail(&format!("gh stub: unexpected invocation: {all}"), 127),
    }
}
