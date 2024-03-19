use crate::config::TwitchWatcher;
use crate::inter_comm::{InterComm, MessageType};
use crate::twitch::auth::{get_client_ids, TwitchToken};
use anyhow::Context;
use std::time::Duration;
use tokio::sync::mpsc::Sender;
use tokio::time::sleep;
use tracing::{debug, debug_span, error, info, trace, warn, Instrument};
use twitch_api::client::ClientDefault;
use twitch_api::eventsub::stream::{StreamOfflineV1, StreamOnlineV1};
use twitch_api::eventsub::{
    Event, EventsubWebsocketData, Message, ReconnectPayload, SessionData, WelcomePayload,
};
use twitch_api::types::{UserId, UserName};
use twitch_api::{eventsub, HelixClient};
use twitch_oauth2::UserToken;

pub async fn run(sender: Sender<InterComm>, config: TwitchWatcher) -> anyhow::Result<()> {
    let twitch_client: HelixClient<_> = HelixClient::with_client(
        <reqwest::Client>::default_client_with_name(Some("samousse-rs".parse()?))?,
    );

    let mut ws = WebsocketClient {
        sender,
        session_id: None,
        token: TwitchToken::new().await?,
        client: twitch_client,
        user_ids: config
            .channels
            .iter()
            .map(|i| UserId::new(i.twitch_channel_id.to_string()))
            .collect(),
        connect_url: twitch_api::TWITCH_EVENTSUB_WEBSOCKET_URL.clone(),
    };

    ws.run().await?;
    // let ws_client = tokio::spawn(async move {ws.run().await});
    //
    // tokio::try_join!(ws_client)?.0.unwrap();

    Ok(())
}

pub struct WebsocketClient {
    sender: Sender<InterComm>,
    user_ids: Vec<UserId>,

    /// The session id of the websocket connection
    session_id: Option<String>,
    /// The token used to authenticate with the Twitch API
    token: TwitchToken,
    /// The client used to make requests to the Twitch API
    client: HelixClient<'static, reqwest::Client>,
    /// The url to use for websocket
    connect_url: url::Url,
}

impl WebsocketClient {
    async fn connect(
        &self,
    ) -> anyhow::Result<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
    > {
        let socket_config = tungstenite::protocol::WebSocketConfig::default();
        info!("connecting to websocket");
        let (socket, _) = tokio_tungstenite::connect_async_with_config(
            &self.connect_url,
            Some(socket_config),
            false,
        )
        .await
        .context("Can't connect")?;

        Ok(socket)
    }

    async fn run(&mut self) -> anyhow::Result<()> {
        let mut s = self.connect().await?;

        loop {
            tokio::select!(
                Some(msg) = futures::StreamExt::next(&mut s) => {
                    let span = debug_span!("Received message", raw_message= ?msg);
                    let msg = match msg {
                        Err(tungstenite::Error::Protocol(tungstenite::error::ProtocolError::ResetWithoutClosingHandshake)) => {
                            warn!("connection was sent an unexpected frame or was reset, reestablishing it");
                            s = self.connect().instrument(span).await.context("when reestablishing connection")?;
                            continue;
                        }
                        _=> msg.context("when getting message")?,
                    };
                    self.process_message(msg).instrument(span).await?
                }
                else => {
                    warn!("Twitch websocket loop exited, waiting before restart");
                    sleep(Duration::from_secs(30)).await;
                }
            )
        }
    }

    /// Process a message from the websocket
    pub async fn process_message(&mut self, msg: tungstenite::Message) -> anyhow::Result<()> {
        trace!("processing");
        match msg {
            tungstenite::Message::Text(s) => {
                // Parse the message into a [twitch_api::eventsub::EventsubWebsocketData]
                match Event::parse_websocket(&s)? {
                    EventsubWebsocketData::Welcome {
                        payload: WelcomePayload { session },
                        ..
                    }
                    | EventsubWebsocketData::Reconnect {
                        payload: ReconnectPayload { session },
                        ..
                    } => match self.process_welcome_message(session).await {
                        Err(e) => {
                            error!("Error on processing welcome message {}", e);
                            Ok(())
                        }
                        _ => Ok(()),
                    },
                    // Here is where you would handle the events you want to listen to
                    EventsubWebsocketData::Notification {
                        metadata: _,
                        payload,
                    } => {
                        match payload {
                            Event::StreamOnlineV1(eventsub::Payload {
                                message: Message::Notification(notif),
                                ..
                            }) => {
                                self.handle_streamer_online(
                                    notif.broadcaster_user_id,
                                    notif.broadcaster_user_login,
                                )
                                .await?;
                            }
                            Event::StreamOfflineV1(eventsub::Payload {
                                message: Message::Notification(notif),
                                ..
                            }) => {
                                self.handle_streamer_offline(
                                    notif.broadcaster_user_id,
                                    notif.broadcaster_user_login,
                                )
                                .await?;
                            }
                            _ => {}
                        }
                        Ok(())
                    }
                    EventsubWebsocketData::Revocation {
                        metadata,
                        payload: _,
                    } => {
                        info!("got revocation event: {metadata:?}");
                        Ok(())
                    }
                    EventsubWebsocketData::Keepalive {
                        metadata: _,
                        payload: _,
                    } => Ok(()),
                    _ => Ok(()),
                }
            }
            tungstenite::Message::Close(_) => {
                info!("Received close event");
                Ok(())
            }
            _ => Ok(()),
        }
    }

    pub async fn process_welcome_message(&mut self, data: SessionData<'_>) -> anyhow::Result<()> {
        self.session_id = Some(data.id.to_string());
        if let Some(url) = data.reconnect_url {
            self.connect_url = url.parse()?;
        }
        let transport = eventsub::Transport::websocket(data.id.clone());
        let cred = get_client_ids();
        let token = UserToken::from_existing_unchecked(
            self.token.access_token.to_owned(),
            Some(self.token.refresh_token.to_owned().into()),
            cred.0,
            Some(cred.1.into()),
            "samoth691".into(),
            "53102824".into(),
            None,
            None,
        );
        for user_id in &self.user_ids {
            debug!("Subscribing to events for {}", user_id);
            self.client
                .create_eventsub_subscription(
                    StreamOnlineV1::broadcaster_user_id(user_id.clone()),
                    transport.clone(),
                    &token,
                )
                .await?;
            self.client
                .create_eventsub_subscription(
                    StreamOfflineV1::broadcaster_user_id(user_id.clone()),
                    transport.clone(),
                    &token,
                )
                .await?;
        }
        info!("welcome message sent");
        Ok(())
    }

    pub async fn handle_streamer_online(
        &self,
        broadcaster_user_id: UserId,
        broadcaster_user_login: UserName,
    ) -> anyhow::Result<()> {
        info!("{} stream is online", broadcaster_user_login);

        self.sender
            .send(InterComm {
                message_type: MessageType::TwitchStreamOnline,
                streamer_user_id: broadcaster_user_id.into(),
                streamer_user_login: broadcaster_user_login.into(),
            })
            .await?;

        Ok(())
    }
    pub async fn handle_streamer_offline(
        &self,
        broadcaster_user_id: UserId,
        broadcaster_user_login: UserName,
    ) -> anyhow::Result<()> {
        info!("{} stream is offline", broadcaster_user_login);

        self.sender
            .send(InterComm {
                message_type: MessageType::TwitchStreamOffline,
                streamer_user_id: broadcaster_user_id.into(),
                streamer_user_login: broadcaster_user_login.into(),
            })
            .await?;

        Ok(())
    }
}
