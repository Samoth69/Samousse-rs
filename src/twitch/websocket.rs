use crate::config::TwitchWatcher;
use crate::message::Message;
use anyhow::Context;
use tokio::sync::mpsc::Sender;
use tracing::{debug, debug_span, info, warn, Instrument};
use twitch_api::client::ClientDefault;
use twitch_api::eventsub::stream::{StreamOfflineV1, StreamOnlineV1};
use twitch_api::eventsub::{
    Event, EventsubWebsocketData, ReconnectPayload, SessionData, WelcomePayload,
};
use twitch_api::twitch_oauth2::UserToken;
use twitch_api::types::UserId;
use twitch_api::{eventsub, HelixClient};

pub async fn run(
    twitch_token: String,
    sender: Sender<Message>,
    config: TwitchWatcher,
) -> anyhow::Result<()> {
    let client: HelixClient<_> = HelixClient::with_client(
        <reqwest::Client>::default_client_with_name(Some("samousse-rs".parse()?))?,
    );

    let mut ws = WebsocketClient {
        sender,
        session_id: None,
        token: UserToken::from_token(&client, twitch_token.into()).await?,
        client,
        user_ids: config
            .channels
            .iter()
            .map(|i| UserId::new(i.twitch_channel_id.to_string()))
            .collect(),
        connect_url: twitch_api::TWITCH_EVENTSUB_WEBSOCKET_URL.clone(),
    };
    
    ws.run().await?;

    Ok(())
}

pub struct WebsocketClient {
    sender: Sender<Message>,
    user_ids: Vec<UserId>,

    /// The session id of the websocket connection
    session_id: Option<String>,
    /// The token used to authenticate with the Twitch API
    token: UserToken,
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
        let socket_config = tungstenite::protocol::WebSocketConfig {
            max_write_buffer_size: 2048,
            max_message_size: Some(64 << 20), // 64 MiB
            max_frame_size: Some(16 << 20),   // 16 MiB
            accept_unmasked_frames: false,
            ..tungstenite::protocol::WebSocketConfig::default()
        };
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
            )
        }

        Ok(())
    }

    /// Process a message from the websocket
    pub async fn process_message(&mut self, msg: tungstenite::Message) -> anyhow::Result<()> {
        match msg {
            tungstenite::Message::Text(s) => {
                info!("{s}");
                // Parse the message into a [twitch_api::eventsub::EventsubWebsocketData]
                match Event::parse_websocket(&s)? {
                    EventsubWebsocketData::Welcome {
                        payload: WelcomePayload { session },
                        ..
                    }
                    | EventsubWebsocketData::Reconnect {
                        payload: ReconnectPayload { session },
                        ..
                    } => {
                        self.process_welcome_message(session).await?;
                        Ok(())
                    }
                    // Here is where you would handle the events you want to listen to
                    EventsubWebsocketData::Notification {
                        metadata: _,
                        payload,
                    } => {
                        match payload {
                            Event::ChannelBanV1(eventsub::Payload { message, .. }) => {
                                info!(?message, "got ban event");
                            }
                            Event::ChannelUnbanV1(eventsub::Payload { message, .. }) => {
                                info!(?message, "got ban event");
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
            tungstenite::Message::Close(_) => todo!(),
            _ => Ok(()),
        }
    }

    pub async fn process_welcome_message(&mut self, data: SessionData<'_>) -> anyhow::Result<()> {
        self.session_id = Some(data.id.to_string());
        if let Some(url) = data.reconnect_url {
            self.connect_url = url.parse()?;
        }
        let transport = eventsub::Transport::websocket(data.id.clone());
        // self.client
        //     .create_eventsub_subscription(
        //         eventsub::channel::ChannelBanV1::broadcaster_user_id(self.user_id.clone()),
        //         transport.clone(),
        //         &self.token,
        //     )
        //     .await?;
        // self.client
        //     .create_eventsub_subscription(
        //         eventsub::channel::ChannelUnbanV1::broadcaster_user_id(self.user_id.clone()),
        //         transport,
        //         &self.token,
        //     )
        //     .await?;
        for user_id in &self.user_ids {
            debug!("Subscribing to events for {}", user_id);
            self.client
                .create_eventsub_subscription(
                    StreamOnlineV1::broadcaster_user_id(user_id.clone()),
                    transport.clone(),
                    &self.token,
                )
                .await?;self.client
                .create_eventsub_subscription(
                    StreamOfflineV1::broadcaster_user_id(user_id.clone()),
                    transport.clone(),
                    &self.token,
                )
                .await?;
        }
        info!("listening to ban and unbans");
        Ok(())
    }
}
