use std::{collections::HashMap, ops::Not};

use anyhow::Context;
use derive_more::{AsRef, Display};
use itertools::Itertools;
use lazy_regex::lazy_regex;
use log::{debug, error, info, warn};
use octocrab::params::State;
use reqwest::Client;
use serde::{de::Unexpected, Deserialize};

mod slack;

use url::Url;

/// The name of a repository.
#[derive(Hash, PartialEq, Eq, Debug, Deserialize, Display, AsRef)]
struct RepoName(String);
impl From<&RepoName> for String {
    fn from(repo_name: &RepoName) -> String {
        repo_name.0.clone()
    }
}

/// A capability to post messages in one Slack room.
///
/// Typically looks like https://hooks.slack.com/services/XXX/YYY/ZZZ
///
/// Confidentiality: secret.
#[derive(Deserialize)]
struct SlackHook(Url);

/// All the secrets we rely upon.
///
/// Typically an environment variable QASTOR_SECRETS, containing a JSON string.
#[derive(Deserialize)]
struct Secrets {
    #[serde(flatten)]
    repo_to_hook: HashMap<Url, Vec<SlackHook>>,
}

/// Configuration of a single project.
struct Project {
    /// Full url for the project. Used for display only.
    url: Url,

    /// Owner (user or org) of the repository. Used for fetching issues.
    owner: String,

    /// Name (user or org) of the repository. Used for fetching issues.
    repo: RepoName,
}

impl<'de> Deserialize<'de> for Project {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de> {
        use serde::de::Error;
        #[derive(Deserialize)]
        struct Payload {
            url: Url
        }
        let payload: Payload = Payload::deserialize(deserializer)?;
        let Some(mut segments) = payload.url.path_segments()
        else {
            return Err(D::Error::invalid_value(Unexpected::Str(payload.url.as_str()), &"a url https://github.com/<owner>/<project> (missing path)"))
        };
        let Some(owner) = segments.next()
        else {
            return Err(D::Error::invalid_value(Unexpected::Str(payload.url.as_str()), &"a url https://github.com/<owner>/<project> (missing owner)"))
        };
        let Some(project) = segments.next()
        else {
            return Err(D::Error::invalid_value(Unexpected::Str(payload.url.as_str()), &"a url https://github.com/<owner>/<project> (missing project)"))
        };
        let owner = owner.to_string();
        let repo = RepoName(project.to_string());
        Ok(Project {
            url: payload.url,
            owner,
            repo
        })
    }
}

/// The configuration for qastor.
#[derive(Deserialize)]
struct Config {
    /// The projects to monitor.
    #[serde(default)]
    projects: Vec<Project>,

    /// How often we're expecting to monitor the projects, as a number followed by a unit d/h/m/s.
    ///
    /// This variable only affects how far back we're looking in time for changes in issues.
    #[serde(
        deserialize_with = "Config::deserialize_update_frequency",
        default = "Config::default_update_frequency"
    )]
    update_frequency: chrono::Duration,
}
impl Config {
    fn deserialize_update_frequency<'de, D>(deserializer: D) -> Result<chrono::Duration, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;
        let source = String::deserialize(deserializer)?;
        let regex = lazy_regex!("([[:digit:]]+) *([hmsd])");
        let found = regex.captures(&source).ok_or_else(|| {
            D::Error::invalid_value(
                Unexpected::Str(&source),
                &"numbers followed by a unit d/h/m/s",
            )
        })?;
        let digits = found.get(1).expect("we should have digits");
        let unit = found.get(2).expect("we should have a unit");
        let digits: i64 = digits
            .as_str()
            .parse()
            .map_err(|_| D::Error::invalid_value(Unexpected::Str(digits.as_str()), &"numbers"))?;
        let unit: char = unit.as_str().parse().map_err(|_| {
            D::Error::invalid_value(Unexpected::Str(unit.as_str()), &"a unit d/h/m/s")
        })?;
        let result = match unit {
            'd' => chrono::Duration::days(digits),
            'h' => chrono::Duration::hours(digits),
            'm' => chrono::Duration::minutes(digits),
            's' => chrono::Duration::seconds(digits),
            _ => unreachable!(),
        };
        Ok(result)
    }

    fn default_update_frequency() -> chrono::Duration {
        chrono::Duration::hours(2)
    }
}

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
            msg.send(client, &hook.0)
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
    let secret_re = lazy_regex!{"(.*)=(.*)"};
    for (key, value) in std::env::vars() {
        if key.starts_with("QASTOR_HOOK") {
            let Some(captures) = secret_re.captures(&value)
            else {
                warn!("Invalid env variable {key}:{value} -- skipping");
                continue;
            };
            let repo = captures.get(1).unwrap();
            let repo = match Url::parse(repo.as_str()) {
                Ok(url) => url,
                Err(err) => {
                    warn!("When parsing {key}, invalid repo url {url}: {err}", url=repo.as_str());
                    continue;
                }
            };
            let hook = captures.get(2).unwrap();
            let hook = match Url::parse(hook.as_str()) {
                Ok(url) => url,
                Err(err) => {
                    warn!("When parsing {key}, invalid hook url {hook}: {err}", hook=hook.as_str());
                    continue;
                }
            };
            secrets.repo_to_hook.entry(repo)
                .or_default()
                .push(SlackHook(hook));
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
