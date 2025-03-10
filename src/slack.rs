use std::{ops::Not, sync::Arc};

use anyhow::{anyhow, Context};
use log::debug;
use reqwest::Client;
use serde::Serialize;
use url::Url;

#[derive(Serialize)]
pub struct Section {
    title: Text,
    fields: Vec<Text>,
}

#[derive(Serialize, Clone)]
struct Text {
    #[serde(rename = "type")]
    typ: &'static str,
    text: Arc<str>,
}

impl Section {
    pub fn new(title: String) -> Self {
        Section {
            title: Text {
                typ: "mrkdwn",
                text: title.into(),
            },
            fields: vec![],
        }
    }

    pub fn append_fields(&mut self, headers: &[String]) {
        self.fields.extend(headers.iter().map(|header| Text {
            typ: "mrkdwn",
            text: header.clone().into(),
        }));
    }

    pub async fn send(&self, client: &Client, hook: &Url) -> Result<(), anyhow::Error> {
        #[derive(Serialize)]
        struct Payload {
            blocks: [Section; 1],
        }
        #[derive(Serialize)]
        struct Section {
            #[serde(rename = "type")]
            typ_: &'static str,
            text: Text,
            fields: Vec<Text>,
        }
        let payload = Payload {
            blocks: [Section {
                typ_: "section",
                text: self.title.clone(),
                fields: self.fields.clone(),
            }],
        };
        debug!(
            "Sending: {}",
            serde_json::to_string_pretty(&payload).unwrap()
        );
        let response = client
            .post(hook.to_string())
            .json(&payload)
            .send()
            .await
            .context("Error while posting message to Slack")?;
        let status = response.status();
        if status.is_success().not() {
            let text = response.text().await.context("Could not gather response")?;
            return Err(anyhow!(
                "Slack responded with an error {}: {}",
                status,
                text
            ));
        }
        Ok(())
    }
}

pub fn link(url: &Url, text: Option<&str>) -> String {
    match text {
        None => format!("[{url}]({url})"),
        Some(text) => {
            let mut escaped = String::new();
            for c in text.chars() {
                match c {
                    '&' => escaped.push_str("&amp;"),
                    '<' => escaped.push_str("&lt;"),
                    '>' => escaped.push_str("&gt;"),
                    c => escaped.push(c),
                }
            }
            format!("<{url}|{escaped}>")
        }
    }
}
