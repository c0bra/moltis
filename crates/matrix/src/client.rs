use std::sync::Arc;

use {
    matrix_sdk::{Client, Room, config::SyncSettings, ruma::OwnedUserId},
    secrecy::ExposeSecret,
    tokio_util::sync::CancellationToken,
    tracing::{info, instrument, warn},
};

use moltis_channels::{Error as ChannelError, Result as ChannelResult};

use crate::{config::MatrixAccountConfig, handler, state::AccountStateMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AuthMode {
    AccessToken,
    Password,
}

#[instrument(skip(config), fields(homeserver = %config.homeserver))]
pub(crate) async fn build_client(config: &MatrixAccountConfig) -> ChannelResult<Client> {
    Client::builder()
        .homeserver_url(&config.homeserver)
        .build()
        .await
        .map_err(|error| ChannelError::external("matrix client build", error))
}

pub(crate) fn auth_mode(config: &MatrixAccountConfig) -> ChannelResult<AuthMode> {
    let access_token = config.access_token.expose_secret().trim();
    if !access_token.is_empty() && access_token != moltis_common::secret_serde::REDACTED {
        return Ok(AuthMode::AccessToken);
    }

    let password = config
        .password
        .as_ref()
        .map(|secret| secret.expose_secret().trim())
        .unwrap_or_default();
    if password.is_empty() || password == moltis_common::secret_serde::REDACTED {
        return Err(ChannelError::invalid_input(
            "either access_token or password is required",
        ));
    }

    if config.user_id.as_deref().is_none_or(str::is_empty) {
        return Err(ChannelError::invalid_input(
            "user_id is required when using password authentication",
        ));
    }

    Ok(AuthMode::Password)
}

#[instrument(skip(client, config), fields(account_id))]
pub(crate) async fn authenticate_client(
    client: &Client,
    account_id: &str,
    config: &MatrixAccountConfig,
) -> ChannelResult<OwnedUserId> {
    match auth_mode(config)? {
        AuthMode::AccessToken => {
            restore_access_token_session(client, account_id, config).await?;
            let bot_user_id = client
                .whoami()
                .await
                .map_err(|error| ChannelError::external("matrix whoami", error))?
                .user_id;
            info!(account_id, user_id = %bot_user_id, "matrix session restored");
            Ok(bot_user_id)
        },
        AuthMode::Password => {
            login_with_password(client, account_id, config).await?;
            let bot_user_id = client
                .whoami()
                .await
                .map_err(|error| ChannelError::external("matrix whoami", error))?
                .user_id;
            info!(account_id, user_id = %bot_user_id, "matrix password login complete");
            Ok(bot_user_id)
        },
    }
}

#[instrument(skip(client, accounts), fields(account_id, user_id = %bot_user_id))]
pub(crate) fn register_event_handlers(
    client: &Client,
    account_id: &str,
    accounts: &AccountStateMap,
    bot_user_id: &OwnedUserId,
) {
    let accounts_for_msg = Arc::clone(accounts);
    let account_id_msg = account_id.to_string();
    let bot_uid_msg = bot_user_id.clone();
    client.add_event_handler(
        move |ev: matrix_sdk::ruma::events::room::message::OriginalSyncRoomMessageEvent,
              room: Room| {
            let accounts = Arc::clone(&accounts_for_msg);
            let aid = account_id_msg.clone();
            let buid = bot_uid_msg.clone();
            async move {
                handler::handle_room_message(ev, room, aid, accounts, buid).await;
            }
        },
    );

    let accounts_for_poll = Arc::clone(accounts);
    let account_id_poll = account_id.to_string();
    client.add_event_handler(
        move |ev: matrix_sdk::ruma::events::poll::response::OriginalSyncPollResponseEvent,
              room: Room| {
            let accounts = Arc::clone(&accounts_for_poll);
            let aid = account_id_poll.clone();
            let sender_id = ev.sender.to_string();
            let callback_data = handler::first_selection(&ev.content.selections);
            async move {
                handler::handle_poll_response(room, aid, accounts, sender_id, callback_data).await;
            }
        },
    );

    let accounts_for_unstable_poll = Arc::clone(accounts);
    let account_id_unstable_poll = account_id.to_string();
    client.add_event_handler(
        move |ev: matrix_sdk::ruma::events::poll::unstable_response::OriginalSyncUnstablePollResponseEvent,
              room: Room| {
            let accounts = Arc::clone(&accounts_for_unstable_poll);
            let aid = account_id_unstable_poll.clone();
            let sender_id = ev.sender.to_string();
            let callback_data = handler::first_selection(&ev.content.poll_response.answers);
            async move {
                handler::handle_poll_response(room, aid, accounts, sender_id, callback_data).await;
            }
        },
    );

    let accounts_for_invite = Arc::clone(accounts);
    let account_id_invite = account_id.to_string();
    let bot_uid_invite = bot_user_id.clone();
    client.add_event_handler(
        move |ev: matrix_sdk::ruma::events::room::member::StrippedRoomMemberEvent, room: Room| {
            let accounts = Arc::clone(&accounts_for_invite);
            let aid = account_id_invite.clone();
            let buid = bot_uid_invite.clone();
            async move {
                handler::handle_invite(ev, room, aid, accounts, buid).await;
            }
        },
    );
}

#[instrument(skip(client, cancel), fields(account_id))]
pub(crate) async fn sync_once_and_spawn_loop(
    client: &Client,
    account_id: &str,
    cancel: CancellationToken,
) -> ChannelResult<()> {
    info!(account_id, "performing initial sync...");
    client
        .sync_once(SyncSettings::default())
        .await
        .map_err(|error| ChannelError::external("matrix initial sync", error))?;
    info!(
        account_id,
        "initial sync complete, starting continuous sync"
    );

    let account_id_for_sync = account_id.to_string();
    let client_for_sync = client.clone();
    tokio::spawn(async move {
        tokio::select! {
            _ = client_for_sync.sync(SyncSettings::default()) => {
                warn!(account_id = %account_id_for_sync, "matrix sync loop ended unexpectedly");
            }
            () = cancel.cancelled() => {
                info!(account_id = %account_id_for_sync, "matrix sync loop cancelled");
            }
        }
    });

    Ok(())
}

fn restore_session_user_id(configured_user_id: Option<&str>) -> ChannelResult<OwnedUserId> {
    match configured_user_id {
        Some(user_id) => user_id
            .parse()
            .map_err(|error: matrix_sdk::ruma::IdParseError| {
                ChannelError::invalid_input(format!("invalid user_id: {error}"))
            }),
        None => "@moltis:invalid"
            .parse()
            .map_err(|error: matrix_sdk::ruma::IdParseError| {
                ChannelError::external("matrix session placeholder user_id", error)
            }),
    }
}

#[instrument(skip(client, config), fields(account_id))]
async fn restore_access_token_session(
    client: &Client,
    account_id: &str,
    config: &MatrixAccountConfig,
) -> ChannelResult<()> {
    let session = matrix_sdk::authentication::matrix::MatrixSession {
        meta: matrix_sdk::SessionMeta {
            user_id: restore_session_user_id(config.user_id.as_deref())?,
            device_id: config
                .device_id
                .clone()
                .unwrap_or_else(|| format!("moltis_{account_id}"))
                .into(),
        },
        tokens: matrix_sdk::SessionTokens {
            access_token: config.access_token.expose_secret().clone(),
            refresh_token: None,
        },
    };

    client
        .restore_session(session)
        .await
        .map_err(|error| ChannelError::external("matrix session restore", error))
}

#[instrument(skip(client, config), fields(account_id))]
async fn login_with_password(
    client: &Client,
    account_id: &str,
    config: &MatrixAccountConfig,
) -> ChannelResult<()> {
    let user_id = config
        .user_id
        .as_deref()
        .filter(|user_id| !user_id.is_empty())
        .ok_or_else(|| {
            ChannelError::invalid_input("user_id is required when using password authentication")
        })?;
    let password = config
        .password
        .as_ref()
        .map(|secret| secret.expose_secret())
        .ok_or_else(|| ChannelError::invalid_input("password is required"))?;

    let mut login = client.matrix_auth().login_username(user_id, password);
    if let Some(device_id) = config.device_id.as_deref().filter(|id| !id.is_empty()) {
        login = login.device_id(device_id);
    }
    if let Some(display_name) = config
        .device_display_name
        .as_deref()
        .filter(|name| !name.is_empty())
    {
        login = login.initial_device_display_name(display_name);
    }

    login
        .send()
        .await
        .map_err(|error| ChannelError::external("matrix password login", error))?;

    info!(account_id, "matrix password login restored session");
    Ok(())
}

#[cfg(test)]
mod tests {
    use {super::*, secrecy::Secret};

    fn config() -> MatrixAccountConfig {
        MatrixAccountConfig {
            homeserver: "https://matrix.example.com".into(),
            ..Default::default()
        }
    }

    #[test]
    fn access_token_auth_is_preferred_when_both_credentials_exist() {
        let cfg = MatrixAccountConfig {
            access_token: Secret::new("syt_test".into()),
            password: Some(Secret::new("wordpass".into())),
            user_id: Some("@bot:example.com".into()),
            ..config()
        };

        assert!(matches!(auth_mode(&cfg), Ok(AuthMode::AccessToken)));
    }

    #[test]
    fn password_auth_is_used_when_token_is_missing() {
        let cfg = MatrixAccountConfig {
            password: Some(Secret::new("wordpass".into())),
            user_id: Some("@bot:example.com".into()),
            ..config()
        };

        assert!(matches!(auth_mode(&cfg), Ok(AuthMode::Password)));
    }

    #[test]
    fn password_auth_requires_user_id() {
        let cfg = MatrixAccountConfig {
            password: Some(Secret::new("wordpass".into())),
            ..config()
        };

        let error = match auth_mode(&cfg) {
            Ok(mode) => panic!("password auth without user_id should fail, got {mode:?}"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("user_id is required"));
    }

    #[test]
    fn authentication_requires_token_or_password() {
        let error = match auth_mode(&config()) {
            Ok(mode) => panic!("missing auth should fail, got {mode:?}"),
            Err(error) => error.to_string(),
        };
        assert!(error.contains("either access_token or password is required"));
    }

    #[test]
    fn restore_session_user_id_uses_configured_value_when_present() {
        let user_id = restore_session_user_id(Some("@moltis:example.com"))
            .unwrap_or_else(|error| panic!("configured user_id should parse: {error}"));

        assert_eq!(user_id.as_str(), "@moltis:example.com");
    }

    #[test]
    fn restore_session_user_id_falls_back_to_placeholder_when_missing() {
        let user_id = restore_session_user_id(None)
            .unwrap_or_else(|error| panic!("placeholder user_id should parse: {error}"));

        assert_eq!(user_id.as_str(), "@moltis:invalid");
    }
}
