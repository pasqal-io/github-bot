use std::collections::HashMap;

use anyhow::{anyhow, Context};
use derive_more::{AsRef, Deref, Display, From};
use lazy_regex::{lazy_regex, Lazy};
use regex::Regex;
use serde::{de::Unexpected, Deserialize};
use url::Url;

/// The name of a repository.
#[derive(Hash, PartialEq, Eq, Debug, Deserialize, Display, AsRef)]
pub struct RepoName(String);
impl From<&RepoName> for String {
    fn from(repo_name: &RepoName) -> String {
        repo_name.0.clone()
    }
}
impl From<&str> for RepoName {
    fn from(name: &str) -> Self {
        RepoName(name.to_string())
    }
}

/// A capability to post messages in one Slack room.
///
/// Typically looks like https://hooks.slack.com/services/XXX/YYY/ZZZ
///
/// Confidentiality: secret.
#[derive(Deserialize, AsRef, From, PartialEq, Debug, Deref)]
pub struct SlackHook(Url);

pub struct ProjectToHook {
    pub project: Url,
    pub hook: SlackHook,
}
impl ProjectToHook {
    pub fn from_env_var(source: &str) -> Result<Self, anyhow::Error> {
        static REGEX: Lazy<Regex> = lazy_regex!{"(.*)=(.*)"};
        let Some(captures) = REGEX.captures(source)
        else {
            return Err(anyhow!("invalid env variable, expected <repo_url>=<hook_url"))
        };
        let project = captures.get(1).unwrap(); // Regex guarantees that we have two captures.
        let project = Url::parse(project.as_str())
            .context("invalid repo url")?;
        let hook = captures.get(2).unwrap();  // Regex guarantees that we have two captures.
        let hook = Url::parse(hook.as_str())
            .context("invalid hook url")?;
        Ok(ProjectToHook { project, hook: SlackHook(hook) })
    }
}


/// All the secrets we rely upon.
///
/// Typically an environment variable QASTOR_SECRETS, containing a JSON string.
#[derive(Deserialize)]
pub struct Secrets {
    #[serde(flatten)]
    pub repo_to_hook: HashMap<Url, Vec<SlackHook>>,
}

/// Configuration of a single project.
pub struct Project {
    /// Full url for the project. Used for display only.
    pub url: Url,

    /// Owner (user or org) of the repository. Used for fetching issues.
    pub owner: String,

    /// Name (user or org) of the repository. Used for fetching issues.
    pub repo: RepoName,
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
pub struct Config {
    /// The projects to monitor.
    #[serde(default)]
    pub projects: Vec<Project>,

    /// How often we're expecting to monitor the projects, as a number followed by a unit d/h/m/s.
    ///
    /// This variable only affects how far back we're looking in time for changes in issues.
    #[serde(
        deserialize_with = "Config::deserialize_update_frequency",
        default = "Config::default_update_frequency"
    )]
    pub update_frequency: chrono::Duration,
}
impl Config {
    /// Custom deserialization for update frequency.
    ///
    /// We don't want to specify the duration in seconds, as that's annoying, so implementing
    /// a shorthand notation.
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


#[cfg(test)]
mod test {
    use crate::{config::RepoName, config::Config};

    use super::ProjectToHook;

    /// Can a typical config be parsed?
    #[test]
    fn test_config_parse() {
        let source = r#"
            projects:
                - url: "https://github.com/owner1/project1"
                - url: "https://github.com/owner2/project2"
            update_frequency: 15m
        "#;
        let config: Config = serde_yaml::from_str(source).unwrap();
        assert_eq!(config.update_frequency, chrono::Duration::minutes(15));
        assert_eq!(config.projects.len(), 2);
        assert_eq!(config.projects[0].owner, "owner1");
        assert_eq!(config.projects[0].repo, RepoName::from("project1"));
        assert_eq!(config.projects[1].owner, "owner2");
        assert_eq!(config.projects[1].repo, RepoName::from("project2"));
    }

    /// Can a typical ProjectToHook be parsed?
    #[test]
    fn test_project_to_hook_parse() {
        let source = "https://github.com/owner1/project1=https://hooks.slack.com/services/YOUR/SLACK/HOOK";
        let parsed = ProjectToHook::from_env_var(source).unwrap();
        assert_eq!(parsed.project.as_str(), "https://github.com/owner1/project1");
        assert_eq!(parsed.hook.as_str(), "https://hooks.slack.com/services/YOUR/SLACK/HOOK");
    }
}