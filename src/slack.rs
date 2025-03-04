use std::ops::Not;

use anyhow::{anyhow, Context};
use itertools::Itertools;
use log::debug;
use reqwest::Client;
use serde::Serialize;
use url::Url;

#[derive(Default, Serialize)]
pub struct SlackMessage {
    blocks: Vec<Block>    
}

#[derive(Serialize)]
struct Block {
    #[serde(rename="type")]
    typ: &'static str,
    emoji: bool,
    verbatim: bool,
    text: String,
}

impl SlackMessage {
    pub fn append_markdown(&mut self, markdown: String) {
        self.blocks.push(Block {
            typ: "mrkdwn",
            verbatim: true,
            text: markdown,
            emoji: false,
        });
    }
    pub fn link(url: &Url, text: Option<&str>) -> String {
        match text {
            None => format!("[{url}]({url})"),
            Some(text) => {
                let mut escaped = String::new();
                for c in text.chars() {
                    match c  {
                        '&' => escaped.push_str("&amp;"),
                        '<' => escaped.push_str("&lt;"),
                        '>' => escaped.push_str("&gt;"),
                        c => escaped.push(c),
                    }
                }
//                format!("[{escaped}]({url})")
                format!("<{url}|{escaped}>")
            }
        }
    }

    pub async fn send(self, client: &Client, hook: &Url) -> Result<(), anyhow::Error> {
        let text = format!("{}", self.blocks.into_iter().map(|b| b.text).format(""));
        #[derive(Serialize)]
        struct Payload {
            blocks: Vec<Context>            
        }
        #[derive(Serialize)]
        struct Context {
            #[serde(rename="type")]
            typ_: &'static str,
            elements: Vec<Text>
        }
        #[derive(Serialize)]
        struct Text {
            #[serde(rename="type")]
            typ_: &'static str,
            text: String
        }
        let payload = Payload {
            blocks: vec![
                Context {
                    typ_: "context",
                    elements: vec![
                        Text {
                            typ_: "mrkdwn",
                            text
                        },
                    ]
                }
            ]
        };
        debug!("Sending: {}",
            serde_json::to_string_pretty(&payload)
                .unwrap());
        let response = client
            .post(hook.to_string())
            .json(&payload)
            .send()
            .await
            .context("Error while posting message to Slack")?;
        let status = response.status();
        if status.is_success().not() {
            let text = response.text()
                .await
                .context("Could not gather response")?;
            return Err(anyhow!("Slack responded with an error {}: {}", status, text));
        }
        Ok(())
    }
}