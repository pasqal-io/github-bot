use std::collections::HashMap;

use anyhow::Context;
use derive_more::{AsRef, Display};
use lazy_regex::lazy_regex;
use log::{debug, info, warn};
use reqwest::Client;
use serde::{de::Unexpected, Deserialize};


/// All the machinery for a single project.
async fn per_project(
    client: &Client,
    secrets: &Secrets,
    project: &Project,
    config: &Config,
) -> Result<(), anyhow::Error> {
    let since = chrono::Local::now() - config.update_frequency;

    // Instantiate the slack hook.
    let slack_hooks = secrets
        .repo_to_hook
        .get(&project.url)
        .context("Missing secret")?;

    // List issues and pull requests.
    //
    // Note that the API could return more than one page, but we're not interested
    // in so many issues/PRs.
    let octocrab = octocrab::instance();
    let issues = octocrab
        .issues(&project.owner, &project.repo)
        .list()
        .since(since)
        .send()
        .await
        .context("Couldn't download recent issues")?;

    let requests = octocrab
        .pulls(&project.owner, &project.repo)
        .list()
        .state(State::Open)
        .send()
        .await
        .context("Couldn't download open pull requests")?;

    // We're only interested in pending requests (i.e. requests with
    // a pending review).
    let pending_requests: HashMap<_, _> = requests
        .into_iter()
        .filter_map(|pr| match pr.requested_reviewers {
                Some(ref reviewers) if reviewers.is_empty().not() => Some((*pr.id, pr)),
                _ => None
            })
        .collect();

    // ...and since requests are also issues, let's make sure that we
    // don't display them twice.
    let pending_issues = issues.into_iter()
        .filter(|issue| pending_requests.contains_key(&*issue.id).not())
        .collect_vec();

    if pending_issues.is_empty() && pending_requests.is_empty() {
        debug!("No issues to report");
        return Ok(());
    }

    if pending_requests.is_empty().not() {
        let title = format!(
            "PRs of repo {link} waiting for reviews",
            link = slack::link(&project.url, Some(project.repo.as_ref())),
        );
        let mut msg = slack::Section::new(title);
        msg.append_fields(&["*Request*".to_string(), "*Reviewer*".to_string()]);
        for pull in pending_requests.into_values() {
            let Some(reviewers) = pull.requested_reviewers
            else {
                panic!("Inconsistency: we just checked that reviewers as not-None")
            };
            let Some(url) = pull.html_url
            else {
                error!("In project {}, PR {} missing a URL, skipping", project.url, pull.id);
                continue
            };
            let Some(title) = pull.title
            else {
                error!("In project {}, PR {} missing a title, skipping", project.url, pull.id);
                continue
            };
            let reviewers = format!("{}", reviewers.into_iter().map(|reviewer| reviewer.login).format(", "));
            msg.append_fields(&[
                slack::link(&url, Some(title.as_str())),
                reviewers
            ])
        }    
        for hook in slack_hooks {
            msg.send(client, &hook.0)
                .await
                .context("Failed to post udpdate on Slack")?;
        }
    }
    if pending_issues.is_empty().not() {
        let title = format!(
            "Issues of repo {link} updated since {since}",
            link = slack::link(&project.url, Some(project.repo.as_ref())),
            since = since.format("%d/%m/%Y %H:%M"),
        );
        let mut msg = slack::Section::new(title);
        msg.append_fields(&["*Issue*".to_string(), "*Updater*".to_string()]);
        for issue in pending_issues.into_iter() {
            msg.append_fields(&[
                slack::link(&issue.html_url, Some(issue.title.as_str())),
                format!(
                    "{} on {}",
                    issue.user.login,
                    issue.updated_at.format("%d/%m/%Y %H:%M")
                ),
            ])
        }    
        for hook in slack_hooks {
            msg.send(client, hook.as_ref()
                .await
                .context("Failed to post udpdate on Slack")?;
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    env_logger::init();
    let _ = dotenv::dotenv(); // If there's no .env, let's not load one!

    // Load secrets.
    info!("Loading secrets");
    // Source 1: Big variable `QASTOR_SECRETS`.
    let env_secrets = std::env::var("QASTOR_SECRETS").unwrap_or_else(|_| "{}".to_string());
    let mut secrets: Secrets =
        serde_json::from_str(&env_secrets).context("Invalid env QASTOR_SECRETS")?;

    // Source 2: any variable `QASTOR_HOOK.*` can contain a mapping 
    for (key, value) in std::env::vars() {
        if key.starts_with("QASTOR_HOOK") {
            let project_to_hook = ProjectToHook::from_env_var(&value)
                .with_context(|| format!("Invalid env variable {key}:{value}"))?;
            secrets.repo_to_hook.entry(project_to_hook.project)
                .or_default()
                .push(project_to_hook.hook);
        }
    }

    // Load config.
    info!("Loading config");
    let file_config = std::fs::File::open("config.yml").context("Could not open config.yml")?;
    let config: Config = serde_yaml::from_reader(file_config).context("Invalid config.yml")?;

    let client = reqwest::Client::new();

    for project in &config.projects {
        info!("Checking project {}", project.url);
        if let Err(err) = per_project(&client, &secrets, project, &config).await {
            warn!(
                "Error handling project {}/{}: {:?}",
                project.owner, project.repo, err
            )
        }
    }
    info!("Done");
    Ok(())
}
