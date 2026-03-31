# Matrix

Moltis can connect to Matrix as a bot account using a homeserver URL plus
either an access token or a username/password login. The integration runs as an
outbound sync loop, so it does not require a public webhook URL, port
forwarding, or TLS termination on your side.

## How It Works

```
┌──────────────────────────────────────────────────────┐
│                 Matrix homeserver                     │
│          (/sync, send, relations, room APIs)         │
└──────────────────┬───────────────────────────────────┘
                   │  outbound HTTPS requests
                   ▼
┌──────────────────────────────────────────────────────┐
│                moltis-matrix crate                    │
│  ┌────────────┐  ┌────────────┐  ┌────────────────┐  │
│  │  Handler   │  │  Outbound  │  │     Plugin     │  │
│  │ (inbound)  │  │ (replies)  │  │  (lifecycle)   │  │
│  └────────────┘  └────────────┘  └────────────────┘  │
└──────────────────┬───────────────────────────────────┘
                   │
                   ▼
┌──────────────────────────────────────────────────────┐
│                 Moltis Gateway                        │
│         (chat dispatch, tools, memory)                │
└──────────────────────────────────────────────────────┘
```

The Matrix integration currently supports:

- Direct messages and room conversations
- Interactive action prompts via native Matrix polls
- Thread-aware replies and thread context fetch
- Streaming text responses
- Voice and audio message transcription
- Emoji reactions
- Native location sends and inbound location sharing
- OTP self-approval for unknown DM senders
- Per-room and per-user model overrides

## Prerequisites

Before configuring Moltis, you need a Matrix bot account:

1. Create or choose a Matrix account for the bot on your homeserver
2. Either obtain an access token, or keep the account password available
3. Note the full user ID, for example `@bot:example.com`
4. Optionally pick a stable `device_id` for session restore

```admonish warning
Matrix credentials are secrets. Treat access tokens and passwords like
passwords, never commit them to version control. Moltis stores them with
`secrecy::Secret` and redacts them from logs and API responses.
```

## Configuration

Add a `[channels.matrix.<account-id>]` section to your `moltis.toml`:

```toml
[channels.matrix.my-bot]
homeserver = "https://matrix.example.com"
access_token = "syt_..."
user_id = "@bot:example.com"
```

Password login is also supported:

```toml
[channels.matrix.my-bot]
homeserver = "https://matrix.example.com"
user_id = "@bot:example.com"
password = "correct horse battery staple"
device_display_name = "Moltis Matrix Bot"
```

To show Matrix in the channel picker, include `"matrix"` in `channels.offered`:

```toml
[channels]
offered = ["telegram", "discord", "slack", "matrix"]
```

### Configuration Fields

| Field | Required | Default | Description |
|-------|----------|---------|-------------|
| `homeserver` | **yes** | — | Base URL of the Matrix homeserver |
| `access_token` | no | — | Access token for the bot account, preferred when both auth methods are configured |
| `password` | no | — | Password for the bot account when access tokens are not used |
| `user_id` | no | — | Bot user ID, for example `@bot:example.com`, auto-detected via `whoami` when omitted |
| `device_id` | no | — | Optional device ID used for session restore |
| `device_display_name` | no | — | Optional device display name used for password-based logins |
| `dm_policy` | no | `"allowlist"` | Who can DM the bot: `"open"`, `"allowlist"`, or `"disabled"` |
| `room_policy` | no | `"allowlist"` | Which rooms can talk to the bot: `"open"`, `"allowlist"`, or `"disabled"` |
| `mention_mode` | no | `"mention"` | When the bot responds in rooms: `"always"`, `"mention"`, or `"none"` |
| `room_allowlist` | no | `[]` | Matrix room IDs or aliases allowed to interact with the bot |
| `user_allowlist` | no | `[]` | Matrix user IDs allowed to DM the bot |
| `auto_join` | no | `"always"` | Invite handling: `"always"`, `"allowlist"`, or `"off"` |
| `model` | no | — | Override the default model for this account |
| `model_provider` | no | — | Provider for the overridden model |
| `stream_mode` | no | `"edit_in_place"` | How streaming replies are sent: `"edit_in_place"` or `"off"` |
| `edit_throttle_ms` | no | `500` | Minimum milliseconds between edit-in-place streaming updates |
| `stream_min_initial_chars` | no | `30` | Minimum buffered characters before the first streamed send |
| `channel_overrides` | no | `{}` | Per-room model/provider overrides |
| `user_overrides` | no | `{}` | Per-user model/provider overrides |
| `reply_to_message` | no | `true` | Send threaded/rich replies when possible |
| `ack_reaction` | no | `"👀"` | Emoji reaction added while processing, omit to disable |
| `otp_self_approval` | no | `true` | Enable OTP self-approval for non-allowlisted DM users |
| `otp_cooldown_secs` | no | `300` | Cooldown in seconds after 3 failed OTP attempts |

### Full Example

```toml
[channels]
offered = ["matrix"]

[channels.matrix.my-bot]
homeserver = "https://matrix.example.com"
access_token = "syt_..."
user_id = "@bot:example.com"
device_id = "MOLTISBOT"
dm_policy = "allowlist"
room_policy = "allowlist"
mention_mode = "mention"
room_allowlist = ["!ops:example.com", "#support:example.com"]
user_allowlist = ["@alice:example.com", "@bob:example.com"]
auto_join = "allowlist"
model = "gpt-4.1"
model_provider = "openai"
stream_mode = "edit_in_place"
edit_throttle_ms = 500
stream_min_initial_chars = 30
reply_to_message = true
ack_reaction = "👀"
otp_self_approval = true
otp_cooldown_secs = 300

[channels.matrix.my-bot.channel_overrides."!ops:example.com"]
model = "claude-sonnet-4-20250514"
model_provider = "anthropic"

[channels.matrix.my-bot.user_overrides."@alice:example.com"]
model = "o3"
model_provider = "openai"
```

## Access Control

Matrix uses the same gating model as the other channel integrations.

### DM Policy

| Value | Behavior |
|-------|----------|
| `"allowlist"` | Only users in `user_allowlist` can DM the bot (default) |
| `"open"` | Anyone can DM the bot |
| `"disabled"` | DMs are silently ignored |

### Room Policy

| Value | Behavior |
|-------|----------|
| `"allowlist"` | Only rooms in `room_allowlist` are allowed (default) |
| `"open"` | Any joined room can interact with the bot |
| `"disabled"` | Room messages are silently ignored |

### Mention Mode

| Value | Behavior |
|-------|----------|
| `"mention"` | Bot only responds when explicitly mentioned in a room (default) |
| `"always"` | Bot responds to every message in allowed rooms |
| `"none"` | Bot never responds in rooms |

When `mention_mode = "mention"`, Moltis checks Matrix intentional mentions
(`m.mentions`) and also falls back to a literal MXID mention in the plain body.

## Invite Handling

| Value | Behavior |
|-------|----------|
| `"always"` | Auto-join every invite (default) |
| `"allowlist"` | Auto-join only when the inviter is in `user_allowlist` or the room is already in `room_allowlist` |
| `"off"` | Never auto-join invites |

## Threads and Replies

Matrix replies now preserve thread context when the referenced event belongs to
an existing thread. When `reply_to_message = true`, Moltis sends a rich reply
and keeps the reply inside the thread when appropriate.

For thread context injection, Moltis resolves the inbound event to the thread
root and fetches prior `m.thread` relations so the LLM sees the room thread
history instead of just the last message.

## Voice and Location Messages

Matrix audio messages are downloaded through the homeserver media API and
transcribed with the same voice pipeline used by the other voice-enabled
channels. If voice transcription is not configured, Moltis replies with setup
guidance instead of silently dropping the message.

Inbound Matrix location shares now update the stored user location and also
resolve pending tool-triggered location requests. If there is no pending
location request, the coordinates are forwarded to the chat session so the LLM
can acknowledge them naturally.

## Interactive Actions

When Moltis needs to ask the user to choose from a short list of actions, the
Matrix integration sends a native poll instead of a plain text fallback. The
selected poll answer is fed back into the same interaction callback path used by
the other channel integrations.

Matrix poll answers are single-choice and capped by the protocol at 20 options.
If a generated interactive message exceeds that limit, Moltis falls back to a
plain numbered text list.

## OTP Self-Approval

When `dm_policy = "allowlist"` and `otp_self_approval = true`, unknown DM users
can self-approve:

1. User sends a DM to the bot
2. Moltis generates a 6-digit OTP challenge
3. The code appears in the web UI under **Channels > Senders**
4. The bot owner shares the code out-of-band
5. User replies with the code in Matrix
6. On success, the sender is approved

After 3 failed attempts, the sender is locked out for `otp_cooldown_secs`
seconds.

## Troubleshooting

### Bot does not connect

- Verify `homeserver` is correct and reachable
- Verify the access token or password is valid
- Set `user_id` explicitly if startup auto-detection is unreliable
- Look at logs: `RUST_LOG=moltis_matrix=debug moltis`

### Bot does not respond in rooms

- Check `room_policy`
- Check `room_allowlist`
- Check `mention_mode`, especially if it is `"mention"` or `"none"`
- Make sure the bot has joined the room, or enable `auto_join`

### Bot does not respond in DMs

- Check `dm_policy`
- Check `user_allowlist`
- If OTP is enabled, look in **Channels > Senders** for a pending challenge
