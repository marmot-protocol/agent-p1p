# Switch Pip's OpenAI account on Pirate

Keep execution paused while changing credentials. These commands target
`pip-control`'s `/var/lib/pip/hermes/auth.json`, shared by its role profiles;
they do not target Jeff's personal Hermes or the interactive Codex app.

While SSH'd into Pirate:

```bash
pip_auth() {
  (
    cd /
    sudo -u pip-control env -i \
      HOME=/var/lib/pip/hermes/home \
      HERMES_HOME=/var/lib/pip/hermes \
      HERMES_KANBAN_HOME=/var/lib/pip/hermes \
      PATH=/usr/local/bin:/usr/bin:/bin \
      /usr/local/bin/hermes auth "$@"
  )
}

pip_auth list openai-codex
pip_auth add openai-codex --label pip-control-pirate-new &&
  pip_auth remove openai-codex pip-control-pirate &&
  pip_auth list openai-codex
```

The old label above was verified on 2026-09-05. For a later switch, use the exact
old label from `auth list`, and choose a distinct new label. Never blindly
remove an index that may have changed. If removal fails after login succeeds,
both credentials may remain; leave execution paused and inspect the list.

Open the printed device-login URL in a private browser window and authenticate
as the desired new account. If device-code login is unavailable, check the
new account's security settings or workspace permissions, per
[OpenAI's headless-login guidance](https://learn.chatgpt.com/docs/auth#login-on-headless-devices).
Do not share the device code, tokens, or auth file.

On the installed Hermes version, `auth add openai-codex` creates a fresh OAuth
pool entry. Removing the old named entry also suppresses its legacy singleton
source, so it is not silently re-imported. Adding first preserves the old
working credential if the new login fails. Leaving both entries would permit
pool rotation rather than replacing the account.

Afterward, verify exactly one intended credential remains and the role auth
links still point to the shared service file. A configured-model provider probe
is a separate, explicitly bounded check; listing credentials does not prove
model entitlement or account limits.
