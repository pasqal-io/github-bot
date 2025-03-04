use std::collections::HashMap;

use anyhow::Context;
use derive_more::{AsRef, Display};
use lazy_regex::lazy_regex;
use log::warn;
use reqwest::Client;
use serde::{de::Unexpected, Deserialize};


mod slack;

use slack::SlackMessage;
use url::Url;

#[derive(Hash, PartialEq, Eq, Debug, Deserialize, Display, AsRef)]
struct RepoName(String);
impl From<&RepoName> for String {
    fn from(repo_name: &RepoName) -> String {
        repo_name.0.clone()
    }
}

#[derive(Deserialize)]
struct SlackHook(Url);

#[derive(Deserialize)]
struct Secrets {
    #[serde(flatten)]
    repo_to_hook: HashMap<RepoName, SlackHook>    
}

#[derive(Deserialize)]
struct Project {
    url: Url,
    owner: String,
    repo: RepoName,
}

#[derive(Deserialize)]
struct Config {
    projects: Vec<Project>,
    #[serde(deserialize_with = "Config::deserialize_update_frequency")]
    update_frequency: chrono::Duration,
}
impl Config {
    fn deserialize_update_frequency<'de, D>(deserializer: D) -> Result<chrono::Duration, D::Error>
    where
        D: serde::Deserializer<'de> {
            use serde::de::Error;
            let source = String::deserialize(deserializer)?;
            let regex = lazy_regex!("([[:digit:]]+) *([hmsd])");
            let found = regex.captures(&source)
                .ok_or_else(|| D::Error::invalid_value(Unexpected::Str(&source), &"numbers followed by a unit d/h/m/s"))?;
            let digits = found.get(1)
                .expect("we should have digits");
            let unit = found.get(2)
                .expect("we should have a unit");
            let digits: i64 = digits.as_str().parse()
                .map_err(|_| D::Error::invalid_value(Unexpected::Str(digits.as_str()), &"numbers"))?;
            let unit: char = unit.as_str().parse()
                .map_err(|_| D::Error::invalid_value(Unexpected::Str(unit.as_str()), &"a unit d/h/m/s"))?;
            let result = match unit {
                'd' => chrono::Duration::days(digits),
                'h' => chrono::Duration::hours(digits),
                'm' => chrono::Duration::minutes(digits),
                's' => chrono::Duration::seconds(digits),
                _ => unreachable!()
            };
            Ok(result)
    }
}



async fn per_project(client: &Client, secrets: &Secrets, project: &Project, config: &Config) -> Result<(), anyhow::Error> {
    let since = chrono::Local::now() - config.update_frequency;

    // First instantiate the slack hook.
    let slack_hook = secrets.repo_to_hook.get(&project.repo)
        .context("Missing secret")?;

    let octocrab = octocrab::instance();
    let issues = octocrab.issues(&project.owner, &project.repo)
        .list()
        .since(since)
        .send()
        .await
        .context("Couldn't download recent issues")?;

    if issues.items.is_empty() {
        return Ok(())
    }

    let mut msg = slack::SlackMessage::default();
    msg.append_markdown(format!("Issues of repo {link} updated since {since:?}",
        link = SlackMessage::link(&project.url, Some(project.repo.as_ref())),
        since = since,
    ));
    for issue in issues.items.into_iter() {
        msg.append_markdown(format!("\n - {link} last updated on {date} by {id}",
            link = SlackMessage::link(&issue.url, Some(issue.title.as_str())),
            date = issue.updated_at.format("%d/%m/%Y %H:%M"),
            id = issue.user.id
        ));
    }
    msg.send(client, &slack_hook.0)
        .await
        .context("Failed to post udpdate on Slack")?;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    env_logger::init();
    dotenv::dotenv()
        .context("Failed to load .env")?;

    // Load secrets.
    let env_secrets = std::env::var("GITHUB_BOT_SECRETS")
        .context("Missing env GITHUB_BOT_SECRETS")?;
    let secrets: Secrets = serde_json::from_str(&env_secrets)
        .context("Invalid env GITHUB_BOT_SECRETS")?;

    // Load config.
    let file_config = std::fs::File::open("config.yml")
        .context("Could not open config.yml")?;
    let config: Config = serde_yaml::from_reader(file_config)
        .context("Invalid config.yml")?;

    let client = reqwest::Client::new();

    for project in &config.projects {
        if let Err(err) = per_project(&client, &secrets, project, &config)
        .await {
            warn!("Error handling project {}/{}: {:?}", project.owner, project.repo, err)
        }
    }
    Ok(())
}