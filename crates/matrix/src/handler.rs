use std::sync::Arc;

use {
    matrix_sdk::{
        Room,
        media::{MediaFormat, MediaRequestParameters},
        ruma::{
            OwnedUserId,
            events::room::{
                member::StrippedRoomMemberEvent,
                message::{
                    AudioMessageEventContent, LocationMessageEventContent, MessageType,
                    OriginalSyncRoomMessageEvent,
                },
            },
        },
    },
    tracing::{debug, info, warn},
};

use {
    moltis_channels::{
        ChannelEvent, ChannelType,
        config_view::ChannelConfigView,
        gating::{self, DmPolicy, GroupPolicy},
        message_log::MessageLogEntry,
        otp::{OtpInitResult, OtpVerifyResult},
        plugin::{ChannelEventSink, ChannelMessageKind, ChannelMessageMeta, ChannelReplyTarget},
    },
    moltis_common::types::ChatType,
};

use crate::{
    access,
    config::{AutoJoinPolicy, MatrixAccountConfig},
    state::AccountStateMap,
};

#[tracing::instrument(skip(ev, room, accounts, bot_user_id), fields(account_id, room = %room.room_id()))]
pub async fn handle_room_message(
    ev: OriginalSyncRoomMessageEvent,
    room: Room,
    account_id: String,
    accounts: AccountStateMap,
    bot_user_id: OwnedUserId,
) {
    if ev.sender == bot_user_id {
        return;
    }

    let room_id = room.room_id().to_string();
    let sender_id = ev.sender.to_string();
    let event_id = ev.event_id.to_string();

    let Some(kind) = inbound_message_kind(&ev.content.msgtype) else {
        return;
    };
    let body = inbound_message_body(&ev.content.msgtype);

    if body.is_empty() && matches!(kind, ChannelMessageKind::Text) {
        return;
    }

    record_message_received();

    // Snapshot config+state without holding lock across .await
    let (config, message_log, event_sink) = {
        let guard = accounts.read().unwrap_or_else(|e| e.into_inner());
        match guard.get(&account_id) {
            Some(s) => (
                s.config.clone(),
                s.message_log.clone(),
                s.event_sink.clone(),
            ),
            None => {
                warn!(account_id, "account state not found");
                return;
            },
        }
    };

    let chat_type = match room.is_direct().await {
        Ok(true) => ChatType::Dm,
        Ok(false) => ChatType::Group,
        Err(error) => {
            warn!(
                account_id,
                room = %room_id,
                "failed to determine Matrix DM state, treating room as group: {error}"
            );
            ChatType::Group
        },
    };

    let bot_mentioned = is_bot_mentioned(&ev, &bot_user_id, &body);

    if let Err(reason) =
        access::check_access(&config, &chat_type, &sender_id, &room_id, bot_mentioned)
    {
        if matches!(chat_type, ChatType::Dm)
            && matches!(reason, access::AccessDenied::NotOnAllowlist)
            && config.otp_self_approval
            && config.dm_policy == DmPolicy::Allowlist
        {
            handle_otp(
                &body,
                &sender_id,
                &account_id,
                &accounts,
                &event_sink,
                &room,
            )
            .await;
            return;
        }
        debug!(account_id, sender = %sender_id, %reason, "access denied");
        return;
    }

    let sender_name = room
        .get_member_no_sync(&ev.sender)
        .await
        .ok()
        .flatten()
        .and_then(|m| m.display_name().map(|s| s.to_string()));

    if let Some(emoji) = &config.ack_reaction {
        let room_clone = room.clone();
        let event_id_clone = ev.event_id.clone();
        let emoji_clone = emoji.clone();
        tokio::spawn(async move {
            use matrix_sdk::ruma::events::{reaction::ReactionEventContent, relation::Annotation};
            let annotation = Annotation::new(event_id_clone, emoji_clone);
            let content = ReactionEventContent::new(annotation);
            if let Err(e) = room_clone.send(content).await {
                warn!("failed to send ack reaction: {e}");
            }
        });
    }

    if let Some(log) = &message_log {
        let _ = log
            .log(MessageLogEntry {
                id: 0,
                account_id: account_id.clone(),
                channel_type: "matrix".into(),
                peer_id: sender_id.clone(),
                username: Some(sender_id.clone()),
                sender_name: sender_name.clone(),
                chat_id: room_id.clone(),
                chat_type: if matches!(chat_type, ChatType::Dm) {
                    "dm"
                } else {
                    "group"
                }
                .into(),
                body: body.clone(),
                access_granted: true,
                created_at: unix_now(),
            })
            .await;
    }

    let reply_to = ChannelReplyTarget {
        channel_type: ChannelType::Matrix,
        account_id: account_id.clone(),
        chat_id: room_id.clone(),
        message_id: if config.reply_to_message {
            Some(event_id.clone())
        } else {
            None
        },
    };

    let meta = ChannelMessageMeta {
        channel_type: ChannelType::Matrix,
        sender_name: sender_name.clone(),
        username: Some(sender_id.clone()),
        message_kind: Some(kind),
        model: config.resolve_model(&room_id, &sender_id).map(String::from),
        audio_filename: None,
    };

    if let Some(sink) = &event_sink {
        sink.emit(ChannelEvent::InboundMessage {
            channel_type: ChannelType::Matrix,
            account_id: account_id.clone(),
            peer_id: sender_id.clone(),
            username: Some(sender_id.clone()),
            sender_name: sender_name.clone(),
            message_count: Some(1),
            access_granted: true,
        })
        .await;

        match &ev.content.msgtype {
            MessageType::Audio(audio) => {
                handle_audio_message(
                    audio,
                    &room,
                    &account_id,
                    &event_id,
                    sink.as_ref(),
                    reply_to,
                    meta,
                )
                .await;
                return;
            },
            MessageType::Location(location) => {
                handle_location_message(
                    location,
                    &room,
                    &account_id,
                    &event_id,
                    sink.as_ref(),
                    reply_to,
                    meta,
                )
                .await;
                return;
            },
            _ => {},
        }

        sink.dispatch_to_chat(&body, reply_to, meta).await;
    }
}

pub async fn handle_poll_response(
    room: Room,
    account_id: String,
    accounts: AccountStateMap,
    sender_id: String,
    callback_data: Option<String>,
) {
    let Some(callback_data) = callback_data else {
        return;
    };

    let room_id = room.room_id().to_string();
    let (event_sink, bot_user_id) = {
        let guard = accounts.read().unwrap_or_else(|e| e.into_inner());
        let Some(state) = guard.get(&account_id) else {
            warn!(account_id, "account state not found");
            return;
        };

        (state.event_sink.clone(), state.bot_user_id.clone())
    };

    if sender_id == bot_user_id {
        return;
    }

    let Some(sink) = event_sink else {
        return;
    };

    record_message_received();

    let reply_to = ChannelReplyTarget {
        channel_type: ChannelType::Matrix,
        account_id: account_id.clone(),
        chat_id: room_id,
        message_id: None,
    };

    if let Err(error) = sink.dispatch_interaction(&callback_data, reply_to).await {
        debug!(
            account_id,
            callback_data, "matrix poll interaction dispatch failed: {error}"
        );
    }
}

fn should_auto_join_invite(config: &MatrixAccountConfig, inviter_id: &str, room_id: &str) -> bool {
    let room_allowed = match config.room_policy {
        GroupPolicy::Disabled => false,
        GroupPolicy::Open => true,
        GroupPolicy::Allowlist => {
            !config.room_allowlist.is_empty() && gating::is_allowed(room_id, &config.room_allowlist)
        },
    };

    if !room_allowed {
        return false;
    }

    match config.auto_join {
        AutoJoinPolicy::Always => true,
        AutoJoinPolicy::Off => false,
        AutoJoinPolicy::Allowlist => {
            gating::is_allowed(inviter_id, &config.user_allowlist)
                || gating::is_allowed(room_id, &config.room_allowlist)
        },
    }
}

fn is_bot_mentioned(
    event: &OriginalSyncRoomMessageEvent,
    bot_user_id: &OwnedUserId,
    body: &str,
) -> bool {
    event
        .content
        .mentions
        .as_ref()
        .is_some_and(|mentions| mentions.room || mentions.user_ids.contains(bot_user_id))
        || body.contains(bot_user_id.as_str())
}

pub(crate) fn first_selection(selections: &[String]) -> Option<String> {
    selections.first().cloned()
}

fn inbound_message_kind(msgtype: &MessageType) -> Option<ChannelMessageKind> {
    Some(match msgtype {
        MessageType::Text(_) | MessageType::Notice(_) => ChannelMessageKind::Text,
        MessageType::Image(_) => ChannelMessageKind::Photo,
        MessageType::Audio(audio) => infer_audio_kind(audio),
        MessageType::Video(_) => ChannelMessageKind::Video,
        MessageType::File(_) => ChannelMessageKind::Document,
        MessageType::Location(_) => ChannelMessageKind::Location,
        _ => return None,
    })
}

fn inbound_message_body(msgtype: &MessageType) -> String {
    match msgtype {
        MessageType::Text(text) => text.body.clone(),
        MessageType::Notice(notice) => notice.body.clone(),
        MessageType::Audio(audio) => audio_caption(audio).unwrap_or_default(),
        MessageType::Location(location) => location.plain_text_representation().to_string(),
        _ => String::new(),
    }
}

#[allow(clippy::too_many_arguments)]
async fn handle_audio_message(
    audio: &AudioMessageEventContent,
    room: &Room,
    account_id: &str,
    event_id: &str,
    sink: &dyn ChannelEventSink,
    reply_to: ChannelReplyTarget,
    mut meta: ChannelMessageMeta,
) {
    if !sink.voice_stt_available().await {
        if let Err(error) = send_text(
            room,
            "I received your audio message but voice transcription is not available. Please visit Settings -> Voice.",
        )
        .await
        {
            warn!(account_id, "failed to send STT setup hint: {error}");
        }
        return;
    }

    let format = audio_format(audio);
    let request = MediaRequestParameters {
        source: audio.source.clone(),
        format: MediaFormat::File,
    };

    let audio_data =
        match room
            .client()
            .media()
            .get_media_content(&request, true)
            .await
        {
            Ok(audio_data) => audio_data,
            Err(error) => {
                warn!(
                    account_id,
                    event_id, "failed to download Matrix audio: {error}"
                );
                if let Err(send_error) = send_text(
                room,
                "I received your audio message but couldn't download the audio. Please try again.",
            )
            .await
            {
                warn!(account_id, "failed to send Matrix audio download error: {send_error}");
            }
                return;
            },
        };

    meta.message_kind = Some(infer_audio_kind(audio));
    let filename = saved_audio_filename(
        event_id,
        audio.filename.as_deref(),
        inferred_filename(audio.body.as_str()),
        format,
    );
    meta.audio_filename = sink
        .save_channel_voice(&audio_data, &filename, &reply_to)
        .await;

    match sink.transcribe_voice(&audio_data, format).await {
        Ok(transcribed) => {
            let transcribed = transcribed.trim();
            let body = if transcribed.is_empty() {
                format!(
                    "[{} message - could not transcribe]",
                    audio_kind_label(meta.message_kind)
                )
            } else if let Some(caption) = audio_caption(audio) {
                format!("{caption}\n\n[Audio message]: {transcribed}")
            } else {
                transcribed.to_string()
            };

            sink.dispatch_to_chat(&body, reply_to, meta).await;
        },
        Err(error) => {
            warn!(
                account_id,
                event_id, "Matrix audio transcription failed: {error}"
            );
            let fallback = audio_caption(audio).unwrap_or_else(|| {
                format!(
                    "[{} message - transcription unavailable]",
                    audio_kind_label(meta.message_kind)
                )
            });
            sink.dispatch_to_chat(&fallback, reply_to, meta).await;
        },
    }
}

async fn handle_location_message(
    location: &LocationMessageEventContent,
    room: &Room,
    account_id: &str,
    event_id: &str,
    sink: &dyn ChannelEventSink,
    reply_to: ChannelReplyTarget,
    meta: ChannelMessageMeta,
) {
    let Some((latitude, longitude)) = parse_geo_uri(location.geo_uri()) else {
        warn!(
            account_id,
            event_id,
            geo_uri = location.geo_uri(),
            "received Matrix location with invalid geo URI"
        );
        let body = location.plain_text_representation().trim().to_string();
        if !body.is_empty() {
            sink.dispatch_to_chat(&body, reply_to, meta).await;
        }
        return;
    };

    let resolved = sink.update_location(&reply_to, latitude, longitude).await;
    info!(
        account_id,
        event_id,
        latitude,
        longitude,
        resolved_pending_request = resolved,
        "Matrix location received"
    );

    if resolved {
        if let Err(error) = send_text(room, "Location updated.").await {
            warn!(
                account_id,
                "failed to send Matrix location confirmation: {error}"
            );
        }
        return;
    }

    sink.dispatch_to_chat(
        &location_dispatch_body(location, latitude, longitude),
        reply_to,
        meta,
    )
    .await;
}

fn audio_kind_label(kind: Option<ChannelMessageKind>) -> &'static str {
    match kind {
        Some(ChannelMessageKind::Voice) => "voice",
        _ => "audio",
    }
}

fn infer_audio_kind(audio: &AudioMessageEventContent) -> ChannelMessageKind {
    match audio_format(audio) {
        "ogg" | "opus" => ChannelMessageKind::Voice,
        _ => ChannelMessageKind::Audio,
    }
}

fn audio_caption(audio: &AudioMessageEventContent) -> Option<String> {
    audio
        .caption()
        .map(str::trim)
        .filter(|caption| !caption.is_empty())
        .map(ToOwned::to_owned)
}

fn audio_format(audio: &AudioMessageEventContent) -> &'static str {
    audio_format_from_metadata(
        audio
            .info
            .as_ref()
            .and_then(|info| info.mimetype.as_deref()),
        audio
            .filename
            .as_deref()
            .or_else(|| inferred_filename(audio.body.as_str())),
    )
}

fn audio_format_from_metadata(mimetype: Option<&str>, filename: Option<&str>) -> &'static str {
    if let Some(mimetype) = mimetype
        && let Some(format) = audio_format_from_mimetype(mimetype)
    {
        return format;
    }

    filename
        .and_then(|filename| {
            std::path::Path::new(filename)
                .extension()
                .and_then(|ext| ext.to_str())
        })
        .and_then(audio_format_from_extension)
        .unwrap_or("ogg")
}

fn audio_format_from_mimetype(mimetype: &str) -> Option<&'static str> {
    Some(match mimetype {
        "audio/ogg" | "audio/ogg; codecs=opus" => "ogg",
        "audio/opus" => "opus",
        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/mp4" | "audio/m4a" | "audio/x-m4a" | "audio/aac" => "m4a",
        "audio/wav" | "audio/x-wav" => "wav",
        "audio/webm" => "webm",
        "audio/flac" | "audio/x-flac" => "flac",
        _ => return None,
    })
}

fn audio_format_from_extension(extension: &str) -> Option<&'static str> {
    Some(match extension.to_ascii_lowercase().as_str() {
        "ogg" => "ogg",
        "opus" => "opus",
        "mp3" => "mp3",
        "m4a" | "aac" => "m4a",
        "wav" => "wav",
        "webm" => "webm",
        "flac" => "flac",
        _ => return None,
    })
}

fn inferred_filename(body: &str) -> Option<&str> {
    let candidate = body.trim();
    if candidate.is_empty() || candidate.contains('\n') {
        return None;
    }

    let extension = std::path::Path::new(candidate)
        .extension()
        .and_then(|ext| ext.to_str())?;
    audio_format_from_extension(extension).map(|_| candidate)
}

fn saved_audio_filename(
    event_id: &str,
    filename: Option<&str>,
    body_filename: Option<&str>,
    format: &str,
) -> String {
    let candidate = filename
        .or(body_filename)
        .map(str::trim)
        .filter(|name| !name.is_empty());
    if let Some(candidate) = candidate {
        let cleaned = candidate
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(candidate)
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
                    ch
                } else {
                    '_'
                }
            })
            .collect::<String>();
        if !cleaned.is_empty() {
            if std::path::Path::new(&cleaned).extension().is_some() {
                return cleaned;
            }
            return format!("{cleaned}.{format}");
        }
    }

    let suffix = event_id
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>();
    format!("voice-matrix-{}.{}", suffix, format)
}

fn parse_geo_uri(geo_uri: &str) -> Option<(f64, f64)> {
    let coordinates = geo_uri.trim().strip_prefix("geo:")?;
    let mut parts = coordinates.split(';');
    let lat_lon = parts.next()?;
    let mut lat_lon_parts = lat_lon.split(',');
    let latitude = lat_lon_parts.next()?.trim().parse().ok()?;
    let longitude = lat_lon_parts.next()?.trim().parse().ok()?;
    Some((latitude, longitude))
}

fn location_dispatch_body(
    location: &LocationMessageEventContent,
    latitude: f64,
    longitude: f64,
) -> String {
    let description = location.plain_text_representation().trim();
    if description.is_empty() || description == location.geo_uri() {
        return format!("I'm sharing my location: {latitude}, {longitude}");
    }

    format!("{description}\n\nShared location: {latitude}, {longitude}")
}

fn record_message_received() {
    #[cfg(feature = "metrics")]
    moltis_metrics::counter!(
        moltis_metrics::channels::MESSAGES_RECEIVED_TOTAL,
        moltis_metrics::labels::CHANNEL => "matrix"
    )
    .increment(1);
}

async fn handle_otp(
    body: &str,
    sender_id: &str,
    account_id: &str,
    accounts: &AccountStateMap,
    event_sink: &Option<Arc<dyn ChannelEventSink>>,
    room: &Room,
) {
    let trimmed = body.trim();

    if trimmed.len() == 6 && trimmed.chars().all(|c| c.is_ascii_digit()) {
        let result = {
            let guard = accounts.read().unwrap_or_else(|e| e.into_inner());
            if let Some(state) = guard.get(account_id) {
                let mut otp = state.otp.lock().unwrap_or_else(|e| e.into_inner());
                otp.verify(sender_id, trimmed)
            } else {
                return;
            }
        };

        match result {
            OtpVerifyResult::Approved => {
                let _ = send_text(room, "Access granted.").await;
                if let Some(sink) = &event_sink {
                    sink.emit(ChannelEvent::OtpResolved {
                        channel_type: ChannelType::Matrix,
                        account_id: account_id.into(),
                        peer_id: sender_id.into(),
                        username: Some(sender_id.into()),
                        resolution: "approved".into(),
                    })
                    .await;
                }
            },
            OtpVerifyResult::WrongCode { attempts_left } => {
                let msg = format!("Invalid code. {attempts_left} attempts remaining.");
                let _ = send_text(room, &msg).await;
            },
            OtpVerifyResult::Expired => {
                let _ = send_text(room, "Code expired. Send any message for a new one.").await;
            },
            OtpVerifyResult::LockedOut => {
                let _ = send_text(room, "Too many attempts. Please wait.").await;
            },
            OtpVerifyResult::NoPending => {
                // Fall through to initiate
            },
        }
        if !matches!(result, OtpVerifyResult::NoPending) {
            return;
        }
    }

    let (result, otp_cooldown_secs) = {
        let guard = accounts.read().unwrap_or_else(|e| e.into_inner());
        if let Some(state) = guard.get(account_id) {
            let mut otp = state.otp.lock().unwrap_or_else(|e| e.into_inner());
            (
                otp.initiate(sender_id, Some(sender_id.into()), None),
                state.config.otp_cooldown_secs,
            )
        } else {
            return;
        }
    };

    match result {
        OtpInitResult::Created(code) => {
            let expires_at =
                unix_now().saturating_add(i64::try_from(otp_cooldown_secs).unwrap_or(i64::MAX));
            let msg = format!(
                "You're not on the allowlist. A verification code has been generated.\n\
                 Ask the admin to approve code: **{code}**\n\
                 Or enter it here if you have it."
            );
            let _ = send_text(room, &msg).await;
            if let Some(sink) = &event_sink {
                sink.emit(ChannelEvent::OtpChallenge {
                    channel_type: ChannelType::Matrix,
                    account_id: account_id.into(),
                    peer_id: sender_id.into(),
                    username: Some(sender_id.into()),
                    sender_name: Some(sender_id.into()),
                    code,
                    expires_at,
                })
                .await;
            }
        },
        OtpInitResult::AlreadyPending => {
            let _ = send_text(room, "A verification code is already pending.").await;
        },
        OtpInitResult::LockedOut => {
            let _ = send_text(room, "Too many failed attempts. Please wait.").await;
        },
    }
}

#[tracing::instrument(skip(ev, room, accounts, bot_user_id), fields(account_id, room = %room.room_id(), inviter = %ev.sender))]
pub async fn handle_invite(
    ev: StrippedRoomMemberEvent,
    room: Room,
    account_id: String,
    accounts: AccountStateMap,
    bot_user_id: OwnedUserId,
) {
    if ev.state_key != bot_user_id {
        return;
    }

    let auto_join = {
        let guard = accounts.read().unwrap_or_else(|e| e.into_inner());
        match guard.get(&account_id) {
            Some(state) => {
                should_auto_join_invite(&state.config, ev.sender.as_str(), room.room_id().as_str())
            },
            None => return,
        }
    };

    if !auto_join {
        debug!(account_id, room = %room.room_id(), "ignoring invite (auto_join policy)");
        return;
    }

    info!(account_id, room = %room.room_id(), inviter = %ev.sender, "auto-joining room");
    if let Err(e) = room.join().await {
        warn!(account_id, room = %room.room_id(), "failed to auto-join: {e}");
    }
}

pub async fn send_text(room: &Room, text: &str) -> Result<(), matrix_sdk::Error> {
    use matrix_sdk::ruma::events::room::message::RoomMessageEventContent;
    let content = RoomMessageEventContent::text_plain(text);
    room.send(content).await?;
    Ok(())
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use {
        super::{
            audio_format_from_metadata, first_selection, infer_audio_kind, is_bot_mentioned,
            location_dispatch_body, parse_geo_uri, saved_audio_filename, should_auto_join_invite,
        },
        crate::config::{AutoJoinPolicy, MatrixAccountConfig},
        matrix_sdk::ruma::{
            events::room::message::{
                AudioMessageEventContent, LocationMessageEventContent, OriginalSyncRoomMessageEvent,
            },
            mxc_uri, owned_user_id,
            serde::Raw,
        },
        moltis_channels::{gating::GroupPolicy, plugin::ChannelMessageKind},
        serde_json::json,
    };

    fn message_event(value: serde_json::Value) -> OriginalSyncRoomMessageEvent {
        Raw::from_json_string(value.to_string())
            .unwrap_or_else(|error| panic!("raw event: {error}"))
            .deserialize()
            .unwrap_or_else(|error| panic!("message event: {error}"))
    }

    #[test]
    fn bot_mention_detected_from_intentional_mentions() {
        let bot_user_id = owned_user_id!("@bot:example.org");
        let event = message_event(json!({
            "type": "m.room.message",
            "event_id": "$1",
            "room_id": "!room:example.org",
            "sender": "@alice:example.org",
            "origin_server_ts": 1,
            "content": {
                "msgtype": "m.text",
                "body": "hello",
                "m.mentions": {
                    "user_ids": ["@bot:example.org"]
                }
            }
        }));

        assert!(is_bot_mentioned(&event, &bot_user_id, "hello"));
    }

    #[test]
    fn bot_mention_detected_from_literal_mxid_fallback() {
        let bot_user_id = owned_user_id!("@bot:example.org");
        let event = message_event(json!({
            "type": "m.room.message",
            "event_id": "$1",
            "room_id": "!room:example.org",
            "sender": "@alice:example.org",
            "origin_server_ts": 1,
            "content": {
                "msgtype": "m.text",
                "body": "@bot:example.org hello"
            }
        }));

        assert!(is_bot_mentioned(
            &event,
            &bot_user_id,
            "@bot:example.org hello"
        ));
    }

    #[test]
    fn room_mention_counts_as_mention() {
        let bot_user_id = owned_user_id!("@bot:example.org");
        let event = message_event(json!({
            "type": "m.room.message",
            "event_id": "$1",
            "room_id": "!room:example.org",
            "sender": "@alice:example.org",
            "origin_server_ts": 1,
            "content": {
                "msgtype": "m.text",
                "body": "@room hello",
                "m.mentions": {
                    "room": true
                }
            }
        }));

        assert!(is_bot_mentioned(&event, &bot_user_id, "@room hello"));
    }

    #[test]
    fn first_selection_returns_the_first_callback_choice() {
        let selections = vec!["agent_switch:2".to_string(), "agent_switch:3".to_string()];

        assert_eq!(
            first_selection(&selections),
            Some("agent_switch:2".to_string())
        );
        assert_eq!(first_selection(&[]), None);
    }

    #[test]
    fn auto_join_policy_always_joins_invites() {
        let cfg = MatrixAccountConfig {
            auto_join: AutoJoinPolicy::Always,
            room_policy: GroupPolicy::Open,
            ..Default::default()
        };

        assert!(should_auto_join_invite(
            &cfg,
            "@alice:example.org",
            "!ops:example.org",
        ));
    }

    #[test]
    fn auto_join_policy_off_ignores_invites() {
        let cfg = MatrixAccountConfig {
            auto_join: AutoJoinPolicy::Off,
            room_policy: GroupPolicy::Open,
            ..Default::default()
        };

        assert!(!should_auto_join_invite(
            &cfg,
            "@alice:example.org",
            "!ops:example.org",
        ));
    }

    #[test]
    fn auto_join_allowlist_uses_existing_user_and_room_allowlists() {
        let cfg = MatrixAccountConfig {
            auto_join: AutoJoinPolicy::Allowlist,
            room_policy: GroupPolicy::Open,
            user_allowlist: vec!["@alice:example.org".into()],
            room_allowlist: vec!["!ops:example.org".into()],
            ..Default::default()
        };

        assert!(should_auto_join_invite(
            &cfg,
            "@alice:example.org",
            "!other:example.org",
        ));
        assert!(should_auto_join_invite(
            &cfg,
            "@bob:example.org",
            "!ops:example.org",
        ));
        assert!(!should_auto_join_invite(
            &cfg,
            "@mallory:example.org",
            "!other:example.org",
        ));
    }

    #[test]
    fn auto_join_never_bypasses_room_allowlist() {
        let cfg = MatrixAccountConfig {
            auto_join: AutoJoinPolicy::Always,
            room_policy: GroupPolicy::Allowlist,
            room_allowlist: vec!["!ops:example.org".into()],
            user_allowlist: vec!["@alice:example.org".into()],
            ..Default::default()
        };

        assert!(should_auto_join_invite(
            &cfg,
            "@mallory:example.org",
            "!ops:example.org",
        ));
        assert!(!should_auto_join_invite(
            &cfg,
            "@alice:example.org",
            "!private:example.org",
        ));
    }

    #[test]
    fn auto_join_respects_disabled_room_policy() {
        let cfg = MatrixAccountConfig {
            auto_join: AutoJoinPolicy::Always,
            room_policy: GroupPolicy::Disabled,
            ..Default::default()
        };

        assert!(!should_auto_join_invite(
            &cfg,
            "@alice:example.org",
            "!ops:example.org",
        ));
    }

    #[test]
    fn parse_geo_uri_accepts_location_with_accuracy_suffix() {
        assert_eq!(
            parse_geo_uri("geo:51.5008,-0.1247;u=35"),
            Some((51.5008, -0.1247))
        );
    }

    #[test]
    fn audio_format_prefers_mimetype_then_filename() {
        assert_eq!(
            audio_format_from_metadata(Some("audio/webm"), Some("voice.ogg")),
            "webm"
        );
        assert_eq!(
            audio_format_from_metadata(None, Some("voice-note.opus")),
            "opus"
        );
        assert_eq!(audio_format_from_metadata(None, None), "ogg");
    }

    #[test]
    fn infer_audio_kind_treats_opus_as_voice() {
        let audio = AudioMessageEventContent::plain(
            "voice-note.opus".to_string(),
            mxc_uri!("mxc://example.org/voice").to_owned(),
        );

        assert!(matches!(
            infer_audio_kind(&audio),
            ChannelMessageKind::Voice
        ));
    }

    #[test]
    fn saved_audio_filename_uses_cleaned_original_name() {
        assert_eq!(
            saved_audio_filename("$event:example.org", Some("nested/path voice"), None, "ogg"),
            "path_voice.ogg"
        );
    }

    #[test]
    fn location_dispatch_body_includes_coordinates() {
        let location = LocationMessageEventContent::new(
            "Meet me here".to_string(),
            "geo:38.7223,-9.1393".to_string(),
        );

        assert_eq!(
            location_dispatch_body(&location, 38.7223, -9.1393),
            "Meet me here\n\nShared location: 38.7223, -9.1393"
        );
    }
}
