# Qastor

A small bot to patrol GitHub repositories and ping developers on Slack whenever
there's an issue or a pending review.

## Why?

There's a Slack GitHub bot, but it requires full privileges to read, write, pretend
to be a user, etc. This is... a bit overkill.

This bot is open-source and only requires the authorization to post on select Slack channels,
so that's an improvement wrt security.

## Setup

### Slack

You need a Slack server and the authorization to create bots in this Slack server.

1. Go to your [Slack app dashboard](https://api.slack.com/apps/).
2. Create a new application, give it a name (e.g. "Qastor", an icon, etc.)
3. Go to "Incoming Webhooks", "Add new webhook to workspace" and pick a channel to which Qastor will be allowed to post.
4. This will give you an URL, write it down.
5. Repeat if you need Qastor to post to more than one channel or more than one server.


### Running on GitHub CI (optional)

One way to run Qastor is on GitHub CI. This has the benefit that you do not need dedicated infrastructure (the bit typically takes a few seconds per day to execute, most of this time being spent loading the binary from the cache).

1. Create a dedicated GitHub project.
2. Add an action patrol.yml, see [examples/patrol.yml](patrol.yml). Customize the frequency.
3. Setup a secret `QASTOR_SECRETS` along the following lines
```js
QASTOR_SECRETS={ // JS object.
    "https://github.com/owner/project": [
        "https://hooks.slack.com/services/YOUR/SLACK/HOOK",
        "https://hooks.slack.com/services/ANOTHER/SLACK/HOOK" // Most projects are only announced on a single chan, but some might need to be announced in more.
    ],
    "https://github.com/owner/project2": [
        ...
    ]
}
```
4. Add a file `config.yml` on your GitHub project, which looks like
```yaml
projects:
    - url: "https://github.com/owner/project"
    - url: "https://github.com/owner/project2"

update_frequency: 12h
```


## Security considerations

### Slack-side

This bot has access to select Slack channels, for writing, under a clearly visible identity, and is labeled as a bot,
so the worst case scenario (assuming that the bot is compromised in a supply-chain attack or that the secrets have
leaked) is erroneous content clearly labeled as sent by the bot on the Slack channels to which the bot has the
authorization to write.

### GitHub side

We have not attempted to determine exactly which permissions this bot has access to when executed from your
pipeline. We _believe_ that it is generally sandboxed to the GitHub project to which it belongs, but at the
very least, it still has network access.
